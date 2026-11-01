//! Property test: any sequence of edits leaves the incremental block index
//! equal to a full reparse of the resulting text (ARCHITECTURE 11).

use mdedit_core::parse::BlockIndex;
use mdedit_core::{Edit, EditOrigin, RopeBuffer, TextBuffer, TxnId};
use proptest::prelude::*;

/// Fragments that exercise block boundaries: fences, HTML, lists, quotes,
/// setext underlines, tables, link reference definitions, blank lines.
const FRAGMENTS: &[&str] = &[
    "\n",
    "\n\n",
    "para",
    "more text",
    "# h",
    "## h2 {#id}",
    "===",
    "---",
    "- item",
    "* item",
    "1. one",
    "2) two",
    "> quote",
    ">",
    "```",
    "```rust",
    "~~~",
    "    code",
    "<div>",
    "</div>",
    "<!--",
    "-->",
    "<pre>",
    "</pre>",
    "| a | b |",
    "|---|---|",
    "[x]: /u",
    "[x]: /v \"t\"",
    "[Y]:\n/w",
    "[^1]: note",
    "  ",
    "\t",
    "*em*",
    "`c`",
    "[x]",
    "é",
    "\r\n",
    "+ ",
    "<script>",
    "</script>",
    "***",
];

fn fragment() -> impl Strategy<Value = String> {
    prop::sample::select(FRAGMENTS).prop_map(str::to_string)
}

fn document() -> impl Strategy<Value = String> {
    prop::collection::vec(fragment(), 0..60).prop_map(|parts| parts.concat())
}

/// An edit as fractions of the current length, resolved against the text
/// when applied.
#[derive(Debug, Clone)]
struct EditSpec {
    at: f64,
    len: f64,
    text: String,
}

fn edit_spec() -> impl Strategy<Value = EditSpec> {
    (0.0..=1.0f64, prop_oneof![3 => Just(0.0), 1 => 0.0..0.3f64], prop::collection::vec(fragment(), 0..3))
        .prop_map(|(at, len, parts)| EditSpec { at, len, text: parts.concat() })
}

