//! Anchor creation and re-anchoring (SPEC 7).
//!
//! An anchor is the quoted text plus a little context on each side and the
//! last known offset. Re-anchoring after an external change tries, in order:
//!
//! 1. exact `prefix + quote + suffix`, the occurrence nearest to `hint`;
//! 2. exact `quote`, nearest to `hint`;
//! 3. fuzzy `quote`: Levenshtein distance at most 10 % of its length in
//!    chars, best context similarity first;
//! 4. otherwise [`Resolution::Detached`].
//!
//! The fuzzy step finds candidates with the pigeonhole principle: split the
//! quote into `k + 1` pieces; a match with at most `k` edits contains one of
//! them unchanged. Pieces are found with `memmem`, hits are clustered by the
//! quote start they imply, and the best-voted clusters are verified with a
//! semi-global edit-distance pass over a small window.

use std::ops::Range;

use mdedit_core::TextBuffer;
use memchr::memmem;

use crate::{Anchor, Resolution};

/// How much context [`make_anchor`] keeps on each side of the quote.
pub const CONTEXT_CHARS: usize = 32;

/// Fuzzy matching tolerates this many edits per 10 chars of quote.
const FUZZY_DIVISOR: usize = 10;
/// Clusters of piece hits verified with the edit-distance pass.
const MAX_CLUSTERS: usize = 8;
/// Stop collecting piece hits after this many (pathological repetitive text).
const MAX_HITS: usize = 200_000;

/// Build an anchor for a new comment on `range` (byte offsets on char
/// boundaries). The prefix stops at the start of the line and the suffix
/// stops after the first line break, so edits to neighbouring lines do not
/// weaken the context.
pub fn make_anchor(buf: &dyn TextBuffer, range: Range<usize>) -> Anchor {
    let len = buf.len_bytes();
    let range = range.start.min(len)..range.end.min(len);
    let window = 4 * CONTEXT_CHARS;

    // Prefix: the last `window` bytes between the line start and the quote.
    let line_start = buf.line_to_byte(buf.byte_to_line(range.start));
    let mut tail: Vec<u8> = Vec::new();
    for chunk in buf.chunks(line_start..range.start) {
        tail.extend_from_slice(chunk);
        if tail.len() > 2 * window {
            tail.drain(..tail.len() - window);
        }
    }
    let cut = tail.len().saturating_sub(window);
    let mut tail = &tail[cut..];
    while let Some((&b, rest)) = tail.split_first() {
        if b & 0xC0 != 0x80 {
            break;
        }
        tail = rest;
    }
    let tail = String::from_utf8_lossy(tail);
    let tail = tail.rsplit(['\n', '\r']).next().unwrap_or_default();
    let skip = tail.chars().count().saturating_sub(CONTEXT_CHARS);
    let prefix: String = tail.chars().skip(skip).collect();

    // Suffix: up to the first line break (included) or CONTEXT_CHARS chars.
    let mut head: Vec<u8> = Vec::new();
    for chunk in buf.chunks(range.end..len) {
        head.extend_from_slice(chunk);
        if head.len() >= window || chunk.contains(&b'\n') {
            break;
        }
    }
    let head = match std::str::from_utf8(&head) {
        Ok(s) => s,
        Err(e) => std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or_default(),
    };
    let mut suffix = String::new();
    let mut chars = head.chars().peekable();
    while let Some(c) = chars.next() {
        if suffix.chars().count() == CONTEXT_CHARS {
            break;
        }
        suffix.push(c);
        if c == '\n' || (c == '\r' && chars.peek() != Some(&'\n')) {
            break;
        }
    }

    Anchor { quote: buf.slice(range.clone()).into_owned(), prefix, suffix, hint: range.start }
}

/// Find an anchor in the text (see the module docs for the order).
pub fn reanchor(anchor: &Anchor, buf: &dyn TextBuffer) -> Resolution {
    find(anchor, &buf.text())
}

pub(crate) fn find(anchor: &Anchor, text: &str) -> Resolution {
    let quote = anchor.quote.as_str();
    let full = format!("{}{quote}{}", anchor.prefix, anchor.suffix);
    if !full.is_empty()
        && let Some(start) = nearest(text, &full, anchor.prefix.len(), anchor.hint)
    {
        return Resolution::Attached(start..start + quote.len());
    }
    if quote.is_empty() {
        return Resolution::Detached;
    }
    if let Some(start) = nearest(text, quote, 0, anchor.hint) {
        return Resolution::Attached(start..start + quote.len());
    }
    match fuzzy(anchor, text) {
        Some(range) => Resolution::Attached(range),
        None => Resolution::Detached,
    }
}

