//! Link destinations in markdown source, with their exact byte ranges, so a
//! move can rewrite paths without touching anything else (SPEC 6.4).
//!
//! Covers inline links and images (`[t](dest)`, `![a](dest "title")`) and
//! reference definitions (`[ref]: dest`). Autolinks and raw HTML (`<img src>`)
//! are not rewritten.

use std::ops::Range;

use mdedit_core::parse::markdown_options;
use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkDest {
    /// Byte range of the destination as written, without `<>`.
    pub range: Range<usize>,
    /// Written as `<dest>`.
    pub angle: bool,
    pub is_image: bool,
}

struct Open {
    is_image: bool,
    inline: bool,
    /// Where the link text ends at the earliest (end of the last child).
    text_end: usize,
}

/// Every rewritable link destination in `text`, in source order.
pub fn find_link_dests(text: &str) -> Vec<LinkDest> {
    let mut out = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut iter = Parser::new_ext(text, markdown_options()).into_offset_iter();
    for (event, range) in iter.by_ref() {
        match event {
            Event::Start(Tag::Link { link_type, .. }) | Event::Start(Tag::Image { link_type, .. }) => {
                if let Some(top) = stack.last_mut() {
                    top.text_end = top.text_end.max(range.end);
                }
                let is_image = text[range.start..].starts_with('!');
                let text_start = range.start + if is_image { 2 } else { 1 };
                stack.push(Open { is_image, inline: link_type == LinkType::Inline, text_end: text_start });
            }
            Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => {
                if let Some(open) = stack.pop()
                    && open.inline
                    && let Some(rel) = text[open.text_end.min(range.end)..range.end].find("](")
                {
                    let at = open.text_end + rel + 2;
                    if let Some((r, angle)) = parse_dest(text, at, range.end) {
                        out.push(LinkDest { range: r, angle, is_image: open.is_image });
                    }
                }
                if let Some(top) = stack.last_mut() {
                    top.text_end = top.text_end.max(range.end);
                }
            }
            _ => {
                if let Some(top) = stack.last_mut() {
                    top.text_end = top.text_end.max(range.end);
                }
            }
        }
    }
    for (_, def) in iter.reference_definitions().iter() {
        if let Some(colon) = label_end(text, def.span.clone())
            && let Some((r, angle)) = parse_dest(text, colon + 1, def.span.end)
        {
            out.push(LinkDest { range: r, angle, is_image: false });
        }
    }
    out.sort_by_key(|d| d.range.start);
    out.dedup();
    out
}

/// Index of the `:` after `[label]` in a reference definition span.
fn label_end(text: &str, span: Range<usize>) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = span.start + text[span.clone()].find('[')? + 1;
    while i < span.end {
        match bytes[i] {
            b'\\' => i += 2,
            b']' => return (bytes.get(i + 1) == Some(&b':')).then_some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Parse a link destination starting at `i` (whitespace allowed before it).
fn parse_dest(text: &str, mut i: usize, end: usize) -> Option<(Range<usize>, bool)> {
    let bytes = text.as_bytes();
    while i < end && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    if i < end && bytes[i] == b'<' {
        let mut j = i + 1;
        while j < end {
            match bytes[j] {
                b'\\' => j += 2,
                b'>' => return Some((i + 1..j, true)),
                b'\n' | b'<' => return None,
                _ => j += 1,
            }
        }
        return None;
    }
    let mut depth = 0usize;
    let mut j = i;
    while j < end {
        match bytes[j] {
            b'\\' if j + 1 < end => j += 2,
            b if b <= b' ' => break,
            b'(' => {
                depth += 1;
                j += 1;
            }
            b')' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                j += 1;
            }
            _ => j += 1,
        }
    }
    let j = j.min(end);
    (j > i).then_some((i..j, false))
}

/// A destination that names a local relative path: the decoded path and the
/// untouched `?query` / `#fragment` suffix. `None` for URLs with a scheme,
/// absolute paths and pure fragments.
pub fn local_target(raw: &str) -> Option<(String, String)> {
    let unescaped = unescape(raw);
    let split = unescaped.find(['#', '?']).unwrap_or(unescaped.len());
    let (path, suffix) = unescaped.split_at(split);
    if path.is_empty() || path.starts_with(['/', '\\']) {
        return None;
    }
    if let Some(colon) = path.find(':')
        && !path[..colon].contains('/')
    {
        return None; // scheme (http:, mailto:) or drive letter
    }
    Some((percent_decode(path)?, suffix.to_owned()))
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&n) = chars.peek()
            && n.is_ascii_punctuation()
        {
            out.push(n);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Encode a `/`-separated relative path as a link destination. Raw form
/// percent-encodes what would end or break the destination; angle form only
/// what `<>` cannot hold.
pub fn encode_dest(path: &str, angle: bool) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '%' | '<' | '>' | '#' | '?' | '\\' => out.push_str(&format!("%{:02X}", c as u32)),
            ' ' | '(' | ')' if !angle => out.push_str(&format!("%{:02X}", c as u32)),
            c if c.is_control() => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dests(text: &str) -> Vec<&str> {
        find_link_dests(text).iter().map(|d| &text[d.range.clone()]).collect()
    }

    #[test]
    fn finds_inline_links_images_and_definitions() {
        let text = "# T\n\n![a](assets/x.png) and [b *c*](other.md#s \"t\") and [r][1].\n\n\
                    [`]`](code.md) [![img](in.png)](out.md) <https://a.b> [e]()\n\n[1]: <ref target.md> \"x\"\n";
        assert_eq!(dests(text), ["assets/x.png", "other.md#s", "code.md", "in.png", "out.md", "ref target.md"]);
        let d = find_link_dests(text);
        assert!(d[0].is_image && !d[1].is_image && d[5].angle);
    }

    #[test]
    fn balanced_parens_and_escapes() {
        assert_eq!(dests("[a](x(1).md) [b](y\\).md)"), ["x(1).md", "y\\).md"]);
        assert_eq!(dests("[a](\n  spaced.md\n)"), ["spaced.md"]);
    }

    #[test]
    fn ignores_code_and_non_links() {
        assert!(dests("`[a](x.md)`\n\n    [b](y.md)\n").is_empty());
    }

    #[test]
    fn local_targets() {
        assert_eq!(local_target("assets/my%20pic.png"), Some(("assets/my pic.png".into(), String::new())));
        assert_eq!(local_target("a.md#sec?x"), Some(("a.md".into(), "#sec?x".into())));
        assert_eq!(local_target("x\\(1\\).md"), Some(("x(1).md".into(), String::new())));
        for raw in ["https://x.y/a.png", "mailto:a@b", "/abs.png", "#frag", "C:/x.png"] {
            assert_eq!(local_target(raw), None, "{raw}");
        }
    }

    #[test]
    fn encoding() {
        assert_eq!(encode_dest("../a b/(1)%.png", false), "../a%20b/%281%29%25.png");
        assert_eq!(encode_dest("../a b/(1).png", true), "../a b/(1).png");
        assert_eq!(encode_dest("caffè.png", false), "caffè.png");
    }
}
