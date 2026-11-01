//! Opt-in latency recording, for checking the SPEC 1 targets (typing p99
//! under 8 ms on a 10 MB file) in a real window. Off by default; recording
//! is a relaxed atomic load when disabled.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static ENABLED: AtomicBool = AtomicBool::new(false);
static SAMPLES: Mutex<Samples> = Mutex::new(Samples { keystroke: Vec::new(), frame: Vec::new() });

struct Samples {
    keystroke: Vec<Duration>,
    frame: Vec<Duration>,
}

pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Time from a key event reaching the widget to the end of drawing the
/// frame that shows its edit (app update, view, layout and draw on the CPU).
pub(crate) fn record_keystroke(d: Duration) {
    if enabled() {
        SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).keystroke.push(d);
    }
}

/// CPU time of the widget's layout + draw for one frame.
pub(crate) fn record_frame(d: Duration) {
    if enabled() {
        SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).frame.push(d);
    }
}

/// Percentile summary of a set of samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub p50: Duration,
    pub p99: Duration,
    pub max: Duration,
}

impl Summary {
    pub fn of(samples: &[Duration]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut s = samples.to_vec();
        s.sort_unstable();
        let pick = |q: f64| s[((s.len() - 1) as f64 * q).round() as usize];
        Some(Self { count: s.len(), p50: pick(0.5), p99: pick(0.99), max: s[s.len() - 1] })
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "n={} p50={:.2?} p99={:.2?} max={:.2?}", self.count, self.p50, self.p99, self.max)
    }
}

/// Take the recorded `(keystroke, frame)` samples, clearing them.
pub fn take() -> (Vec<Duration>, Vec<Duration>) {
    let mut s = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
    (std::mem::take(&mut s.keystroke), std::mem::take(&mut s.frame))
}