/// Start of the quote (`offset` bytes into `needle`) for the occurrence of
/// `needle` whose quote start is nearest to `hint`.
fn nearest(text: &str, needle: &str, offset: usize, hint: usize) -> Option<usize> {
    let finder = memmem::Finder::new(needle.as_bytes());
    let bytes = text.as_bytes();
    let mut best: Option<usize> = None;
    let mut from = 0;
    // Overlapping occurrences count too, so step one byte past each hit.
    while let Some(p) = bytes.get(from..).and_then(|rest| finder.find(rest)).map(|p| from + p) {
        from = p + 1;
        let q = p + offset;
        if best.is_none_or(|b| q.abs_diff(hint) < b.abs_diff(hint)) {
            best = Some(q);
        }
        if q >= hint {
            break; // later occurrences are only further away
        }
    }
    best
}

struct Cluster {
    lo: usize,
    hi: usize,
    votes: usize,
}

fn fuzzy(anchor: &Anchor, text: &str) -> Option<Range<usize>> {
    let quote: Vec<char> = anchor.quote.chars().collect();
    let m = quote.len();
    let k = m / FUZZY_DIVISOR;
    if k == 0 {
        return None;
    }

    // Piece hits → implied byte start of the quote.
    let byte_at: Vec<usize> = anchor.quote.char_indices().map(|(i, _)| i).chain([anchor.quote.len()]).collect();
    let pieces = k + 1;
    let mut starts: Vec<usize> = Vec::new();
    'pieces: for i in 0..pieces {
        let (a, b) = (i * m / pieces, (i + 1) * m / pieces);
        if a == b {
            continue;
        }
        let piece = &anchor.quote[byte_at[a]..byte_at[b]];
        for p in memmem::find_iter(text.as_bytes(), piece.as_bytes()) {
            starts.push(p.saturating_sub(byte_at[a]));
            if starts.len() >= MAX_HITS {
                break 'pieces;
            }
        }
    }
    if starts.is_empty() {
        return None;
    }

    // Cluster implied starts that are within the edit budget of each other.
    starts.sort_unstable();
    let slack = 4 * (k + 1);
    let mut clusters: Vec<Cluster> = Vec::new();
    for s in starts {
        match clusters.last_mut() {
            Some(c) if s <= c.hi + slack => {
                c.hi = s;
                c.votes += 1;
            }
            _ => clusters.push(Cluster { lo: s, hi: s, votes: 1 }),
        }
    }
    clusters.sort_by_key(|c| (std::cmp::Reverse(c.votes), c.lo.abs_diff(anchor.hint)));
    clusters.truncate(MAX_CLUSTERS);

    let mut best: Option<(f64, usize, Range<usize>)> = None;
    for c in clusters {
        let lo = floor_boundary(text, c.lo.saturating_sub(slack));
        let hi = ceil_boundary(text, c.hi + anchor.quote.len() + slack);
        let Some((dist, range)) = semi_global(&quote, text, lo..hi, k) else { continue };
        let score = context_similarity(anchor, text, &range) - dist as f64 / m as f64;
        let near = range.start.abs_diff(anchor.hint);
        if best.as_ref().is_none_or(|(bs, bn, _)| score > *bs || (score == *bs && near < *bn)) {
            best = Some((score, near, range));
        }
    }
    best.map(|(_, _, r)| r)
}

