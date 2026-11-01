//! One open document: the core `Document` plus the per-tab UI state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use mdedit_core::document::SavePoint;
use mdedit_core::io::{self, FileFormat};
use mdedit_core::mapped::Remap;
use mdedit_core::{Document, Edit, EditOrigin, Selection, TextBuffer};
use mdedit_editor::EditorState;
use mdedit_view::ViewState;

use crate::settings::Mode;

pub type TabId = u64;

/// What we last saw on disk, to tell our own saves from external changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

impl DiskStamp {
    pub fn read(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self { modified: m.modified().ok(), len: m.len() })
    }
}

/// The file changed on disk while the buffer had unsaved edits
/// (SPEC 3): the banner offers reload / keep mine / diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum External {
    Changed,
    Deleted,
}

pub struct Tab {
    pub id: TabId,
    pub doc: Document,
    pub editor: EditorState,
    pub view: ViewState,
    pub preview: ViewState,
    pub mode: Mode,
    pub disk: Option<DiskStamp>,
    pub external: Option<External>,
    /// "Untitled-N" number for buffers without a path.
    pub untitled: usize,
    /// A background save is running (DESIGN_SYSTEM 8.6: never block typing).
    pub saving: bool,
    /// Smart input replaced a typed edit: drop the widget's caret update
    /// that follows it (its position is for the plain edit).
    pub skip_select: bool,
    /// The table the cursor is in, to auto-align it when the cursor leaves
    /// (DESIGN_SYSTEM 8.4).
    pub table: Option<std::ops::Range<usize>>,
    /// The sidecar's comments and their live ranges (`comments.rs`).
    pub comments: crate::comments::DocComments,
}

impl Tab {
    pub fn new(id: TabId, doc: Document, mode: Mode, untitled: usize) -> Self {
        let disk = doc.path.as_deref().and_then(DiskStamp::read);
        let mode = if doc.is_large() { Mode::Edit } else { mode };
        let comments = crate::comments::DocComments::load(&doc);
        Self {
            id,
            doc,
            editor: EditorState::default(),
            view: ViewState::default(),
            preview: ViewState { read_only: true, ..ViewState::default() },
            mode,
            disk,
            external: None,
            untitled,
            saving: false,
            skip_select: false,
            table: None,
            comments,
        }
    }

    pub fn title(&self) -> String {
        match &self.doc.path {
            Some(p) => {
                p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
            }
            None => format!("Untitled-{}", self.untitled),
        }
    }

    /// Not valid UTF-8 (for large files, known once indexing has scanned it).
    pub fn read_only(&self) -> bool {
        self.doc.format.read_only || self.doc.mapped().and_then(|m| m.utf8_valid()) == Some(false)
    }

    /// Whether edits are accepted: not a read-only file, and not View mode
    /// switched to view only.
    pub fn editable(&self) -> bool {
        !self.read_only() && !(self.mode == Mode::View && self.view.read_only)
    }

    pub fn is_dirty(&self) -> bool {
        self.doc.is_dirty()
    }

    /// The selection of whichever pane has the cursor in this mode.
    pub fn selection(&self) -> Selection {
        match self.mode {
            Mode::View => self.view.selection,
            Mode::Edit | Mode::Split => self.editor.selection,
        }
    }

    /// Keep the cursor and selection across mode switches (DESIGN_SYSTEM 8.1).
    pub fn set_mode(&mut self, mode: Mode) {
        if self.doc.is_large() && mode != Mode::Edit {
            return;
        }
        let sel = self.selection();
        self.editor.selection = sel;
        self.view.selection = sel;
        // View mode opens where the editor was: the editor reports its top
        // line after every scroll, and the preview keeps the latest one.
        if mode == Mode::View && self.mode != Mode::View {
            self.view.sync = self.preview.sync;
        }
        self.mode = mode;
    }

    pub fn set_selection(&mut self, sel: Selection) {
        self.editor.selection = sel;
        self.view.selection = sel;
    }

    /// Apply a command's edits as one undo step and take its selection.
    pub fn apply_command(&mut self, result: mdedit_core::command::CommandResult) {
        if !result.edits.is_empty() {
            let txn = self.doc.next_txn();
            for edit in result.edits {
                self.apply(&Edit { txn, origin: EditOrigin::Command, ..edit });
            }
        }
        self.set_selection(result.selection);
    }

