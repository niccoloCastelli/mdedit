//! Mermaid diagrams for View mode (ARCHITECTURE 4.4): rendered to SVG off
//! the UI thread by [`mdedit_mermaid`], kept in an LRU bounded by memory.
//!
//! Works like [`crate::images`]: one process-wide cache ([`cache`]); a
//! request for a diagram not rendered yet queues it on a worker thread
//! (started on first use, so documents without diagrams cost nothing) and
//! views poll [`DiagramCache::generation`] to lay out again. While a block
//! is edited its source changes on every key, so lookups name a `slot` (the
//! block) and get the slot's last good diagram to show until the new one is
//! ready, instead of a flashing placeholder.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

use iced::advanced::svg::Handle;
use mdedit_mermaid::Style;

/// Default memory budget for rendered SVG bytes.
pub const DEFAULT_BUDGET: usize = 16 * 1024 * 1024;

/// A rendered diagram ready to draw.
#[derive(Debug, Clone)]
pub struct Ready {
    pub handle: Handle,
    /// Natural size in px at zoom 1.
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub enum DiagramState {
    /// Rendering; `stale` is the slot's previous diagram, if any.
    Loading {
        stale: Option<Ready>,
    },
    Ready(Ready),
    /// The source does not render; `stale` as for `Loading`.
    Failed {
        error: String,
        stale: Option<Ready>,
    },
}

enum Entry {
    Loading,
    Ready { ready: Ready, bytes: usize, used: u64 },
    Failed(String),
}

struct Job {
    key: u64,
    src: String,
    style: Arc<Style>,
}

struct Inner {
    entries: HashMap<u64, Entry>,
    /// Last diagram that rendered for each slot.
    last_good: HashMap<u64, u64>,
    bytes: usize,
    budget: usize,
    tick: u64,
    generation: u64,
    jobs: Option<Sender<Job>>,
}

pub struct DiagramCache {
    inner: Arc<Mutex<Inner>>,
}

/// The process-wide diagram cache.
pub fn cache() -> &'static DiagramCache {
    static CACHE: OnceLock<DiagramCache> = OnceLock::new();
    CACHE.get_or_init(|| DiagramCache::new(DEFAULT_BUDGET))
}

impl DiagramCache {
    pub fn new(budget: usize) -> Self {
        let inner = Inner {
            entries: HashMap::new(),
            last_good: HashMap::new(),
            bytes: 0,
            budget,
            tick: 0,
            generation: 0,
            jobs: None,
        };
        Self { inner: Arc::new(Mutex::new(inner)) }
    }

    /// Bumped whenever a render finishes (or fails).
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Bytes of SVG held.
    pub fn memory(&self) -> usize {
        self.lock().bytes
    }

    pub fn is_loading(&self) -> bool {
        self.lock().entries.values().any(|e| matches!(e, Entry::Loading))
    }

