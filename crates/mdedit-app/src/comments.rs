//! Comments (SPEC 7; DESIGN_SYSTEM 6, 7 and 8.2): comments live in the
//! sidecar next to the markdown file, anchored by quote and context.
//!
//! - Each tab owns a [`DocComments`]: the sidecar plus the live comment
//!   ranges, which follow the document's edit log ([`DocComments::sync`],
//!   run after every update), so typing, undo and reloads move or detach
//!   them without searching.
//! - Comments are added from a selection (floating toolbar, context menu,
//!   `comment.add` = Ctrl+Alt+M, palette) in every mode, Read included,
//!   since adding a comment never edits the markdown.
//! - The sidecar is written when the document is saved, and right away when
//!   a comment changes while the document is clean (Read mode, say). In
//!   project mode every write counts as a save for the autocommit, which
//!   commits the whole worktree, sidecar included.
//! - The sidecar file is watched like the document: an external change
//!   (git pull, branch switch, another editor) reloads it unless there are
//!   unsaved comment changes.
//!
//! The app hooks in through `App.comments`, `Tab.comments`,
//! `Message::Comments` and a few calls (`on_comments`, `comments_key`,
//! `after_comments_update`, `comments_saved`, `comments_file_changed`,
//! `comment_files`); the panel is in `view/comments_view.rs`.

use std::ops::Range;
use std::path::{Path, PathBuf};

use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::text_editor;
use mdedit_core::{AppliedEdit, Document, EditListener, Selection, TextBuffer};
use mdedit_editor::{Highlight, HighlightKind};
use mdedit_sidecar::{Comment, LiveAnchors, Reply, Resolution, Sidecar, make_anchor, sidecar_path};

use crate::app::{App, Message};
use crate::tab::{DiskStamp, Tab, TabId};

pub const DRAFT_INPUT: &str = "comment-draft";
pub const REPLY_INPUT: &str = "comment-reply";

#[derive(Debug, Clone)]
pub enum CommentMsg {
    DraftAction(text_editor::Action),
    DraftSubmit,
    DraftCancel,
    /// Focus a comment: highlight it and move the cursor to its anchor.
    Select(String),
    StartReply(String),
    ReplyAction(text_editor::Action),
    ReplySubmit,
    ReplyCancel,
    Resolve(String, bool),
    /// First click asks, the second deletes.
    Delete(String),
    ShowResolved(bool),
}

/// Comments of one open document.
#[derive(Debug, Default)]
pub struct DocComments {
    pub sidecar: Sidecar,
    pub live: LiveAnchors,
    /// Edit log version the ranges are at.
    version: u64,
    /// The range a new comment is being written for.
    draft: Option<DraftRange>,
    /// Changed since the sidecar was last written.
    pub dirty: bool,
    /// The sidecar could not be read: shown in the panel, never overwritten.
    pub load_error: Option<String>,
    /// The sidecar file as we last wrote or read it.
    disk: Option<DiskStamp>,
    /// Bumped whenever ranges or comments change (highlight cache key).
    generation: u64,
    highlights_for: Option<(u64, Option<String>, bool)>,
}

#[derive(Debug, Clone)]
struct DraftRange(Range<usize>);

impl EditListener for DraftRange {
    fn on_edit(&mut self, applied: &AppliedEdit) {
        let start = applied.map_offset(self.0.start, true);
        let end = applied.map_offset(self.0.end, false).max(start);
        self.0 = start..end;
    }
}

impl DocComments {
    /// Read the sidecar of `doc` and anchor its comments. Large files get
    /// none (anchoring needs the whole text).
    pub fn load(doc: &Document) -> Self {
        let mut this = Self { version: doc.log().last_version(), ..Self::default() };
        if doc.is_large() {
            return this;
        }
        if let Some(path) = &doc.path {
            match Sidecar::load(path) {
                Ok(s) => this.sidecar = s,
                Err(e) => this.load_error = Some(e.to_string()),
            }
            this.disk = DiskStamp::read(&sidecar_path(path));
        }
        this.live = LiveAnchors::resolve(&this.sidecar.comments, doc.buffer());
        this
    }

