//! UI-free document core for mdedit.
//!
//! Everything a front end (the iced app today, an MCP server later) needs to
//! open, edit, parse, search and save a markdown document. Nothing in here
//! depends on a GUI toolkit. See `docs/ARCHITECTURE.md` sections 2, 3 and 5.
//!
//! The types re-exported at the crate root are the **shared interface** the
//! other crates build against. Changing their shape affects several
//! workstreams; see "Shared interfaces" in `docs/PLAN.md`.

pub mod buffer;
pub mod command;
pub mod document;
pub mod edit;
pub mod edit_log;
pub mod highlight;
pub mod io;
pub mod mapped;
pub mod parse;
pub mod search;
pub mod selection;
pub mod undo;

pub use buffer::{RopeBuffer, TextBuffer};
pub use document::Document;
pub use edit::{AppliedEdit, Edit, EditOrigin, TxnId};
pub use edit_log::{EditListener, EditLog};
pub use parse::{Block, BlockIndex, BlockKind};
pub use selection::Selection;
