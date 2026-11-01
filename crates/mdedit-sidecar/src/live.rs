//! Comment ranges while a document is open (SPEC 7, ARCHITECTURE 6).
//!
//! On open, [`LiveAnchors::resolve`] re-anchors every comment once. From then
//! on the ranges follow the edit log as plain offset arithmetic, so typing
//! never searches. Before saving, [`LiveAnchors::write_back`] turns the live
//! ranges back into quote + context + hint.
//!
//! Detaching rules for edits:
//! - Editing inside a range shrinks or grows it; it detaches only when every
//!   char of the quote has been deleted (the comment is kept, in the
//!   "Detached" group).
//! - An [`EditOrigin::External`] edit (reload after the file changed on
//!   disk) that touches a range marks it pending instead of guessing; call
//!   [`LiveAnchors::reanchor_pending`] with the new text to find it again.

use mdedit_core::{AppliedEdit, EditListener, EditOrigin, TextBuffer};

use crate::anchor::{make_anchor, reanchor};
use crate::{Comment, Resolution};

/// Comment ranges while the document is open, moved by the edit log.
#[derive(Debug, Default)]
pub struct LiveAnchors {
    /// Indexed like `Sidecar::comments`.
    pub ranges: Vec<Resolution>,
    /// Ranges an external change touched; looked up again by
    /// [`Self::reanchor_pending`]. May be shorter than `ranges`.
    pending: Vec<bool>,
}

impl LiveAnchors {
    /// Re-anchor every comment against the current text (on open).
    pub fn resolve(comments: &[Comment], buf: &dyn TextBuffer) -> Self {
        Self { ranges: comments.iter().map(|c| reanchor(&c.anchor, buf)).collect(), pending: Vec::new() }
    }

    /// Track a new comment (appended to `Sidecar::comments` at the same
    /// index). Returns its index.
    pub fn push(&mut self, resolution: Resolution) -> usize {
        self.ranges.push(resolution);
        self.ranges.len() - 1
    }

    /// Stop tracking comment `idx` (removed from `Sidecar::comments`).
    pub fn remove(&mut self, idx: usize) -> Resolution {
        if idx < self.pending.len() {
            self.pending.remove(idx);
        }
        self.ranges.remove(idx)
    }

    /// Whether an external change left ranges to look up again.
    pub fn has_pending(&self) -> bool {
        self.pending.iter().any(|&p| p)
    }

    /// Re-anchor the comments an external change touched, from their stored
    /// anchors. Call after the reload's edits are applied.
    pub fn reanchor_pending(&mut self, comments: &[Comment], buf: &dyn TextBuffer) {
        for (i, pending) in self.pending.iter_mut().enumerate() {
            if std::mem::take(pending)
                && let (Some(range), Some(c)) = (self.ranges.get_mut(i), comments.get(i))
            {
                *range = reanchor(&c.anchor, buf);
            }
        }
    }

    /// Refresh the stored anchors of attached comments from the live ranges
    /// (before saving the sidecar, and before reloading after an external
    /// change). Detached comments keep their last anchor so they can still
    /// be found if the text comes back.
    pub fn write_back(&self, comments: &mut [Comment], buf: &dyn TextBuffer) {
        for (range, c) in self.ranges.iter().zip(comments) {
            if let Resolution::Attached(r) = range {
                c.anchor = make_anchor(buf, r.clone());
            }
        }
    }
}

impl EditListener for LiveAnchors {
    fn on_edit(&mut self, applied: &AppliedEdit) {
        let edit = &applied.edit.range;
        let external = applied.edit.origin == EditOrigin::External;
        for (i, res) in self.ranges.iter_mut().enumerate() {
            let Resolution::Attached(r) = res else { continue };
            if external && edit.start <= r.end && edit.end >= r.start {
                *res = Resolution::Detached;
                if self.pending.len() <= i {
                    self.pending.resize(i + 1, false);
                }
                self.pending[i] = true;
                continue;
            }
            if r.start == r.end {
                let at = applied.map_offset(r.start, false);
                *r = at..at;
                continue;
            }
            // Text typed at either edge stays outside the comment.
            let start = applied.map_offset(r.start, true);
            // An end inside or at the end of a replaced range moves to its
            // start: replacement text for the comment's tail is not part of it.
            let end = if edit.start < r.end && r.end <= edit.end { edit.start } else { applied.map_offset(r.end, false) };
            *res = if start < end { Resolution::Attached(start..end) } else { Resolution::Detached };
        }
    }
}