fn floor_char(text: &str, mut i: usize) -> usize {
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn resolve(spec: &EditSpec, text: &str) -> Edit {
    let n = text.len();
    let start = floor_char(text, (spec.at * n as f64) as usize);
    let end = floor_char(text, (start + (spec.len * n as f64) as usize).min(n)).max(start);
    Edit { range: start..end, text: spec.text.clone(), origin: EditOrigin::User, txn: TxnId(0) }
}

fn check(index: &BlockIndex, buf: &RopeBuffer) -> Result<(), TestCaseError> {
    let full = BlockIndex::parse(buf);
    let got: Vec<_> = index.blocks().iter().map(|b| (b.kind, b.bytes)).collect();
    let want: Vec<_> = full.blocks().iter().map(|b| (b.kind, b.bytes)).collect();
    if got != want {
        let i = got.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
        let lo = i.saturating_sub(2);
        let text = buf.text();
        let from = want.get(lo).map_or(0, |b| b.1.start);
        let to = want.get(i + 2).map_or(text.len(), |b| b.1.end);
        prop_assert!(
            false,
            "first difference at block {}: got {:?}, want {:?}, text around: {:?}",
            i,
            &got[lo..(i + 3).min(got.len())],
            &want[lo..(i + 3).min(want.len())],
            &text[from..to]
        );
    }
    let mut got: Vec<_> = index.link_refs().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let mut want: Vec<_> = full.link_refs().map(|(k, v)| (k.to_string(), v.clone())).collect();
    got.sort_by(|a, b| a.0.cmp(&b.0));
    want.sort_by(|a, b| a.0.cmp(&b.0));
    prop_assert_eq!(got, want, "link refs, text {:?}", buf.text());
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn random_edits_match_full_reparse(doc in document(), edits in prop::collection::vec(edit_spec(), 1..12)) {
        let mut buf = RopeBuffer::from_text(&doc);
        let mut index = BlockIndex::parse(&buf);
        for spec in &edits {
            let edit = resolve(spec, &buf.text());
            let applied = buf.apply(&edit);
            index.on_edit(&applied, &buf);
            check(&index, &buf)?;
        }
    }

    /// With a tiny offload threshold most edits go through the pending path:
    /// several edits pile up, then a job or a flush catches up.
    #[test]
    fn deferred_reparse_catches_up(doc in document(), edits in prop::collection::vec(edit_spec(), 1..12), flush_every in 1..4usize) {
        let mut buf = RopeBuffer::from_text(&doc);
        let mut index = BlockIndex::parse(&buf);
        index.set_offload_threshold(Some(8));
        for (i, spec) in edits.iter().enumerate() {
            let edit = resolve(spec, &buf.text());
            let applied = buf.apply(&edit);
            index.on_edit(&applied, &buf);
            if i % flush_every == 0 {
                if let Some(job) = index.take_parse_job(&buf) {
                    prop_assert!(index.finish_parse(job.run(), &buf));
                }
                prop_assert!(!index.is_pending());
                check(&index, &buf)?;
            }
        }
        index.flush(&buf);
        check(&index, &buf)?;
    }

    /// Documents with hundreds of blocks, so splices cross leaves.
    #[test]
    fn many_blocks(n in 100..400usize, edits in prop::collection::vec(edit_spec(), 1..20)) {
        let doc: String = (0..n).map(|i| match i % 5 {
            0 => format!("# h{i}\n\n"),
            1 => format!("p{i}\n\n"),
            2 => format!("- l{i}\n\n"),
            3 => format!("[r{i}]: /x{i}\n\n"),
            _ => format!("> q{i}\n\n"),
        }).collect();
        let mut buf = RopeBuffer::from_text(&doc);
        let mut index = BlockIndex::parse(&buf);
        for spec in &edits {
            let edit = resolve(spec, &buf.text());
            let applied = buf.apply(&edit);
            index.on_edit(&applied, &buf);
            index.flush(&buf);
            check(&index, &buf)?;
        }
    }
}

/// Replays an edit sequence (as `EditSpec` fractions), checking after each
/// edit; `deferred` takes the parse-job path with a tiny threshold.
fn replay(doc: &str, edits: &[(f64, f64, &str)], deferred: bool) -> Result<(), TestCaseError> {
    let mut buf = RopeBuffer::from_text(doc);
    let mut index = BlockIndex::parse(&buf);
    if deferred {
        index.set_offload_threshold(Some(8));
    }
    for &(at, len, text) in edits {
        let edit = resolve(&EditSpec { at, len, text: text.into() }, &buf.text());
        let applied = buf.apply(&edit);
        index.on_edit(&applied, &buf);
        if let Some(job) = index.take_parse_job(&buf) {
            prop_assert!(index.finish_parse(job.run(), &buf));
        }
        check(&index, &buf)?;
    }
    Ok(())
}

/// CRLF plus a lazy blockquote continuation: a deferred reparse started
/// after the touched HTML block instead of before it.
#[test]
fn regression_deferred_lazy_quote_crlf() {
    let doc = "é===> quote| a | b |===more text| a | b |  \r\n</div>\n\n===>  \n\n\n\n\n- itempara\n    code```| a | b |## h2 {#id}## h2 {#id}\n- item## h2 {#id}- item\n## h2 {#id}\n| a | b |\n\npara[x]: /v \"t\"\n";
    let edits = [
        (0.9734472141394692, 0.0, "more textpara"),
        (0.6991388199458342, 0.24733095121594173, "| a | b |"),
        (0.58422769757525, 0.0, "para"),
        (0.0, 0.03237789354992918, ""),
        (0.44291995044244137, 0.0, "```rustpara"),
        (0.8681122476827561, 0.0, "\n"),
        (0.2525601165566407, 0.1751503036583719, ""),
    ];
    replay(doc, &edits, true).unwrap();
}

/// A tab-indented line after a footnote definition continues it; a window
/// starting on that line parsed it as indented code.
#[test]
fn regression_footnote_continuation() {
    replay("| a | b |\r\n[^1]: note\n\t[^1]: note\n# h", &[(1.0, 0.0, "")], false).unwrap();
    replay("| a | b |\r\n[^1]: note\n\t[^1]: note\n# h", &[(0.9, 0.0, "")], false).unwrap();
}

/// Two blocks starting on one line, and a duplicate link definition that
/// pulldown-cmark hid until the first one stopped being a definition.
#[test]
fn regression_shared_line_and_duplicate_definition() {
    replay("[^1]:[^1]: note note\n\npara\n", &[(0.5, 0.0, "x"), (0.1, 0.0, "")], false).unwrap();
    replay(">===\n\n\n\n[x]: /u\n# h\n    code\n[x]: /u  \n\n\n", &[(0.2066798255696416, 0.0, "para")], false).unwrap();
    replay(">===\n\n\n\n[x]: /u\n# h\n    code\n[x]: /u  \n\n\n", &[(0.2066798255696416, 0.0, "para")], true).unwrap();
}

/// An indented line after a blank line still continues a footnote.
#[test]
fn regression_footnote_continuation_after_blank() {
    let doc = "[^1]: note\n\n\t[^1]: note\n\nparapara</pre>para\n";
    for at in [0.0, 0.3, 0.5, 0.6, 0.9, 1.0] {
        replay(doc, &[(at, 0.0, "")], false).unwrap();
        replay(doc, &[(at, 0.0, "x")], false).unwrap();
    }
}

/// CRLF after an opening fence, with a deferred reparse.
#[test]
fn regression_fence_crlf_deferred() {
    let doc = "  [Y]:\n/w```\r\n| a | b |- item\n| a | b |para- itempara\n\n- item| a | b |para- item| a | b |- item- item\npara| a | b |- item[x]: /v \"t\"\n\npara";
    replay(doc, &[(0.0525, 0.189, "")], false).unwrap();
    replay(doc, &[(0.0525, 0.189, "")], true).unwrap();
}
