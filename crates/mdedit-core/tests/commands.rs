//! Fixture tests for the document commands (DESIGN_SYSTEM 8.2-8.4).
//!
//! Fixtures mark the selection inline: `¦` is a cursor, `‹` the anchor and
//! `›` the head of a selection (`›…‹` is a backwards selection).

use mdedit_core::command::{self, ColumnAlign, CommandResult, DocCommand, TableOp};
use mdedit_core::{Document, EditOrigin, Selection};

fn parse(fixture: &str) -> (String, Selection) {
    let mut text = String::new();
    let (mut anchor, mut head) = (None, None);
    for c in fixture.chars() {
        match c {
            '¦' => {
                anchor = Some(text.len());
                head = Some(text.len());
            }
            '‹' => anchor = Some(text.len()),
            '›' => head = Some(text.len()),
            c => text.push(c),
        }
    }
    let anchor = anchor.expect("fixture has a selection");
    (text, Selection { anchor, head: head.unwrap_or(anchor) })
}

fn render(text: &str, sel: Selection) -> String {
    let mut marks = if sel.is_empty() {
        vec![(sel.head, '¦')]
    } else if sel.anchor < sel.head {
        vec![(sel.anchor, '‹'), (sel.head, '›')]
    } else {
        vec![(sel.head, '›'), (sel.anchor, '‹')]
    };
    marks.sort_by_key(|m| std::cmp::Reverse(m.0));
    let mut out = text.to_string();
    for (at, c) in marks {
        out.insert(at, c);
    }
    out
}

fn apply(doc: &mut Document, result: CommandResult) -> String {
    let txn = doc.next_txn();
    for edit in &result.edits {
        assert_eq!(edit.origin, EditOrigin::Command);
        doc.apply(&mdedit_core::Edit { txn, ..edit.clone() });
    }
    render(&doc.buffer().text(), result.selection)
}

fn run(cmd: DocCommand, fixture: &str) -> String {
    let (text, sel) = parse(fixture);
    let mut doc = Document::from_text(&text);
    let result = command::run(&doc, sel, &cmd);
    apply(&mut doc, result)
}

#[track_caller]
fn check(cmd: DocCommand, before: &str, after: &str) {
    assert_eq!(run(cmd, before), after, "command on {before:?}");
}

/// Runs the command, then runs it again on the result and expects the
/// original back.
#[track_caller]
fn check_toggle(cmd: DocCommand, before: &str, after: &str) {
    check(cmd.clone(), before, after);
    check(cmd, after, before);
}

#[track_caller]
fn check_input(input: &str, before: &str, after: Option<&str>) {
    let (text, sel) = parse(before);
    let mut doc = Document::from_text(&text);
    let got = command::smart_input(&doc, sel, input).map(|r| apply(&mut doc, r));
    assert_eq!(got.as_deref(), after, "typing {input:?} on {before:?}");
}

// ---- registry ----

#[test]
fn registry_ids_round_trip() {
    let reg = command::registry();
    assert!(reg.len() >= 20);
    for meta in reg {
        let cmd = DocCommand::from_id(meta.id.0).unwrap_or_else(|| panic!("{} has no command", meta.id.0));
        assert_eq!(cmd.id(), Some(meta.id), "{}", meta.id.0);
        assert_eq!(meta.command(), cmd);
    }
    let mut ids: Vec<_> = reg.iter().map(|m| m.id.0).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), reg.len(), "duplicate ids");
    assert_eq!(DocCommand::from_id("format.bold"), Some(DocCommand::ToggleBold));
    assert_eq!(DocCommand::from_id("nope"), None);
    assert_eq!(DocCommand::Link { url: Some("x".into()) }.id(), None);
}

