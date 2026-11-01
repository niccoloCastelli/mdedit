//! One open document: the core `Document` plus the per-tab UI state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use mdedit_core::io::{self, IoError};
use mdedit_core::{Document, Edit, EditOrigin, Selection, TxnId};
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
    next_txn: u64,
}

impl Tab {
    pub fn new(id: TabId, doc: Document, mode: Mode, untitled: usize) -> Self {
        let disk = doc.path.as_deref().and_then(DiskStamp::read);
        let mode = if doc.is_large() { Mode::Edit } else { mode };
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
            next_txn: 1,
        }
    }

    pub fn title(&self) -> String {
        match &self.doc.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
            None => format!("Untitled-{}", self.untitled),
        }
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
        self.mode = mode;
    }

    pub fn set_selection(&mut self, sel: Selection) {
        self.editor.selection = sel;
        self.view.selection = sel;
    }

    pub fn next_txn(&mut self) -> TxnId {
        let t = TxnId(self.next_txn);
        self.next_txn += 1;
        t
    }

    /// Apply an edit and move every cursor through it.
    pub fn apply(&mut self, edit: &Edit) {
        let applied = self.doc.apply(edit);
        self.editor.selection = self.editor.selection.map(&applied);
        self.view.selection = self.view.selection.map(&applied);
        self.preview.selection = self.preview.selection.map(&applied);
    }

    /// Save to the document's path, or `to` (save as).
    pub fn save(&mut self, to: Option<&Path>) -> Result<(), String> {
        let target = to.map(Path::to_path_buf).or_else(|| self.doc.path.clone()).ok_or("no path to save to")?;
        save_document(&mut self.doc, &target)?;
        self.doc.path = Some(target.clone());
        self.doc.mark_saved();
        self.disk = DiskStamp::read(&target);
        self.external = None;
        Ok(())
    }

    /// Replace the buffer with what is on disk now, as one `External` edit,
    /// so the edit log (anchors, caches) sees the reload.
    pub fn reload(&mut self, threshold: u64) -> Result<(), String> {
        let path = self.doc.path.clone().ok_or("untitled buffer")?;
        let fresh = io::load(&path, threshold).map_err(|e| e.to_string())?;
        let text = fresh.buffer().text().into_owned();
        if text != self.doc.buffer().text() {
            let txn = self.next_txn();
            let edit = Edit { range: 0..self.doc.buffer().len_bytes(), text, origin: EditOrigin::External, txn };
            self.apply(&edit);
        }
        self.doc.format = fresh.format;
        self.doc.mark_saved();
        self.disk = DiskStamp::read(&path);
        self.external = None;
        Ok(())
    }
}

/// `io::save`, with a plain atomic write while the core-buffer workstream's
/// version is still the placeholder. The placeholder loader keeps BOM and
/// CRLF in the text, so writing the text back is byte-identical.
fn save_document(doc: &mut Document, path: &Path) -> Result<(), String> {
    match io::save(doc, Some(path)) {
        Ok(()) => Ok(()),
        Err(IoError::Unimplemented(_)) => {
            let text = doc.buffer().text().into_owned();
            crate::settings::write_atomic(path, text.as_bytes()).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Moves a value that is not `Clone` through an iced message (which must
/// be). The receiver takes it once.
#[derive(Clone)]
pub struct Handoff<T>(Arc<Mutex<Option<T>>>);

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
    tokio::task::spawn_blocking(move || io::load(&path, threshold).map(Handoff::new).map_err(|e| e.to_string()))
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
        let txn = tab.next_txn();
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
        tab.reload(u64::MAX).unwrap();
        assert_eq!(tab.doc.buffer().text(), "two lines\nnow\n");
        assert!(!tab.is_dirty());
        assert_eq!(tab.doc.log().len(), 1);
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