#[cfg(test)]
mod tests {
    use mdedit_core::{Edit, RopeBuffer, TxnId};
    use proptest::prelude::*;

    use super::*;
    use crate::{Anchor, CONTEXT_CHARS, reanchor};

    fn comment(anchor: Anchor) -> Comment {
        Comment {
            id: "c".into(),
            author: "t".into(),
            created: jiff::Timestamp::UNIX_EPOCH,
            resolved: false,
            body: String::new(),
            anchor,
            replies: Vec::new(),
        }
    }

    struct Fixture {
        buf: RopeBuffer,
        live: LiveAnchors,
        comments: Vec<Comment>,
    }

    impl Fixture {
        fn new(text: &str, quotes: &[&str]) -> Self {
            let buf = RopeBuffer::from_text(text);
            let comments: Vec<Comment> = quotes
                .iter()
                .map(|q| {
                    let s = text.find(q).unwrap();
                    comment(make_anchor(&buf, s..s + q.len()))
                })
                .collect();
            let live = LiveAnchors::resolve(&comments, &buf);
            Self { buf, live, comments }
        }

        fn apply(&mut self, range: std::ops::Range<usize>, text: &str, origin: EditOrigin) {
            let applied = self.buf.apply(&Edit { range, text: text.into(), origin, txn: TxnId(0) });
            self.live.on_edit(&applied);
        }

        fn quoted(&self, i: usize) -> Option<String> {
            self.live.ranges[i].range().map(|r| self.buf.slice(r).into_owned())
        }
    }

    #[test]
    fn edits_around_and_inside_move_the_range() {
        let mut f = Fixture::new("one two three\n", &["two"]);
        f.apply(0..0, "zero ", EditOrigin::User);
        assert_eq!(f.quoted(0).as_deref(), Some("two"));
        // Typing at either edge stays outside.
        let r = f.live.ranges[0].range().unwrap();
        f.apply(r.end..r.end, "!", EditOrigin::User);
        f.apply(r.start..r.start, "(", EditOrigin::User);
        assert_eq!(f.quoted(0).as_deref(), Some("two"));
        // Typing inside is included.
        let r = f.live.ranges[0].range().unwrap();
        f.apply(r.start + 1..r.start + 1, "oo", EditOrigin::User);
        assert_eq!(f.quoted(0).as_deref(), Some("toowo"));
        // Deleting part of it shrinks it, deleting all of it detaches.
        let r = f.live.ranges[0].range().unwrap();
        f.apply(r.start..r.start + 2, "", EditOrigin::User);
        assert_eq!(f.quoted(0).as_deref(), Some("owo"));
        let r = f.live.ranges[0].range().unwrap();
        f.apply(r.end - 1..r.end, "X", EditOrigin::User);
        assert_eq!(f.quoted(0).as_deref(), Some("ow"));
        let r = f.live.ranges[0].range().unwrap();
        f.apply(r.start - 1..r.end + 1, "X", EditOrigin::User);
        assert_eq!(f.live.ranges[0], Resolution::Detached);
        assert!(!f.live.has_pending());
        assert_eq!(f.live.ranges.len(), 1);
    }

    #[test]
    fn external_change_reanchors_touched_ranges_only() {
        let text = "# Title\n\nFirst paragraph with a remark.\n\nSecond paragraph, untouched.\n";
        let mut f = Fixture::new(text, &["a remark", "untouched"]);
        f.live.write_back(&mut f.comments, &f.buf);
        // The reload replaces the first paragraph (with an extra word) and
        // shifts everything after it.
        let p = text.find("First").unwrap();
        let e = text.find("\n\nSecond").unwrap();
        f.apply(p..e, "First new paragraph, now with a remark.", EditOrigin::External);
        assert_eq!(f.live.ranges[0], Resolution::Detached);
        assert_eq!(f.quoted(1).as_deref(), Some("untouched"));
        assert!(f.live.has_pending());
        f.live.reanchor_pending(&f.comments, &f.buf);
        assert!(!f.live.has_pending());
        assert_eq!(f.quoted(0).as_deref(), Some("a remark"));
        assert_eq!(f.quoted(1).as_deref(), Some("untouched"));
    }