#[test]
fn registry_shortcuts_match_design_system() {
    let shortcut = |id: &str| command::registry().iter().find(|m| m.id.0 == id).and_then(|m| m.shortcut);
    assert_eq!(shortcut("format.bold"), Some("Ctrl+B"));
    assert_eq!(shortcut("format.strike"), Some("Ctrl+Shift+X"));
    assert_eq!(shortcut("format.h3"), Some("Ctrl+Alt+3"));
    assert_eq!(shortcut("format.task_list"), Some("Ctrl+Shift+9"));
    assert_eq!(shortcut("insert.table"), Some("Ctrl+Alt+T"));
    assert_eq!(shortcut("edit.paste_table"), Some("Ctrl+Shift+V"));
    let mut keys: Vec<_> = command::registry().iter().filter_map(|m| m.shortcut).collect();
    keys.sort();
    let n = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), n, "duplicate shortcuts");
    assert!(!keys.contains(&"Ctrl+Shift+B"), "Ctrl+Shift+B is the sidebar toggle");
    let slash = |id: &str| command::registry().iter().find(|m| m.id.0 == id).and_then(|m| m.slash);
    assert_eq!(slash("format.h1"), Some("h1"));
    assert_eq!(slash("format.task_list"), Some("todo"));
    assert_eq!(slash("insert.hr"), Some("hr"));
}

// ---- inline toggles ----

#[test]
fn bold_wraps_and_unwraps_selection() {
    check_toggle(DocCommand::ToggleBold, "say ‹hello› world", "say **‹hello›** world");
    check(DocCommand::ToggleBold, "say ‹**hello**› world", "say ‹hello› world");
    check(DocCommand::ToggleBold, "say __‹hello›__ world", "say ‹hello› world");
    // Surrounding whitespace stays outside the markers.
    check(DocCommand::ToggleBold, "say‹ hello ›world", "say‹ **hello** ›world");
}

#[test]
fn bold_at_cursor() {
    check_toggle(DocCommand::ToggleBold, "say hel¦lo now", "say **hel¦lo** now");
    check(DocCommand::ToggleBold, "say ¦hello", "say **¦hello**");
    check(DocCommand::ToggleBold, "**a b¦ c**", "a b¦ c");
    check_toggle(DocCommand::ToggleBold, "x ¦", "x **¦**");
}

#[test]
fn italic_and_bold_nest() {
    check_toggle(DocCommand::ToggleItalic, "a ‹b› c", "a *‹b›* c");
    check(DocCommand::ToggleItalic, "a **‹b›** c", "a ***‹b›*** c");
    check(DocCommand::ToggleItalic, "a ***‹b›*** c", "a **‹b›** c");
    check(DocCommand::ToggleBold, "a ***‹b›*** c", "a *‹b›* c");
    check(DocCommand::ToggleItalic, "a _‹b›_ c", "a ‹b› c");
}

#[test]
fn strike_and_code() {
    check_toggle(DocCommand::ToggleStrike, "‹gone›", "~~‹gone›~~");
    check_toggle(DocCommand::ToggleInlineCode, "run ‹ls -l› now", "run `‹ls -l›` now");
    check(DocCommand::ToggleInlineCode, "‹a`b›", "``‹a`b›``");
    check(DocCommand::ToggleInlineCode, "‹`x›", "`` ‹`x› ``");
    check(DocCommand::ToggleInlineCode, "`` ‹`x› ``", "‹`x›");
}

#[test]
fn inline_toggle_multi_line_wraps_each_line() {
    check_toggle(DocCommand::ToggleBold, "- ‹one\n- two›\n", "- **‹one**\n- **two›**\n");
    check(DocCommand::ToggleBold, "‹a\n\nb›", "**‹a**\n\n**b›**");
}

#[test]
fn inline_toggle_in_heading_and_quote() {
    check_toggle(DocCommand::ToggleItalic, "# Ti¦tle", "# *Ti¦tle*");
    check_toggle(DocCommand::ToggleBold, "> ‹quoted text›", "> **‹quoted text›**");
}

#[test]
fn link_wraps_selection_and_unlinks() {
    check(DocCommand::Link { url: None }, "see ‹docs› here", "see [docs](¦) here");
    check(DocCommand::Link { url: Some("https://x.org".into()) }, "see ‹docs› here", "see [docs](https://x.org)¦ here");
    check(DocCommand::Link { url: None }, "see [do¦cs](https://x.org) here", "see do¦cs here");
    check(DocCommand::Link { url: None }, "‹https://x.org›", "[¦](https://x.org)");
    check(DocCommand::Link { url: None }, "go ¦", "go [¦]()");
    check(DocCommand::Link { url: None }, "wo¦rd", "[word](¦)");
    check(DocCommand::Link { url: Some("a b.md".into()) }, "‹f›", "[f](<a b.md>)¦");
}