fn floor_boundary(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Best approximate occurrence of `pattern` anywhere in `text[window]`, if
/// its edit distance is at most `k`. Returns the distance and byte range.
fn semi_global(pattern: &[char], text: &str, window: Range<usize>, k: usize) -> Option<(usize, Range<usize>)> {
    let slice = &text[window.clone()];
    let chars: Vec<char> = slice.chars().collect();
    let offs: Vec<usize> = slice.char_indices().map(|(i, _)| window.start + i).chain([window.end]).collect();
    let w = chars.len();

    // (distance, start column) per cell; row 0 lets a match start anywhere.
    let mut prev: Vec<(usize, usize)> = (0..=w).map(|j| (0, j)).collect();
    let mut cur = vec![(0, 0); w + 1];
    for (i, &pc) in pattern.iter().enumerate() {
        cur[0] = (i + 1, 0);
        let mut row_min = cur[0].0;
        for j in 1..=w {
            let diag = (prev[j - 1].0 + usize::from(pc != chars[j - 1]), prev[j - 1].1);
            let up = (prev[j].0 + 1, prev[j].1);
            let left = (cur[j - 1].0 + 1, cur[j - 1].1);
            let mut cell = diag;
            if up.0 < cell.0 {
                cell = up;
            }
            if left.0 < cell.0 {
                cell = left;
            }
            cur[j] = cell;
            row_min = row_min.min(cell.0);
        }
        if row_min > k {
            return None;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let (end, &(dist, start)) = prev.iter().enumerate().min_by_key(|(_, (d, _))| *d)?;
    (dist <= k && start < end).then(|| (dist, offs[start]..offs[end]))
}

/// 0..1: how well the text around `range` matches the anchor's context.
/// Neutral (0.5) when the anchor has no context.
fn context_similarity(anchor: &Anchor, text: &str, range: &Range<usize>) -> f64 {
    let pre: Vec<char> = anchor.prefix.chars().collect();
    let suf: Vec<char> = anchor.suffix.chars().collect();
    if pre.is_empty() && suf.is_empty() {
        return 0.5;
    }
    let before: Vec<char> = {
        let mut v: Vec<char> = text[..range.start].chars().rev().take(pre.len()).collect();
        v.reverse();
        v
    };
    let after: Vec<char> = text[range.end..].chars().take(suf.len()).collect();
    let sim = |a: &[char], b: &[char]| {
        if a.is_empty() { 1.0 } else { 1.0 - levenshtein(a, b) as f64 / a.len().max(b.len()) as f64 }
    };
    let (lp, ls) = (pre.len() as f64, suf.len() as f64);
    (sim(&pre, &before) * lp + sim(&suf, &after) * ls) / (lp + ls)
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use mdedit_core::RopeBuffer;

    use super::*;

    const DOC: &str = "# Transfers\n\nIn the recipient's currency, 3–5 business days\n\nSEPA payouts arrive the next business day. Card payouts arrive in 3–5 business days too.\n";

    fn anchor_on(text: &str, quote: &str) -> Anchor {
        let start = text.find(quote).unwrap();
        make_anchor(&RopeBuffer::from_text(text), start..start + quote.len())
    }

    fn attached(text: &str, anchor: &Anchor) -> &'static str {
        match reanchor(anchor, &RopeBuffer::from_text(text)) {
            Resolution::Attached(r) => text[r].to_owned().leak(),
            Resolution::Detached => "<detached>",
        }
    }

    #[test]
    fn make_anchor_matches_spec_example() {
        let a = anchor_on(DOC, "3–5 business days");
        assert_eq!(a.quote, "3–5 business days");
        assert_eq!(a.prefix, "In the recipient's currency, ");
        assert_eq!(a.suffix, "\n");
        assert_eq!(a.hint, DOC.find("3–5").unwrap());
    }

    #[test]
    fn make_anchor_limits_context_to_32_chars() {
        let text = format!("{}é{}QUOTE{}\nnext", "x".repeat(40), "y".repeat(31), "z".repeat(40));
        let a = anchor_on(&text, "QUOTE");
        assert_eq!(a.prefix, format!("é{}", "y".repeat(31)));
        assert_eq!(a.prefix.chars().count(), CONTEXT_CHARS);
        assert_eq!(a.suffix, "z".repeat(32));
        // At the edges of the document.
        let a = anchor_on("QUOTE", "QUOTE");
        assert_eq!((a.prefix.as_str(), a.suffix.as_str()), ("", ""));
        let a = anchor_on("ab\r\nQUOTE\r\ncd", "QUOTE");
        assert_eq!((a.prefix.as_str(), a.suffix.as_str()), ("", "\r\n"));
    }

    #[test]
    fn unchanged_text_reanchors_in_place() {
        let a = anchor_on(DOC, "3–5 business days");
        let start = a.hint;
        assert_eq!(reanchor(&a, &RopeBuffer::from_text(DOC)), Resolution::Attached(start..start + a.quote.len()));
    }

    #[test]
    fn text_inserted_above_moves_the_anchor() {
        let a = anchor_on(DOC, "3–5 business days");
        let edited = format!("Intro paragraph added by another tool.\n\n{DOC}");
        let r = reanchor(&a, &RopeBuffer::from_text(&edited)).range().unwrap();
        assert_eq!(r.start, a.hint + 40);
        assert_eq!(&edited[r], "3–5 business days");
    }

    #[test]
    fn context_picks_the_right_duplicate() {
        // The quote appears twice; the second has different context. Even
        // with a hint pointing at the second one, context wins.
        let mut a = anchor_on(DOC, "3–5 business days");
        a.hint = DOC.rfind("3–5").unwrap();
        let r = reanchor(&a, &RopeBuffer::from_text(DOC)).range().unwrap();
        assert_eq!(r.start, DOC.find("3–5").unwrap());
    }

    #[test]
    fn changed_context_falls_back_to_nearest_quote() {
        let a = anchor_on(DOC, "3–5 business days");
        let edited = DOC.replace("In the recipient's currency, ", "Paid in local money: ");
        let r = reanchor(&a, &RopeBuffer::from_text(&edited)).range().unwrap();
        assert_eq!(r.start, edited.find("3–5").unwrap());
    }

    #[test]
    fn small_edit_in_quote_matches_fuzzily() {
        let quote = "SEPA payouts arrive the next business day.";
        let a = anchor_on(DOC, quote);
        // Two char edits in 42 chars (k = 4), plus a shift.
        let edited = format!("New line\n{}", DOC.replace(quote, "SEPA payout arrives the next business day."));
        assert_eq!(attached(&edited, &a), "SEPA payout arrives the next business day.");
        // A multibyte change counts as one char edit.
        let a = anchor_on(DOC, "In the recipient's currency, 3–5 business days");
        let edited = DOC.replace("3–5", "3-5");
        assert_eq!(attached(&edited, &a), "In the recipient's currency, 3-5 business days");
    }

    #[test]
    fn fuzzy_prefers_matching_context() {
        let quote = "arrive in five business days";
        let text = "Cards: payments arrive in five business days.\nWires: payments arrive in five business days.\n";
        let start = text.rfind(quote).unwrap();
        let a = make_anchor(&RopeBuffer::from_text(text), start..start + quote.len());
        let edited = text.replace(quote, "arrive in 5ive business days");
        let r = reanchor(&a, &RopeBuffer::from_text(&edited)).range().unwrap();
        assert_eq!(&edited[r.clone()], "arrive in 5ive business days");
        assert!(edited[..r.start].ends_with("Wires: payments "));
    }

    #[test]
    fn heavy_edit_or_deletion_detaches() {
        let quote = "SEPA payouts arrive the next business day.";
        let a = anchor_on(DOC, quote);
        assert_eq!(attached(&DOC.replace(quote, ""), &a), "<detached>");
        assert_eq!(attached(&DOC.replace(quote, "Instant payouts settle within the hour."), &a), "<detached>");
        // Short quotes get no fuzzy tolerance.
        let a = anchor_on(DOC, "SEPA");
        assert_eq!(attached(&DOC.replace("SEPA", "SEPB"), &a), "<detached>");
    }

    #[test]
    fn long_quote_survives_scattered_edits() {
        let para = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.";
        let text = format!("{}\n\n{para}\n\n{}", "filler text. ".repeat(500), "more filler. ".repeat(500));
        let a = anchor_on(&text, para);
        let changed = para.replace("dolor sit", "dolour sit").replace("magna", "MAGNA").replace("nisi ut", "nisi");
        let edited = text.replace(para, &changed);
        assert_eq!(attached(&edited, &a), changed.leak() as &str);
    }

    #[test]
    fn empty_quote_needs_exact_context() {
        let text = "alpha beta\ngamma\n";
        let at = text.find("beta").unwrap();
        let a = make_anchor(&RopeBuffer::from_text(text), at..at);
        assert_eq!(a.quote, "");
        let edited = format!("zero\n{text}");
        assert_eq!(reanchor(&a, &RopeBuffer::from_text(&edited)), Resolution::Attached(at + 5..at + 5));
        assert_eq!(reanchor(&a, &RopeBuffer::from_text("alpha gamma\n")), Resolution::Detached);
    }

    #[test]
    fn levenshtein_basics() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(levenshtein(&c("kitten"), &c("sitting")), 3);
        assert_eq!(levenshtein(&c(""), &c("abc")), 3);
        assert_eq!(levenshtein(&c("–"), &c("-")), 1);
    }
}