    /// Apply an edit and move every cursor through it.
    pub fn apply(&mut self, edit: &Edit) {
        let applied = self.doc.apply(edit);
        self.editor.selection = self.editor.selection.map(&applied);
        self.view.selection = self.view.selection.map(&applied);
        self.preview.selection = self.preview.selection.map(&applied);
    }

    /// Blocking save to the document's path, or `to` (save as).
    #[cfg(test)]
    pub fn save(&mut self, to: Option<&Path>) -> Result<(), String> {
        let job = self.begin_save(to)?;
        let path = job.path.clone();
        let point = job.point;
        let remap = job.run()?;
        self.finish_save(path, point, remap);
        Ok(())
    }

    /// Snapshot the document for a background save.
    pub fn begin_save(&mut self, to: Option<&Path>) -> Result<SaveJob, String> {
        if self.read_only() {
            return Err(io::IoError::ReadOnly.to_string());
        }
        let path = to.map(Path::to_path_buf).or_else(|| self.doc.path.clone()).ok_or("no path to save to")?;
        let point = self.doc.begin_save();
        self.saving = true;
        Ok(SaveJob { path, point, snapshot: self.doc.buffer().snapshot(), format: self.doc.format })
    }

    /// The snapshot taken at `point` is on disk at `path`.
    pub fn finish_save(&mut self, path: PathBuf, point: SavePoint, remap: Option<Remap>) {
        self.saving = false;
        if let Some(remap) = remap {
            // Point the map at the new file; a no-op if edits came in since.
            let _ = self.doc.remap(remap);
        }
        self.doc.mark_saved_at(point);
        self.disk = DiskStamp::read(&path);
        self.doc.path = Some(path);
        self.external = None;
    }

    /// Replace the buffer with what is on disk now, as one `External` edit,
    /// so the edit log (anchors, caches) and undo see the reload.
    pub fn reload(&mut self, threshold: u64) -> Result<(), String> {
        let path = self.doc.path.clone().ok_or("untitled buffer")?;
        let fresh = io::load(&path, threshold).map_err(|e| e.to_string())?;
        let text = fresh.buffer().text();
        if text != self.doc.buffer().text() {
            self.comments.before_reload(&self.doc);
            // Mapping through a whole-document replace would push every
            // cursor to the end; keep the offsets instead, clamped.
            let keep = |s: Selection, len: usize| Selection { anchor: s.anchor.min(len), head: s.head.min(len) };
            self.doc.replace_all(&text, EditOrigin::External);
            let len = self.doc.buffer().len_bytes();
            let fix = |s: Selection| {
                let s = keep(s, len);
                Selection { anchor: char_floor(&text, s.anchor), head: char_floor(&text, s.head) }
            };
            self.editor.selection = fix(self.editor.selection);
            self.view.selection = fix(self.view.selection);
            self.preview.selection = fix(self.preview.selection);
        }
        self.doc.format = fresh.format;
        self.doc.mark_saved();
        self.disk = DiskStamp::read(&path);
        self.external = None;
        Ok(())
    }
}

fn char_floor(text: &str, mut i: usize) -> usize {
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// A save running on a worker.
pub struct SaveJob {
    pub path: PathBuf,
    pub point: SavePoint,
    snapshot: std::sync::Arc<dyn TextBuffer>,
    format: FileFormat,
}

impl SaveJob {
    /// Write the snapshot. A large (mapped) document streams its pieces and
    /// returns the [`Remap`] to hand to `Document::remap`.
    pub fn run(&self) -> Result<Option<Remap>, String> {
        match self.snapshot.as_mapped() {
            Some(m) => m.save_to(&self.path, self.format).map(Some),
            None => io::write_atomic(&self.path, self.snapshot.as_ref(), self.format).map(|()| None),
        }
        .map_err(|e| e.to_string())
    }
}

/// Moves a value that is not `Clone` through an iced message (which must
/// be). The receiver takes it once.
pub struct Handoff<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Handoff<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Handoff<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    pub fn take(&self) -> Option<T> {
        self.0.lock().ok()?.take()
    }
}

impl<T> std::fmt::Debug for Handoff<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Handoff(..)")
    }
}