#[test]
fn image_inserts() {
    check(DocCommand::Image { url: None }, "¦", "![](¦)");
    check(DocCommand::Image { url: None }, "‹alt text›", "![alt text](¦)");
    check(DocCommand::Image { url: Some("assets/a.png".into()) }, "‹cat›", "![cat](assets/a.png)¦");
}

#[test]
fn smart_paste_url_makes_link() {
    let (text, sel) = parse("read ‹this› now");
    let mut doc = Document::from_text(&text);
    let r = command::smart_paste(&doc, sel, "https://example.com").unwrap();
    assert_eq!(apply(&mut doc, r), "read [this](https://example.com)¦ now");
    let (text, sel) = parse("read ‹this› now");
    let doc = Document::from_text(&text);
    assert!(command::smart_paste(&doc, sel, "not a url").is_none());
    assert!(command::smart_paste(&doc, Selection::cursor(2), "https://x.org").is_none());
}

// ---- headings ----

#[test]
fn headings_set_switch_and_toggle() {
    check_toggle(DocCommand::SetHeading(2), "Ti¦tle", "## Ti¦tle");
    check(DocCommand::SetHeading(3), "# Ti¦tle", "### Ti¦tle");
    check(DocCommand::SetHeading(0), "#### Ti¦tle ##", "Ti¦tle");
    check(DocCommand::SetHeading(1), "- it¦em", "# it¦em");
    check(DocCommand::SetHeading(1), "> quo¦te", "> # quo¦te");
    check(DocCommand::SetHeading(2), "¦", "## ¦");
    check(DocCommand::SetHeading(1), "‹a\nb›", "# ‹a\n# b›");
}

#[test]
fn setext_headings_convert() {
    check(DocCommand::SetHeading(3), "Ti¦tle\n=====\n\nnext", "### Ti¦tle\n\nnext");
    check(DocCommand::SetHeading(2), "Ti¦tle\n-----\n", "Ti¦tle\n");
}

// ---- lists, quotes, code blocks, rules ----

#[test]
fn bullet_list_toggles() {
    check_toggle(DocCommand::BulletList, "‹one\ntwo›", "- ‹one\n- two›");
    check(DocCommand::BulletList, "1. o¦ne", "- o¦ne");
    check(DocCommand::BulletList, "- [x] o¦ne", "- o¦ne");
    check(DocCommand::BulletList, "‹a\n\nb›", "- ‹a\n\n- b›");
    check_toggle(DocCommand::BulletList, "¦", "- ¦");
    check(DocCommand::BulletList, "# He¦ad", "- He¦ad");
}

#[test]
fn numbered_list_numbers_per_level() {
    check(DocCommand::NumberedList, "‹a\nb\n  c\n  d\ne›", "1. ‹a\n2. b\n  1. c\n  2. d\n3. e›");
    check(DocCommand::NumberedList, "1. ‹a\n2. b›", "‹a\nb›");
    check(DocCommand::NumberedList, "- ‹a\n- b›", "1. ‹a\n2. b›");
}

#[test]
fn task_list_toggles_and_keeps_checked() {
    check_toggle(DocCommand::TaskList, "bu¦y milk", "- [ ] bu¦y milk");
    check(DocCommand::TaskList, "‹- [x] done\nnew›", "- [x] ‹done\n- [ ] new›");
    check(DocCommand::TaskList, "- [x] do¦ne", "do¦ne");
}

#[test]
fn quote_toggles() {
    check_toggle(DocCommand::Quote, "‹a\n\nb›", "> ‹a\n>\n> b›");
    check(DocCommand::Quote, "> > ne¦st", "> ne¦st");
    check_toggle(DocCommand::Quote, "¦", "> ¦");
}