    #[test]
    fn whole_file_reload_reanchors_everything() {
        let old = "alpha beta gamma\ndelta epsilon zeta\n";
        let mut f = Fixture::new(old, &["beta gamma", "epsilon"]);
        f.live.write_back(&mut f.comments, &f.buf);
        let new = "preface\nalpha beta gamma!\ndelta and epsilon zeta\n";
        f.apply(0..old.len(), new, EditOrigin::External);
        f.live.reanchor_pending(&f.comments, &f.buf);
        assert_eq!(f.quoted(0).as_deref(), Some("beta gamma"));
        assert_eq!(f.quoted(1).as_deref(), Some("epsilon"));
    }

    #[test]
    fn external_deletion_detaches_but_keeps_the_comment() {
        let old = "keep this\ndrop this line entirely\n";
        let mut f = Fixture::new(old, &["drop this line entirely"]);
        f.live.write_back(&mut f.comments, &f.buf);
        f.apply(0..old.len(), "keep this\n", EditOrigin::External);
        f.live.reanchor_pending(&f.comments, &f.buf);
        assert_eq!(f.live.ranges, vec![Resolution::Detached]);
        // Its anchor survives a write_back for when the text comes back.
        f.live.write_back(&mut f.comments, &f.buf);
        assert_eq!(f.comments[0].anchor.quote, "drop this line entirely");
    }

    #[test]
    fn push_and_remove_keep_indices_aligned() {
        let mut f = Fixture::new("a b c d\n", &["b", "d"]);
        f.apply(0..8, "a b c d\n", EditOrigin::External);
        f.live.remove(0);
        f.comments.remove(0);
        assert_eq!(f.live.push(Resolution::Attached(0..1)), 1);
        f.comments.push(comment(make_anchor(&f.buf, 0..1)));
        f.live.reanchor_pending(&f.comments, &f.buf);
        assert_eq!(f.quoted(0).as_deref(), Some("d"));
        assert_eq!(f.quoted(1).as_deref(), Some("a"));
    }

    // Fuzz: random edits around anchors. A reference model tags every char
    // with the set of anchors it belongs to: original chars by position,
    // inserted chars when the insertion point has members of that anchor on
    // both sides. An anchor must cover exactly its member chars, or be
    // detached when none are left.
    // original char; an anchor must cover exactly the span from its first
    // to its last surviving char, or be detached when none survive. Anchors
    // are never lost, and a written-back anchor re-anchors to the same range.

    #[derive(Debug, Clone)]
    struct FuzzEdit {
        at: usize,
        del: usize,
        text: String,
    }

    fn alphabet() -> impl Strategy<Value = String> {
        proptest::collection::vec(prop::sample::select(vec!['a', 'b', ' ', '\n', 'é', '–', 'x', '\r']), 0..6)
            .prop_map(|v| v.into_iter().collect())
    }