    pub fn available(doc: &Document) -> bool {
        !doc.is_large()
    }

    /// Bring the ranges up to date with the document's edits. Returns
    /// whether anything moved.
    pub fn sync(&mut self, doc: &Document) -> bool {
        let last = doc.log().last_version();
        if last == self.version {
            return false;
        }
        let from = self.version;
        if doc.log().catch_up(&mut self.live, from).is_none() {
            self.live.mark_all_pending();
        }
        if let Some(d) = &mut self.draft
            && doc.log().catch_up(d, from).is_none()
        {
            self.draft = None;
        }
        if self.live.has_pending() {
            self.live.reanchor_pending(&self.sidecar.comments, doc.buffer());
        }
        self.version = last;
        self.generation += 1;
        true
    }

    /// Before the buffer is replaced by what is on disk: store the current
    /// anchors so touched comments can be found again in the new text.
    pub fn before_reload(&mut self, doc: &Document) {
        self.sync(doc);
        self.live.write_back(&mut self.sidecar.comments, doc.buffer());
    }

    fn changed(&mut self) {
        self.generation += 1;
        self.dirty = true;
    }

    pub fn range_of(&self, id: &str) -> Option<Range<usize>> {
        let i = self.index_of(id)?;
        self.live.ranges.get(i).and_then(Resolution::range)
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.sidecar.comments.iter().position(|c| c.id == id)
    }

    fn comment_mut(&mut self, id: &str) -> Option<&mut Comment> {
        self.sidecar.comments.iter_mut().find(|c| c.id == id)
    }

    /// Open comments in document order, then detached ones; resolved ones
    /// only with `resolved`.
    pub fn ordered(&self, resolved: bool) -> (Vec<(&Comment, Range<usize>)>, Vec<&Comment>) {
        let mut attached = Vec::new();
        let mut detached = Vec::new();
        for (c, r) in self.sidecar.comments.iter().zip(&self.live.ranges) {
            if c.resolved && !resolved {
                continue;
            }
            match r {
                Resolution::Attached(r) => attached.push((c, r.clone())),
                Resolution::Detached => detached.push(c),
            }
        }
        attached.sort_by_key(|(c, r)| (r.start, c.created));
        (attached, detached)
    }

    pub fn resolved_count(&self) -> usize {
        self.sidecar.comments.iter().filter(|c| c.resolved).count()
    }

    /// The unresolved comment whose range holds `byte`, preferring the
    /// innermost one.
    pub fn at(&self, byte: usize) -> Option<&str> {
        self.sidecar
            .comments
            .iter()
            .zip(&self.live.ranges)
            .filter_map(|(c, r)| Some((c, r.range()?)))
            .filter(|(c, r)| !c.resolved && r.start <= byte && byte <= r.end && !r.is_empty())
            .min_by_key(|(_, r)| r.len())
            .map(|(c, _)| c.id.as_str())
    }

    /// Highlights for the document widgets, if they changed since last time.
    fn highlights(&mut self, active: Option<&str>, resolved: bool) -> Option<Vec<Highlight>> {
        let key = (self.generation, active.map(str::to_owned), resolved);
        if self.highlights_for.as_ref() == Some(&key) {
            return None;
        }
        self.highlights_for = Some(key);
        let mut out: Vec<Highlight> = self
            .sidecar
            .comments
            .iter()
            .zip(&self.live.ranges)
            .filter_map(|(c, r)| {
                let range = r.range()?;
                let is_active = active == Some(c.id.as_str());
                if (c.resolved && !is_active) || range.is_empty() {
                    return None;
                }
                let kind = if is_active { HighlightKind::ActiveComment } else { HighlightKind::Comment };
                Some(Highlight { range, kind })
            })
            .collect();
        if let Some(d) = self.draft.as_ref().filter(|d| !d.0.is_empty()) {
            out.push(Highlight { range: d.0.clone(), kind: HighlightKind::ActiveComment });
        }
        out.sort_by_key(|h| h.range.start);
        Some(out)
    }