#[test]
fn code_block_wraps_and_unwraps() {
    check_toggle(DocCommand::CodeBlock, "‹let a = 1;\nlet b = 2;›", "```\n‹let a = 1;\nlet b = 2;›\n```");
    check(DocCommand::CodeBlock, "text\n\n¦\n\nmore", "text\n\n```\n¦\n```\n\nmore");
    check(DocCommand::CodeBlock, "```rust\nfn ¦main() {}\n```\n", "fn ¦main() {}\n");
    check(DocCommand::CodeBlock, "‹```x```›", "````\n‹```x```›\n````");
}

#[test]
fn horizontal_rule_is_its_own_block() {
    check(DocCommand::HorizontalRule, "a¦\nb", "a\n\n---\n¦\nb");
    check(DocCommand::HorizontalRule, "a¦", "a\n\n---\n¦");
    check(DocCommand::HorizontalRule, "a\n\n¦\n\nb", "a\n\n---\n¦\nb");
    check(DocCommand::HorizontalRule, "a\n¦", "a\n\n---\n¦");
}

// ---- smart editing ----

#[test]
fn enter_continues_lists() {
    check_input("\n", "- one¦", Some("- one\n- ¦"));
    check_input("\n", "* one¦", Some("* one\n* ¦"));
    check_input("\n", "9. one¦", Some("9. one\n10. ¦"));
    check_input("\n", "3) one¦", Some("3) one\n4) ¦"));
    check_input("\n", "- [x] one¦", Some("- [x] one\n- [ ] ¦"));
    check_input("\n", "  - nested¦", Some("  - nested\n  - ¦"));
    check_input("\n", "- sp¦lit", Some("- sp\n- ¦lit"));
    check_input("\n", "- ‹sel›ected", Some("- \n- ¦ected"));
    check_input("\n", "> - in quote¦", Some("> - in quote\n> - ¦"));
}

#[test]
fn enter_on_empty_item_ends_list() {
    check_input("\n", "- one\n- ¦", Some("- one\n¦"));
    check_input("\n", "- one\n- [ ] ¦", Some("- one\n¦"));
    check_input("\n", "- one\n  - ¦", Some("- one\n- ¦"));
    check_input("\n", "> - one\n> - ¦", Some("> - one\n> ¦"));
}

#[test]
fn enter_continues_and_ends_quotes() {
    check_input("\n", "> quote¦", Some("> quote\n> ¦"));
    check_input("\n", "> quote\n> ¦", Some("> quote\n¦"));
}

#[test]
fn enter_elsewhere_is_plain() {
    check_input("\n", "plain¦", None);
    check_input("\n", "# head¦", None);
    check_input("\n", "¦- item", None);
}

#[test]
fn tab_indents_list_items() {
    check_input("\t", "- a\n- b¦", Some("- a\n  - b¦"));
    check_input("\t", "1. a\n2. b¦", Some("1. a\n   1. b¦"));
    check_input("\t", "- a\n‹- b\n- c›", Some("- a\n  ‹- b\n  - c›"));
    check_input("\t", "plain¦", None);
}

#[test]
fn shift_tab_outdents_list_items() {
    check(DocCommand::Outdent, "- a\n  - b¦", "- a\n- b¦");
    check(DocCommand::Outdent, "1. a\n   1. b¦", "1. a\n2. b¦");
    check(DocCommand::Outdent, "- a\n  - b\n    - c¦", "- a\n  - b\n  - c¦");
    check(DocCommand::Outdent, "- a¦", "- a¦");
    check(DocCommand::Indent, "- a\n- b¦", "- a\n  - b¦");
}

#[test]
fn typing_pair_wraps_selection() {
    check_input("*", "a ‹word› b", Some("a *‹word›* b"));
    check_input("**", "a ‹word› b", Some("a **‹word›** b"));
    check_input("`", "‹code›", Some("`‹code›`"));
    check_input("[", "‹link›", Some("[‹link›]"));
    check_input("_", "›x‹", Some("_›x‹_"));
    check_input("*", "no¦ selection", None);
    check_input("x", "‹word›", None);
}

// ---- tables ----

const TABLE: &str = "| Name | Qty |\n|:--|--:|\n| apple | 3 |\n| fig | 12 |";