/// Load a file off the UI thread.
pub async fn load(path: PathBuf, threshold: u64) -> Result<Handoff<Document>, String> {
    tokio::task::spawn_blocking(move || {
        io::load_deferred(&path, threshold).map(Handoff::new).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_for(path: &Path) -> Tab {
        Tab::new(1, io::load(path, u64::MAX).unwrap(), Mode::Edit, 0)
    }

    #[test]
    fn unedited_save_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        let bytes = "\u{feff}# Title\r\n\r\nBody with é\r\n".as_bytes();
        std::fs::write(&path, bytes).unwrap();
        let mut tab = tab_for(&path);
        assert!(!tab.is_dirty());
        tab.save(None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn edit_save_as_and_dirty_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "hello\n").unwrap();
        let mut tab = tab_for(&path);
        tab.set_selection(Selection::cursor(5));
        let txn = tab.doc.next_txn();
        tab.apply(&Edit::insert(5, " world", EditOrigin::User, txn));
        assert!(tab.is_dirty());
        assert_eq!(tab.editor.selection, Selection::cursor(11));
        let other = dir.path().join("b.md");
        tab.save(Some(&other)).unwrap();
        assert!(!tab.is_dirty());
        assert_eq!(tab.title(), "b.md");
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "hello world\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[test]
    fn reload_replaces_text_through_the_edit_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one\n").unwrap();
        let mut tab = tab_for(&path);
        std::fs::write(&path, "two lines\nnow\n").unwrap();
        tab.set_selection(Selection::cursor(2));
        tab.reload(u64::MAX).unwrap();
        assert_eq!(tab.doc.buffer().text(), "two lines\nnow\n");
        assert_eq!(tab.selection(), Selection::cursor(2));
        assert!(!tab.is_dirty());
        tab.doc.undo();
        assert_eq!(tab.doc.buffer().text(), "one\n");
    }

    #[test]
    fn mode_switch_keeps_selection() {
        let mut tab = Tab::new(1, Document::from_text("abc def"), Mode::Edit, 1);
        tab.editor.selection = Selection { anchor: 1, head: 4 };
        tab.set_mode(Mode::View);
        assert_eq!(tab.selection(), Selection { anchor: 1, head: 4 });
        tab.view.selection = Selection::cursor(6);
        tab.set_mode(Mode::Split);
        assert_eq!(tab.editor.selection, Selection::cursor(6));
    }
}

/// M1 numbers on a 10 MB document, without a window (open and edit apply;
/// first frame and widget time are measured in the running app).
/// `cargo test --release -p mdedit-app -- --ignored --nocapture m1_`
#[cfg(test)]
mod perf {
    use std::time::Instant;

    use super::*;

    #[test]
    #[ignore]
    fn m1_open_and_type_10mb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.md");
        let mut text = String::new();
        let mut i = 0;
        while text.len() < 10 * 1024 * 1024 {
            text.push_str(&format!(
                "## Section {i}\n\nSome *prose* with a [link](http://x.y) and `code`, line {i}.\n\n- item\n- item\n\n"
            ));
            i += 1;
        }
        std::fs::write(&path, &text).unwrap();

        let t = Instant::now();
        let mut doc = io::load_deferred(&path, u64::MAX).unwrap();
        let open = t.elapsed();
        let t = Instant::now();
        while let Some(job) = doc.take_parse_job() {
            doc.finish_parse(job.run());
        }
        let parse = t.elapsed();
        let mut tab = Tab::new(1, doc, Mode::Edit, 0);

        let mut times = Vec::new();
        let at = tab.doc.buffer().len_bytes() / 2;
        for k in 0..200 {
            let t = Instant::now();
            tab.apply(&Edit::insert(at + k, "x", EditOrigin::User, mdedit_core::TxnId(0)));
            while let Some(job) = tab.doc.take_parse_job() {
                tab.doc.finish_parse(job.run());
            }
            times.push(t.elapsed());
        }
        times.sort();
        let p99 = times[times.len() * 99 / 100];
        let t = Instant::now();
        tab.save(Some(&dir.path().join("copy.md"))).unwrap();
        let save = t.elapsed();
        eprintln!(
            "10 MB: load {open:?} (parse on worker {parse:?}), apply p50 {:?} p99 {p99:?}, save {save:?}",
            times[times.len() / 2]
        );
    }
}