    /// Write the sidecar for the document at `path` (deleted when empty).
    pub fn write(&mut self, doc: &Document, path: &Path) -> Result<(), String> {
        if let Some(e) = &self.load_error {
            return Err(format!("the comments file could not be read ({e}), so it was not overwritten"));
        }
        self.sync(doc);
        self.live.write_back(&mut self.sidecar.comments, doc.buffer());
        self.sidecar.save(path).map_err(|e| e.to_string())?;
        self.disk = DiskStamp::read(&sidecar_path(path));
        self.dirty = false;
        Ok(())
    }

    /// The sidecar changed on disk: read it again, unless ours has unsaved
    /// changes. Returns whether it was reloaded.
    fn reload_from_disk(&mut self, doc: &Document, path: &Path) -> bool {
        let now = DiskStamp::read(&sidecar_path(path));
        if now == self.disk || self.dirty {
            return false;
        }
        let fresh = Self::load(doc);
        let draft = self.draft.take();
        *self = Self { draft, generation: self.generation + 1, ..fresh };
        true
    }
}

/// A new comment being written.
pub struct Draft {
    pub tab: TabId,
    pub quote: String,
    pub content: text_editor::Content,
}

/// A reply being written.
pub struct ReplyDraft {
    pub tab: TabId,
    pub id: String,
    pub content: text_editor::Content,
}

/// App-wide comment UI state.
#[derive(Default)]
pub struct CommentsUi {
    pub draft: Option<Draft>,
    pub reply: Option<ReplyDraft>,
    /// Focused comment, per the active tab.
    pub active: Option<(TabId, String)>,
    pub confirm_delete: Option<String>,
    pub show_resolved: bool,
    /// A comment box has the keyboard (its last input came after the
    /// document's), so document shortcuts such as Ctrl+Z stay away.
    pub typing: bool,
    /// Focus this input after the update (opening a box).
    focus: Option<&'static str>,
    /// The caret moved in this tab's document during the update.
    pub caret_moved: Option<TabId>,
}

impl CommentsUi {
    pub fn active_for(&self, tab: TabId) -> Option<&str> {
        self.active.as_ref().filter(|(t, _)| *t == tab).map(|(_, id)| id.as_str())
    }
}

impl App {
    /// `comment.add`: start a comment on the selection of the active tab.
    pub(crate) fn add_comment(&mut self) -> Task<Message> {
        let Some(tab) = self.tabs.get_mut(self.active) else { return Task::none() };
        if !DocComments::available(&tab.doc) {
            self.toast("Comments are not available for large files.");
            return Task::none();
        }
        let range = trim_range(tab.doc.buffer(), tab.selection().range());
        if range.is_empty() {
            self.toast("Select the text to comment on first.");
            return Task::none();
        }
        let quote = tab.doc.buffer().slice(range.clone()).into_owned();
        tab.comments.sync(&tab.doc);
        tab.comments.draft = Some(DraftRange(range));
        tab.comments.generation += 1;
        let id = tab.id;
        self.drop_draft();
        self.comments.draft = Some(Draft { tab: id, quote, content: text_editor::Content::new() });
        self.comments.reply = None;
        self.comments.active = None;
        self.comments_visible = true;
        self.comments.focus = Some(DRAFT_INPUT);
        self.comments.typing = true;
        self.hide_toolbar_for_typing();
        self.context_menu = None;
        Task::none()
    }

    /// Forget the open draft and its highlighted range.
    fn drop_draft(&mut self) {
        if let Some(d) = self.comments.draft.take()
            && let Some(tab) = self.tabs.iter_mut().find(|t| t.id == d.tab)
        {
            tab.comments.draft = None;
            tab.comments.generation += 1;
        }
    }