    /// The diagram for `src` in `style`, queueing a render if needed.
    pub fn get(&self, src: &str, style: &Arc<Style>, slot: u64) -> DiagramState {
        let mut h = DefaultHasher::new();
        src.hash(&mut h);
        style.key().hash(&mut h);
        let key = h.finish();
        let mut inner = self.lock();
        inner.tick += 1;
        let tick = inner.tick;
        match inner.entries.get_mut(&key) {
            Some(Entry::Ready { ready, used, .. }) => {
                *used = tick;
                let ready = ready.clone();
                inner.last_good.insert(slot, key);
                DiagramState::Ready(ready)
            }
            Some(Entry::Loading) => DiagramState::Loading { stale: inner.stale(slot) },
            Some(Entry::Failed(e)) => {
                let error = e.clone();
                DiagramState::Failed { error, stale: inner.stale(slot) }
            }
            None => {
                let job = Job { key, src: src.to_string(), style: Arc::clone(style) };
                if inner.queue(&self.inner, job) {
                    inner.entries.insert(key, Entry::Loading);
                    DiagramState::Loading { stale: inner.stale(slot) }
                } else {
                    DiagramState::Failed { error: "no diagram worker".into(), stale: None }
                }
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Inner {
    fn stale(&self, slot: u64) -> Option<Ready> {
        match self.entries.get(self.last_good.get(&slot)?) {
            Some(Entry::Ready { ready, .. }) => Some(ready.clone()),
            _ => None,
        }
    }

    /// Send a job, starting the worker on first use.
    fn queue(&mut self, shared: &Arc<Mutex<Inner>>, job: Job) -> bool {
        if self.jobs.is_none() {
            let (tx, rx) = channel::<Job>();
            let shared = Arc::clone(shared);
            let spawned = std::thread::Builder::new().name("mdedit-mermaid".into()).spawn(move || worker(&rx, &shared));
            if spawned.is_err() {
                return false;
            }
            self.jobs = Some(tx);
        }
        self.jobs.as_ref().is_some_and(|tx| tx.send(job).is_ok())
    }

    /// Drop least recently used diagrams until under budget, keeping `keep`.
    /// Failures are tiny but unbounded in number, so they go too once the
    /// map is large.
    fn evict(&mut self, keep: u64) {
        while self.bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(k, _)| **k != keep)
                .filter_map(|(k, e)| match e {
                    Entry::Ready { used, .. } => Some((*used, *k)),
                    _ => None,
                })
                .min_by_key(|(used, _)| *used);
            let Some((_, key)) = victim else { break };
            if let Some(Entry::Ready { bytes, .. }) = self.entries.remove(&key) {
                self.bytes -= bytes;
            }
        }
        if self.entries.len() > 4096 {
            self.entries.retain(|k, e| *k == keep || !matches!(e, Entry::Failed(_)));
        }
        let entries = &self.entries;
        self.last_good.retain(|_, k| entries.contains_key(k));
    }
}

fn worker(rx: &Receiver<Job>, inner: &Mutex<Inner>) {
    while let Ok(job) = rx.recv() {
        // Typing in a diagram queues one job per key; only the latest
        // version of a block matters, but the others are cheap enough to
        // finish (and may be typed again).
        let result = mdedit_mermaid::render(&job.src, &job.style);
        let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.generation += 1;
        inner.tick += 1;
        let used = inner.tick;
        let entry = match result {
            Ok(d) => {
                let bytes = d.svg.len();
                inner.bytes += bytes;
                let ready = Ready { handle: Handle::from_memory(d.svg), width: d.width, height: d.height };
                Entry::Ready { ready, bytes, used }
            }
            Err(e) => Entry::Failed(e.to_string()),
        };
        inner.entries.insert(job.key, entry);
        inner.evict(job.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(cache: &DiagramCache, src: &str, style: &Arc<Style>, slot: u64) -> DiagramState {
        let started = Instant::now();
        loop {
            match cache.get(src, style, slot) {
                DiagramState::Loading { .. } if started.elapsed() < Duration::from_secs(20) => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                other => return other,
            }
        }
    }

    fn style() -> Arc<Style> {
        Arc::new(Style::from_theme(&mdedit_theme::Theme::ember_light()))
    }

    #[test]
    fn renders_off_thread_and_keeps_the_last_good_diagram_per_slot() {
        let cache = DiagramCache::new(DEFAULT_BUDGET);
        let style = style();
        assert!(matches!(cache.get("flowchart LR\n A --> B", &style, 1), DiagramState::Loading { stale: None }));
        let DiagramState::Ready(first) = wait(&cache, "flowchart LR\n A --> B", &style, 1) else { panic!() };
        assert!(first.width > 0.0 && cache.memory() > 0 && cache.generation() >= 1);
        // Half-typed: the slot's previous diagram stands in.
        let DiagramState::Failed { error, stale: Some(stale) } = wait(&cache, "flowchart LR\n A -->", &style, 1) else {
            panic!()
        };
        assert!(!error.is_empty());
        assert_eq!(stale.width, first.width);
        // Another slot has no stand-in.
        assert!(matches!(wait(&cache, "flowchart LR\n A -->", &style, 2), DiagramState::Failed { stale: None, .. }));
    }

    #[test]
    fn evicts_over_budget() {
        let cache = DiagramCache::new(1);
        let style = style();
        wait(&cache, "flowchart LR\n A --> B", &style, 1);
        wait(&cache, "flowchart LR\n C --> D", &style, 1);
        let inner = cache.lock();
        let ready = inner.entries.values().filter(|e| matches!(e, Entry::Ready { .. })).count();
        assert_eq!(ready, 1, "only the newest diagram is kept");
    }
}