    fn fuzz_edit() -> impl Strategy<Value = FuzzEdit> {
        (any::<usize>(), 0usize..6, alphabet()).prop_map(|(at, del, text)| FuzzEdit { at, del, text })
    }


    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn anchors_move_or_detach_never_lost(
            doc in proptest::collection::vec(prop::sample::select(vec!['a', 'b', 'c', ' ', '\n', 'é', '–']), 1..80),
            spans in proptest::collection::vec((any::<usize>(), 1usize..12), 1..6),
            edits in proptest::collection::vec(fuzz_edit(), 1..40),
        ) {
            let text: String = doc.iter().collect();
            let byte_of = |model: &[(char, u32)], ci: usize| model[..ci].iter().map(|(c, _)| c.len_utf8()).sum::<usize>();

            let mut buf = RopeBuffer::from_text(&text);
            let anchors: Vec<std::ops::Range<usize>> = spans
                .iter()
                .map(|&(s, l)| { let s = s % doc.len(); s..(s + l).min(doc.len()) })
                .collect();
            let mut model: Vec<(char, u32)> = doc
                .iter()
                .enumerate()
                .map(|(ci, &c)| (c, (0..anchors.len()).filter(|&a| anchors[a].contains(&ci)).fold(0, |m, a| m | 1 << a)))
                .collect();
            let comments: Vec<Comment> = anchors
                .iter()
                .map(|r| comment(make_anchor(&buf, byte_of(&model, r.start)..byte_of(&model, r.end))))
                .collect();
            let mut live = LiveAnchors::resolve(&comments, &buf);
            for (i, r) in anchors.iter().enumerate() {
                prop_assert_eq!(live.ranges[i].range(), Some(byte_of(&model, r.start)..byte_of(&model, r.end)));
            }

            for e in edits {
                let at = e.at % (model.len() + 1);
                let end = (at + e.del).min(model.len());
                let range = byte_of(&model, at)..byte_of(&model, end);
                let applied = buf.apply(&Edit { range, text: e.text.clone(), origin: EditOrigin::User, txn: TxnId(0) });
                live.on_edit(&applied);
                let before = model[..at].iter().fold(0, |m, (_, t)| m | t);
                let after = model[end..].iter().fold(0, |m, (_, t)| m | t);
                model.splice(at..end, e.text.chars().map(|c| (c, before & after)));
            }

            let current: String = model.iter().map(|(c, _)| c).collect();
            let now = buf.text().into_owned();
            prop_assert_eq!(now.as_str(), current.as_str());
            prop_assert_eq!(live.ranges.len(), anchors.len());
            for i in 0..anchors.len() {
                let surviving: Vec<usize> = (0..model.len()).filter(|&ci| model[ci].1 & (1 << i) != 0).collect();
                let expected = match (surviving.first(), surviving.last()) {
                    (Some(&a), Some(&b)) => Resolution::Attached(byte_of(&model, a)..byte_of(&model, b + 1)),
                    _ => Resolution::Detached,
                };
                prop_assert_eq!(&live.ranges[i], &expected, "anchor {}", i);
            }

            // Written-back anchors find exactly the live ranges again.
            let mut comments = comments;
            live.write_back(&mut comments, &buf);
            let again = LiveAnchors::resolve(&comments, &buf);
            for (i, r) in live.ranges.iter().enumerate() {
                if let Resolution::Attached(r) = r {
                    prop_assert_eq!(again.ranges[i].range(), Some(r.clone()), "reanchor {}", i);
                }
            }
        }

        #[test]
        fn reanchor_after_unrelated_external_edits(
            words in proptest::collection::vec("[a-z]{2,8}", 30..60),
            pick in any::<usize>(),
            noise in proptest::collection::vec((any::<usize>(), "[A-Z ]{1,6}"), 1..8),
        ) {
            // A comment on a 3-word phrase; external edits insert noise
            // elsewhere (never inside the phrase or its context).
            let text = words.join(" ");
            let w = pick % (words.len() - 2);
            let start: usize = words[..w].iter().map(|s| s.len() + 1).sum();
            let quote = words[w..w + 3].join(" ");
            let buf = RopeBuffer::from_text(&text);
            let a = make_anchor(&buf, start..start + quote.len());
            let guard = start.saturating_sub(CONTEXT_CHARS * 2)..start + quote.len() + CONTEXT_CHARS * 2;

            let mut edited = text.clone();
            let mut expected = start;
            let mut offsets: Vec<(usize, String)> = noise
                .into_iter()
                .map(|(p, s)| (p % (text.len() + 1), s))
                .filter(|(p, _)| !guard.contains(p))
                .collect();
            offsets.sort_by(|x, y| y.0.cmp(&x.0));
            for (p, s) in offsets {
                edited.insert_str(p, &s);
                if p <= start { expected += s.len(); }
            }
            let r = reanchor(&a, &RopeBuffer::from_text(&edited)).range();
            prop_assert!(r.is_some());
            let r = r.unwrap();
            prop_assert_eq!(&edited[r.clone()], quote.as_str());
            // Identical phrases elsewhere may exist; the context must match.
            prop_assert!(edited[..r.start].ends_with(&a.prefix));
            let full = format!("{}{}{}", a.prefix, quote, a.suffix);
            if text.matches(full.as_str()).count() == 1 {
                prop_assert_eq!(r.start, expected);
            }
        }
    }
}