    pub(crate) fn on_comments(&mut self, msg: CommentMsg) -> Task<Message> {
        match msg {
            CommentMsg::DraftAction(a) => {
                self.comments.typing = true;
                if let Some(d) = &mut self.comments.draft {
                    d.content.perform(a);
                }
            }
            CommentMsg::DraftCancel => {
                self.drop_draft();
                return self.comment_box_closed();
            }
            CommentMsg::DraftSubmit => {
                let Some(d) = &self.comments.draft else { return Task::none() };
                let body = d.content.text().trim().to_string();
                let tab_id = d.tab;
                if body.is_empty() {
                    return Task::none();
                }
                let author = self.comment_author();
                let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return Task::none() };
                tab.comments.sync(&tab.doc);
                let Some(range) = tab.comments.draft.take().map(|d| d.0).filter(|r| !r.is_empty()) else {
                    self.comments.draft = None;
                    self.toast_error("The commented text was deleted.");
                    return self.comment_box_closed();
                };
                let comment = Comment {
                    id: new_id(),
                    author,
                    created: jiff::Timestamp::now(),
                    resolved: false,
                    body,
                    anchor: make_anchor(tab.doc.buffer(), range.clone()),
                    replies: Vec::new(),
                };
                let id = comment.id.clone();
                tab.comments.sidecar.comments.push(comment);
                tab.comments.live.push(Resolution::Attached(range));
                tab.comments.changed();
                self.comments.draft = None;
                self.comments.active = Some((tab_id, id));
                return Task::batch([self.persist_comments(tab_id), self.comment_box_closed()]);
            }
            CommentMsg::Select(id) => {
                let Some(tab) = self.tabs.get_mut(self.active) else { return Task::none() };
                self.comments.confirm_delete = None;
                if let Some(r) = tab.comments.range_of(&id) {
                    // Cursor at the start: selecting the text would bring the
                    // format toolbar up over the panel's action.
                    tab.set_selection(Selection::cursor(r.start));
                    tab.editor.scroll_to_byte = Some(r.start);
                    tab.view.sync = Some(mdedit_view::SyncAnchor { byte: r.start, fraction: 0.0 });
                    tab.preview.sync = tab.view.sync;
                }
                self.comments.active = Some((tab.id, id));
            }
            CommentMsg::StartReply(id) => {
                let Some(tab) = self.tabs.get(self.active) else { return Task::none() };
                self.comments.reply =
                    Some(ReplyDraft { tab: tab.id, id: id.clone(), content: text_editor::Content::new() });
                self.comments.active = Some((tab.id, id));
                self.comments.focus = Some(REPLY_INPUT);
                self.comments.typing = true;
            }
            CommentMsg::ReplyAction(a) => {
                self.comments.typing = true;
                if let Some(r) = &mut self.comments.reply {
                    r.content.perform(a);
                }
            }
            CommentMsg::ReplyCancel => {
                self.comments.reply = None;
                return self.comment_box_closed();
            }
            CommentMsg::ReplySubmit => {
                let Some(r) = &self.comments.reply else { return Task::none() };
                let body = r.content.text().trim().to_string();
                if body.is_empty() {
                    return Task::none();
                }
                let (tab_id, id) = (r.tab, r.id.clone());
                let author = self.comment_author();
                let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return Task::none() };
                if let Some(c) = tab.comments.comment_mut(&id) {
                    c.replies.push(Reply { author, created: jiff::Timestamp::now(), body });
                    // Answering a resolved thread reopens it.
                    c.resolved = false;
                    tab.comments.changed();
                }
                self.comments.reply = None;
                return Task::batch([self.persist_comments(tab_id), self.comment_box_closed()]);
            }
            CommentMsg::Resolve(id, resolved) => {
                let Some(tab) = self.tabs.get_mut(self.active) else { return Task::none() };
                if let Some(c) = tab.comments.comment_mut(&id) {
                    c.resolved = resolved;
                    tab.comments.changed();
                }
                let tab_id = tab.id;
                if resolved && self.comments.active_for(tab_id) == Some(id.as_str()) {
                    self.comments.active = None;
                }
                return self.persist_comments(tab_id);
            }
            CommentMsg::Delete(id) => {
                if self.comments.confirm_delete.as_deref() != Some(id.as_str()) {
                    self.comments.confirm_delete = Some(id);
                    return Task::none();
                }
                self.comments.confirm_delete = None;
                let Some(tab) = self.tabs.get_mut(self.active) else { return Task::none() };
                tab.comments.sync(&tab.doc);
                if let Some(i) = tab.comments.index_of(&id) {
                    tab.comments.sidecar.comments.remove(i);
                    tab.comments.live.remove(i);
                    tab.comments.changed();
                }
                let tab_id = tab.id;
                if self.comments.reply.as_ref().is_some_and(|r| r.id == id) {
                    self.comments.reply = None;
                }
                if self.comments.active_for(tab_id) == Some(id.as_str()) {
                    self.comments.active = None;
                }
                return self.persist_comments(tab_id);
            }
            CommentMsg::ShowResolved(on) => {
                self.comments.show_resolved = on;
                self.comments.confirm_delete = None;
            }
        }
        Task::none()
    }

    /// A comment box closed: the keyboard goes back to the document.
    fn comment_box_closed(&mut self) -> Task<Message> {
        self.comments.typing = false;
        self.focus_document()
    }

    fn comment_author(&self) -> String {
        let name = self.settings.author.name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        if let Some(p) = &self.project
            && let Ok(repo) = mdedit_git::Repo::open(&p.project.root)
        {
            let name = repo.default_author().name;
            if !name.trim().is_empty() {
                return name;
            }
        }
        ["USER", "USERNAME"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty())).unwrap_or("Me".into())
    }

    /// Write the sidecar now if the document is clean (its text on disk
    /// matches the anchors), else at the next save.
    fn persist_comments(&mut self, tab_id: TabId) -> Task<Message> {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return Task::none() };
        let Some(path) = tab.doc.path.clone() else { return Task::none() };
        if tab.is_dirty() || tab.saving {
            return Task::none();
        }
        match tab.comments.write(&tab.doc, &path) {
            Ok(()) => self.project_saved(&path),
            Err(e) => {
                self.toast_error(format!("Could not save comments: {e}"));
                Task::none()
            }
        }
    }

    /// The document was saved to `path`: save its comments with it.
    pub(crate) fn comments_saved(&mut self, tab_id: TabId, path: &Path) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
        let c = &tab.comments;
        if !(c.dirty || !c.sidecar.comments.is_empty()) || !DocComments::available(&tab.doc) {
            return;
        }
        if let Err(e) = tab.comments.write(&tab.doc, path) {
            self.toast_error(format!("Could not save comments: {e}"));
        }
    }

    /// Sidecar paths to watch next to the open documents.
    pub(crate) fn comment_files(&self) -> Vec<PathBuf> {
        self.tabs.iter().filter_map(|t| t.doc.path.as_deref()).map(sidecar_path).collect()
    }

    /// `path` may be the sidecar of an open document. Returns whether it was.
    pub(crate) fn comments_file_changed(&mut self, path: &Path) -> bool {
        let mut found = false;
        let mut kept = false;
        for tab in &mut self.tabs {
            let Some(doc_path) = tab.doc.path.clone() else { continue };
            if sidecar_path(&doc_path) != path {
                continue;
            }
            found = true;
            let before = DiskStamp::read(path);
            if !tab.comments.reload_from_disk(&tab.doc, &doc_path) && tab.comments.dirty && before != tab.comments.disk
            {
                kept = true;
            }
        }
        if kept {
            self.toast("The comments file changed on disk; your unsaved comment changes are kept.");
        }
        found
    }

    /// Keys while a comment box has the keyboard: Esc closes it, and
    /// document shortcuts (undo, formatting) don't reach the document.
    pub(crate) fn comments_key(&mut self, key: &Key, modifiers: Modifiers) -> Option<Task<Message>> {
        if !self.comments.typing || (self.comments.draft.is_none() && self.comments.reply.is_none()) {
            return None;
        }
        if *key == Key::Named(Named::Escape) {
            return Some(if self.comments.reply.is_some() {
                self.on_comments(CommentMsg::ReplyCancel)
            } else {
                self.on_comments(CommentMsg::DraftCancel)
            });
        }
        let chord = crate::keymap::KeyChord::from_event(key, modifiers)?;
        let id = self.keymap.lookup(&chord)?;
        let blocked = match crate::command::find(id).map(|e| &e.command) {
            Some(crate::command::Command::Doc(_)) => true,
            Some(crate::command::Command::App(cmd)) => {
                use crate::command::AppCommand::*;
                matches!(cmd, Undo | Redo | Cut | Copy | Paste | SelectAll | AddComment)
            }
            None => false,
        };
        blocked.then(Task::none)
    }

    /// After every update: move comment ranges with the edits, refresh the
    /// highlights, and hand focus to a box that just opened. Returns the
    /// focus task, if any.
    pub(crate) fn after_comments_update(&mut self) -> Option<Task<Message>> {
        for tab in &mut self.tabs {
            tab.comments.sync(&tab.doc);
        }
        if let Some(id) = self.comments.caret_moved.take() {
            self.comment_at_caret(id);
        }
        let resolved = self.comments.show_resolved;
        for tab in &mut self.tabs {
            let active = self.comments.active.as_ref().filter(|(t, _)| *t == tab.id).map(|(_, id)| id.as_str());
            if let Some(h) = tab.comments.highlights(active, resolved) {
                set_highlights(tab, h);
            }
        }
        if let Some(d) = &self.comments.draft
            && !self.tabs.iter().any(|t| t.id == d.tab)
        {
            self.comments.draft = None;
        }
        if let Some(r) = &self.comments.reply
            && !self.tabs.iter().any(|t| t.id == r.tab)
        {
            self.comments.reply = None;
        }
        let id = self.comments.focus.take()?;
        Some(iced::widget::operation::focus(id))
    }

    /// The caret moved in the document: focus the comment under it, and
    /// give the keyboard back to the document.
    fn comment_at_caret(&mut self, tab_id: TabId) {
        self.comments.typing = false;
        let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) else { return };
        let sel = tab.selection();
        if !sel.is_empty() {
            return;
        }
        if let Some(id) = tab.comments.at(sel.head) {
            self.comments.active = Some((tab_id, id.to_string()));
        } else if self.comments.active_for(tab_id).is_some() {
            self.comments.active = None;
        }
    }
}