#[test]
fn table_auto_align() {
    check(
        DocCommand::Table(TableOp::Format),
        "|Name|Qty|\n|:--|--:|\n|ap¦ple|3|\n|fig|12|",
        "| Name  | Qty |\n| :---- | --: |\n| ap¦ple |   3 |\n| fig   |  12 |",
    );
    // Already aligned: nothing to do.
    let aligned = "| Name  | Qty |\n| :---- | --: |\n| ap¦ple |   3 |\n| fig   |  12 |";
    let (text, sel) = parse(aligned);
    let doc = Document::from_text(&text);
    assert!(command::run(&doc, sel, &DocCommand::Table(TableOp::Format)).edits.is_empty());
}

#[test]
fn table_align_counts_wide_characters_twice() {
    check(
        DocCommand::Table(TableOp::Format),
        "| a | b |\n|---|---|\n| 日本語 | é¦ |\n| x | 👍🏽 |",
        "| a      | b   |\n| ------ | --- |\n| 日本語 | é¦   |\n| x      | 👍🏽  |",
    );
}

#[test]
fn table_align_center_ragged_rows_escapes_and_quotes() {
    check(
        DocCommand::Table(TableOp::Format),
        "| a | b |\n|:-:|-|\n| long cell¦ |\n| x \\| y | z | extra |",
        "|     a     | b   |       |\n| :-------: | --- | ----- |\n| long cell¦ |     |       |\n|  x \\| y   | z   | extra |",
    );
    check(
        DocCommand::Table(TableOp::Format),
        "> |a|b|\n> |-|-|\n> |1¦|2|",
        "> | a   | b   |\n> | --- | --- |\n> | 1¦   | 2   |",
    );
}

#[test]
fn ctrl_alt_t_inside_table_aligns() {
    check(
        DocCommand::InsertTable { rows: 3, cols: 3 },
        "|a|b|\n|-|-|\n|1|2¦|",
        "| a   | b   |\n| --- | --- |\n| 1   | 2¦   |",
    );
}

#[test]
fn insert_table() {
    check(
        DocCommand::InsertTable { rows: 3, cols: 2 },
        "Intro¦",
        "Intro\n\n| ¦    |     |\n| --- | --- |\n|     |     |\n|     |     |\n",
    );
    check(DocCommand::InsertTable { rows: 1, cols: 1 }, "¦", "| ¦    |\n| --- |\n");
}

#[test]
fn table_cell_navigation() {
    check_input("\t", "| a¦ | b |\n|---|---|\n| 1 | 2 |", Some("| a   | ‹b›   |\n| --- | --- |\n| 1   | 2   |"));
    check_input("\t", "| a | b¦ |\n|---|---|\n| 1 | 2 |", Some("| a   | b   |\n| --- | --- |\n| ‹1›   | 2   |"));
    check_input(
        "\t",
        "| a | b |\n|---|---|\n| 1 | 2¦ |",
        Some("| a   | b   |\n| --- | --- |\n| 1   | 2   |\n| ¦    |     |"),
    );
    check(DocCommand::Outdent, "| a | b |\n|---|---|\n| 1¦ | 2 |", "| a   | ‹b›   |\n| --- | --- |\n| 1   | 2   |");
    check(DocCommand::Outdent, "| a¦ | b |\n|---|---|", "| ‹a›   | b   |\n| --- | --- |");
}

#[test]
fn table_row_operations() {
    let t = "| h1 | h2 |\n|---|---|\n| a¦ | b |\n| c | d |";
    check(DocCommand::Table(TableOp::InsertRowAbove), t, "| h1  | h2  |\n| --- | --- |\n| ¦    |     |\n| a   | b   |\n| c   | d   |");
    check(DocCommand::Table(TableOp::InsertRowBelow), t, "| h1  | h2  |\n| --- | --- |\n| a   | b   |\n| ¦    |     |\n| c   | d   |");
    check(DocCommand::Table(TableOp::DeleteRow), t, "| h1  | h2  |\n| --- | --- |\n| ¦c   | d   |");
    check(DocCommand::Table(TableOp::MoveRowDown), t, "| h1  | h2  |\n| --- | --- |\n| c   | d   |\n| a¦   | b   |");
    check(DocCommand::Table(TableOp::MoveRowUp), t, t);
    let header = "| h¦1 | h2 |\n|---|---|\n| a | b |";
    check(DocCommand::Table(TableOp::DeleteRow), header, header);
    check(DocCommand::Table(TableOp::InsertRowAbove), header, "| h1  | h2  |\n| --- | --- |\n| ¦    |     |\n| a   | b   |");
}

