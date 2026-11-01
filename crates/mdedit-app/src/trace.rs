//! `MDEDIT_TRACE=1`: startup and typing latency for the M1 targets
//! (PLAN 5), printed to stderr. Time to first frame is measured from
//! process start; typing latency from the editor's edit action to the next
//! frame the window draws. `MDEDIT_TRACE_TYPE=n` also types `n`
//! characters itself (through the same path as the editor's edit actions)
//! once the document is shown, then quits: a typing benchmark that needs
//! no input injection.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub fn start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MDEDIT_TRACE").is_some())
}

pub fn auto_type() -> usize {
    std::env::var("MDEDIT_TRACE_TYPE").ok().and_then(|v| v.parse().ok()).unwrap_or(0)
}

#[derive(Debug, Default)]
pub struct Trace {
    /// Synthetic keystrokes still to send.
    pub to_type: usize,
    pub doc_shown: bool,
    first_frame: bool,
    first_doc_frame: bool,
    pending_edit: Option<Instant>,
    keys: Vec<Duration>,
    /// Time spent in `App::update` for each edit (apply, reparse, derived data).
    updates: Vec<Duration>,
}

impl Trace {
    pub fn new(to_type: usize) -> Self {
        Self { to_type, ..Default::default() }
    }

    pub fn edit(&mut self) {
        self.pending_edit.get_or_insert_with(Instant::now);
    }

    /// A frame was drawn; `has_doc` once a loaded document is shown.
    pub fn frame(&mut self, has_doc: bool) {
        if !self.first_frame {
            self.first_frame = true;
            eprintln!("trace: first frame {:?}", start().elapsed());
        }
        if has_doc && !self.first_doc_frame {
            self.first_doc_frame = true;
            self.doc_shown = true;
            eprintln!("trace: first frame with the document {:?}", start().elapsed());
        }
        if let Some(t) = self.pending_edit.take() {
            self.keys.push(t.elapsed());
        }
    }

    pub fn update_took(&mut self, d: Duration) {
        self.updates.push(d);
    }

    pub fn report(&self) {
        if self.keys.is_empty() {
            return;
        }
        let mut k = self.keys.clone();
        k.sort();
        let pct = |p: usize| k[(k.len() * p / 100).min(k.len() - 1)];
        let mut u = self.updates.clone();
        u.sort();
        if !u.is_empty() {
            let p = |q: usize| u[(u.len() * q / 100).min(u.len() - 1)];
            eprintln!("trace: update per edit p50 {:?} p99 {:?} max {:?}", p(50), p(99), u[u.len() - 1]);
        }
        eprintln!(
            "trace: {} edits, edit to frame p50 {:?} p99 {:?} max {:?}",
            k.len(),
            pct(50),
            pct(99),
            k[k.len() - 1]
        );
    }
}