fn set_highlights(tab: &mut Tab, h: Vec<Highlight>) {
    tab.view.highlights = h.clone();
    tab.preview.highlights = h.clone();
    tab.editor.highlights = h;
}

/// Whitespace at either end of a selection is not part of the comment.
fn trim_range(buf: &dyn TextBuffer, range: Range<usize>) -> Range<usize> {
    let text = buf.slice(range.clone());
    let start = range.start + (text.len() - text.trim_start().len());
    let end = range.end - (text.len() - text.trim_end().len());
    start..end.max(start)
}

/// `c_` and 8 Crockford base32 chars from the clock and a random hasher
/// seed (SPEC 7 ids look like `c_01JC8Z3Q`).
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    let mut n = h.finish();
    let mut id = String::from("c_");
    for _ in 0..8 {
        id.push(ALPHABET[(n & 31) as usize] as char);
        n >>= 5;
    }
    id
}

/// "just now", "5 min ago", "3 h ago", "2 d ago", or the date.
pub fn ago(ts: jiff::Timestamp) -> String {
    let secs = jiff::Timestamp::now().as_second() - ts.as_second();
    match secs {
        ..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..604_800 => format!("{} d ago", secs / 86_400),
        _ => ts.strftime("%Y-%m-%d").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use mdedit_core::{Edit, EditOrigin, TxnId};

    use super::*;

    fn doc_with_comment(text: &str, quote: &str) -> (Document, DocComments) {
        let doc = Document::from_text(text);
        let start = text.find(quote).unwrap();
        let mut c = DocComments::load(&doc);
        c.sidecar.comments.push(Comment {
            id: "c_1".into(),
            author: "N".into(),
            created: jiff::Timestamp::UNIX_EPOCH,
            resolved: false,
            body: "check".into(),
            anchor: make_anchor(doc.buffer(), start..start + quote.len()),
            replies: Vec::new(),
        });
        c.live.push(Resolution::Attached(start..start + quote.len()));
        (doc, c)
    }

    fn quoted(doc: &Document, c: &DocComments) -> Option<String> {
        c.range_of("c_1").map(|r| doc.buffer().slice(r).into_owned())
    }

    #[test]
    fn ranges_follow_edits_and_undo() {
        let text = "# Title\n\nA sentence worth a comment.\n";
        let (mut doc, mut c) = doc_with_comment(text, "worth");
        doc.apply(&Edit::insert(0, "Intro\n\n", EditOrigin::User, TxnId(1)));
        assert!(c.sync(&doc));
        assert_eq!(quoted(&doc, &c).as_deref(), Some("worth"));
        assert!(!c.sync(&doc));

        let p = doc.buffer().text().find("A sentence").unwrap();
        doc.apply(&Edit::delete(p..p + "A sentence worth a comment.".len(), EditOrigin::User, TxnId(2)));
        c.sync(&doc);
        assert_eq!(quoted(&doc, &c), None);
        doc.undo();
        c.sync(&doc);
        assert_eq!(quoted(&doc, &c).as_deref(), Some("worth"));
    }

    #[test]
    fn external_reload_reanchors() {
        let text = "One remark here.\n\nTwo.\n";
        let (mut doc, mut c) = doc_with_comment(text, "remark");
        c.before_reload(&doc);
        doc.replace_all("Zero.\n\nOne remark here, changed.\n\nTwo.\n", EditOrigin::External);
        c.sync(&doc);
        assert_eq!(quoted(&doc, &c).as_deref(), Some("remark"));
    }

    #[test]
    fn write_load_round_trip_and_external_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        let text = "Some text to discuss.\n";
        std::fs::write(&path, text).unwrap();
        let (mut doc, mut c) = doc_with_comment(text, "discuss");
        doc.path = Some(path.clone());
        c.changed();
        c.write(&doc, &path).unwrap();
        assert!(!c.dirty);
        assert!(sidecar_path(&path).exists());

        let loaded = DocComments::load(&doc);
        assert_eq!(quoted(&doc, &loaded).as_deref(), Some("discuss"));

        // Our own write is not an external change; someone else's is.
        assert!(!c.reload_from_disk(&doc, &path));
        let mut other = Sidecar::load(&path).unwrap();
        other.comments[0].body = "edited elsewhere".into();
        std::thread::sleep(std::time::Duration::from_millis(10));
        other.save(&path).unwrap();
        assert!(c.reload_from_disk(&doc, &path));
        assert_eq!(c.sidecar.comments[0].body, "edited elsewhere");

        // Deleting the last comment deletes the file.
        c.sidecar.comments.clear();
        c.live.remove(0);
        c.write(&doc, &path).unwrap();
        assert!(!sidecar_path(&path).exists());
    }

    #[test]
    fn unreadable_sidecar_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "x\n").unwrap();
        std::fs::write(sidecar_path(&path), "not = [valid").unwrap();
        let doc = mdedit_core::io::load(&path, u64::MAX).unwrap();
        let mut c = DocComments::load(&doc);
        assert!(c.load_error.is_some());
        assert!(c.write(&doc, &path).is_err());
        assert_eq!(std::fs::read_to_string(sidecar_path(&path)).unwrap(), "not = [valid");
    }

    #[test]
    fn highlights_skip_resolved_and_mark_active() {
        let (_, mut c) = doc_with_comment("alpha beta\n", "beta");
        let h = c.highlights(None, false).unwrap();
        assert_eq!(h, vec![Highlight { range: 6..10, kind: HighlightKind::Comment }]);
        assert!(c.highlights(None, false).is_none(), "cached");
        let h = c.highlights(Some("c_1"), false).unwrap();
        assert_eq!(h[0].kind, HighlightKind::ActiveComment);
        c.sidecar.comments[0].resolved = true;
        c.changed();
        assert!(c.highlights(None, false).unwrap().is_empty());
        assert_eq!(c.at(7), None);
    }

    #[test]
    fn trim_and_ids() {
        let doc = Document::from_text("  word \n");
        assert_eq!(trim_range(doc.buffer(), 0..8), 2..6);
        assert_eq!(trim_range(doc.buffer(), 0..2), 2..2);
        let id = new_id();
        assert!(id.starts_with("c_") && id.len() == 10);
        assert_ne!(new_id(), new_id());
    }
}