#[test]
fn table_column_operations() {
    let t = "| h1 | h2 |\n|:--|---|\n| a¦ | b |";
    check(DocCommand::Table(TableOp::InsertColumnLeft), t, "|     | h1  | h2  |\n| --- | :-- | --- |\n| ¦    | a   | b   |");
    check(DocCommand::Table(TableOp::InsertColumnRight), t, "| h1  |     | h2  |\n| :-- | --- | --- |\n| a   | ¦    | b   |");
    check(DocCommand::Table(TableOp::DeleteColumn), t, "| h2  |\n| --- |\n| ¦b   |");
    check(DocCommand::Table(TableOp::MoveColumnRight), t, "| h2  | h1  |\n| --- | :-- |\n| b   | a¦   |");
    // Already leftmost: nothing to do.
    check(DocCommand::Table(TableOp::MoveColumnLeft), t, t);
    check(DocCommand::Table(TableOp::Align(ColumnAlign::Right)), t, "|  h1 | h2  |\n| --: | --- |\n|   a¦ | b   |");
    check(DocCommand::Table(TableOp::Align(ColumnAlign::Center)), t, "| h1  | h2  |\n| :-: | --- |\n|  a¦  | b   |");
    let one = "| h¦ |\n|---|";
    check(DocCommand::Table(TableOp::DeleteColumn), one, one);
}

#[test]
fn table_ops_outside_table_do_nothing() {
    check(DocCommand::Table(TableOp::InsertRowBelow), "plain ¦text", "plain ¦text");
    check(DocCommand::Table(TableOp::Format), "a | b¦", "a | b¦");
}

#[test]
fn table_detection_and_leave_alignment() {
    let (text, _) = parse(&format!("intro\n\n{TABLE}\n\nafter¦"));
    let doc = Document::from_text(&text);
    let start = text.find('|').unwrap();
    let range = command::table_range_at(&doc, start + 3).unwrap();
    assert_eq!(&text[range.clone()], TABLE);
    assert!(command::table_range_at(&doc, 2).is_none());
    // Cursor left the table for "after": align it and keep the cursor there.
    let mut doc = doc;
    let cursor = Selection::cursor(text.len());
    let r = command::align_table_at(&doc, range.start, cursor).unwrap();
    assert_eq!(
        apply(&mut doc, r),
        "intro\n\n| Name  | Qty |\n| :---- | --: |\n| apple |   3 |\n| fig   |  12 |\n\nafter¦"
    );
}

#[test]
fn paste_csv_and_tsv_as_table() {
    check(
        DocCommand::PasteTable { text: "name,qty\napple,3\n\"fig, dried\",12\n".into() },
        "Fruit:¦",
        "Fruit:\n\n| name       | qty |\n| ---------- | --- |\n| apple      | 3   |\n| fig, dried | 12  |\n¦",
    );
    check(
        DocCommand::PasteTable { text: "a\tb|c\r\n1\t2\r\n".into() },
        "¦",
        "| a   | b\\|c |\n| --- | ---- |\n| 1   | 2    |\n¦",
    );
    check(DocCommand::PasteTable { text: String::new() }, "x¦", "x¦");
}

// ---- documents ----

#[test]
fn crlf_documents_get_crlf_lines() {
    let mut doc = Document::from_text("a\r\nb");
    doc.format.line_ending = mdedit_core::io::LineEnding::CrLf;
    let r = command::run(&doc, Selection::cursor(1), &DocCommand::HorizontalRule);
    let txn = doc.next_txn();
    for e in r.edits {
        doc.apply(&mdedit_core::Edit { txn, ..e });
    }
    assert_eq!(doc.buffer().text(), "a\r\n\r\n---\r\n\r\nb");
}

#[test]
fn command_undoes_as_one_step() {
    let (text, sel) = parse("‹one\ntwo›");
    let mut doc = Document::from_text(&text);
    let r = command::run(&doc, sel, &DocCommand::BulletList);
    assert_eq!(r.edits.len(), 2);
    apply(&mut doc, r);
    assert_eq!(doc.buffer().text(), "- one\n- two");
    doc.undo();
    assert_eq!(doc.buffer().text(), "one\ntwo");
}

#[test]
fn large_documents_skip_smart_input_but_run_commands() {
    let doc = Document::with_buffer(Box::new(mdedit_core::RopeBuffer::from_text("- a")), false);
    assert!(command::smart_input(&doc, Selection::cursor(3), "\n").is_none());
    let r = command::run(&doc, Selection::cursor(3), &DocCommand::ToggleBold);
    assert_eq!(r.edits.len(), 2);
}

// ---- robustness ----

mod fuzz {
    use super::*;
    use proptest::prelude::*;

    const PIECES: &[&str] = &[
        "- ", "1. ", "> ", "| ", " |", "|---|", "*", "**", "`", "# ", "\n", "\n\n", "a", "word ", "日本", " ", "[", "](",
        ")", "~~", "_", "\t", "---", "```", "[ ] ", "\\|", "é", "\r\n",
    ];

    fn doc_text() -> impl Strategy<Value = String> {
        prop::collection::vec(prop::sample::select(PIECES), 0..40).prop_map(|v| v.concat())
    }

    fn boundary(text: &str, n: usize) -> usize {
        let mut at = n % (text.len() + 1);
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    fn commands() -> Vec<DocCommand> {
        let mut v: Vec<DocCommand> = command::registry().iter().map(|m| m.command()).collect();
        v.push(DocCommand::Link { url: Some("https://x.org/a b".into()) });
        v.push(DocCommand::PasteTable { text: "a,b\n1,\"2\n3\"\n".into() });
        v.push(DocCommand::InsertTable { rows: 1, cols: 4 });
        v
    }

    fn check_result(doc: &Document, r: &CommandResult) -> Document {
        let mut copy = Document::from_text(&doc.buffer().text());
        copy.format = doc.format;
        let mut last_start = usize::MAX;
        for e in &r.edits {
            // Ordered last first and non-overlapping.
            assert!(e.range.end <= last_start, "edits overlap or are out of order: {:?}", r.edits);
            last_start = e.range.start;
            let text = copy.buffer().text();
            assert!(text.is_char_boundary(e.range.start) && text.is_char_boundary(e.range.end));
            copy.apply(e);
        }
        let len = copy.buffer().len_bytes();
        let text = copy.buffer().text();
        for at in [r.selection.anchor, r.selection.head] {
            assert!(at <= len && text.is_char_boundary(at), "selection {:?} invalid in {text:?}", r.selection);
        }
        copy
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn commands_never_panic_and_give_valid_edits(text in doc_text(), a in 0usize..400, h in 0usize..400) {
            let doc = Document::from_text(&text);
            let sel = Selection { anchor: boundary(&text, a), head: boundary(&text, h) };
            for cmd in commands() {
                let r = command::run(&doc, sel, &cmd);
                let copy = check_result(&doc, &r);
                // The block index stays consistent with a full reparse.
                let fresh = Document::from_text(&copy.buffer().text());
                prop_assert_eq!(
                    copy.blocks().unwrap().blocks().iter().collect::<Vec<_>>().len(),
                    fresh.blocks().unwrap().blocks().iter().collect::<Vec<_>>().len()
                );
            }
            for input in ["\n", "\t", "*", "[", "**"] {
                if let Some(r) = command::smart_input(&doc, sel, input) {
                    check_result(&doc, &r);
                }
            }
            if let Some(r) = command::smart_paste(&doc, sel, "https://x.org") {
                check_result(&doc, &r);
            }
            if let Some(r) = command::align_table_at(&doc, sel.head, sel) {
                check_result(&doc, &r);
            }
        }
    }
}
