//! Images for View mode (ARCHITECTURE 4.2): decoded off the UI thread,
//! downscaled to display width, kept in an LRU bounded by memory.
//!
//! One cache is shared by every view in the process ([`cache`]). A request
//! for an image not yet decoded starts a decode on a worker thread and
//! reports [`ImageState::Loading`]; views poll [`ImageCache::generation`]
//! to know when to lay out again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use iced::advanced::image::Handle;

/// Default memory budget for decoded pixels (ARCHITECTURE 4.2).
pub const DEFAULT_BUDGET: usize = 64 * 1024 * 1024;

const WORKERS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    modified: Option<SystemTime>,
    /// Width bucket the image was downscaled to (px).
    max_width: u32,
}

#[derive(Debug, Clone)]
pub enum ImageState {
    Loading,
    Ready { handle: Handle, width: u32, height: u32 },
    Failed(String),
}

enum Entry {
    Loading,
    Ready { handle: Handle, width: u32, height: u32, bytes: usize, used: u64 },
    Failed(String),
}

struct Inner {
    entries: HashMap<Key, Entry>,
    bytes: usize,
    budget: usize,
    tick: u64,
    generation: u64,
    jobs: Option<Sender<Key>>,
}

pub struct ImageCache {
    inner: Arc<Mutex<Inner>>,
}

/// The process-wide image cache.
pub fn cache() -> &'static ImageCache {
    static CACHE: OnceLock<ImageCache> = OnceLock::new();
    CACHE.get_or_init(|| ImageCache::new(DEFAULT_BUDGET))
}

impl ImageCache {
    pub fn new(budget: usize) -> Self {
        let inner = Arc::new(Mutex::new(Inner {
            entries: HashMap::new(),
            bytes: 0,
            budget,
            tick: 0,
            generation: 0,
            jobs: None,
        }));
        let (tx, rx) = channel::<Key>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..WORKERS {
            let rx = Arc::clone(&rx);
            let inner = Arc::clone(&inner);
            let spawned =
                std::thread::Builder::new().name(format!("mdedit-image-{i}")).spawn(move || worker(&rx, &inner));
            if spawned.is_err() {
                break;
            }
        }
        inner.lock().unwrap_or_else(|e| e.into_inner()).jobs = Some(tx);
        Self { inner }
    }

    pub fn set_budget(&self, budget: usize) {
        let mut inner = self.lock();
        inner.budget = budget;
        inner.evict(None);
    }

    /// Bumped whenever a decode finishes (or fails).
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Bytes of decoded pixels held.
    pub fn memory(&self) -> usize {
        self.lock().bytes
    }

    pub fn is_loading(&self) -> bool {
        self.lock().entries.values().any(|e| matches!(e, Entry::Loading))
    }

    /// The image at `path` downscaled to at most `max_width` px wide,
    /// starting a decode if needed.
    pub fn get(&self, path: &Path, max_width: u32) -> ImageState {
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        // Round the width up to a 128 px bucket so window resizes reuse work.
        let max_width = max_width.max(1).div_ceil(128) * 128;
        let key = Key { path: path.to_path_buf(), modified, max_width };
        let mut inner = self.lock();
        inner.tick += 1;
        let tick = inner.tick;
        match inner.entries.get_mut(&key) {
            Some(Entry::Ready { handle, width, height, used, .. }) => {
                *used = tick;
                ImageState::Ready { handle: handle.clone(), width: *width, height: *height }
            }
            Some(Entry::Loading) => ImageState::Loading,
            Some(Entry::Failed(e)) => ImageState::Failed(e.clone()),
            None => {
                let sent = inner.jobs.as_ref().is_some_and(|tx| tx.send(key.clone()).is_ok());
                if sent {
                    inner.entries.insert(key, Entry::Loading);
                    ImageState::Loading
                } else {
                    ImageState::Failed("no image worker".into())
                }
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Inner {
    /// Drop least recently used images until under budget, keeping `keep`.
    fn evict(&mut self, keep: Option<&Key>) {
        while self.bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(k, _)| Some(*k) != keep)
                .filter_map(|(k, e)| match e {
                    Entry::Ready { used, .. } => Some((*used, k.clone())),
                    _ => None,
                })
                .min_by_key(|(used, _)| *used);
            let Some((_, key)) = victim else { break };
            if let Some(Entry::Ready { bytes, .. }) = self.entries.remove(&key) {
                self.bytes -= bytes;
            }
        }
    }
}

fn worker(rx: &Mutex<Receiver<Key>>, inner: &Mutex<Inner>) {
    loop {
        let key = match rx.lock() {
            Ok(rx) => match rx.recv() {
                Ok(key) => key,
                Err(_) => return,
            },
            Err(_) => return,
        };
        let result = decode(&key.path, key.max_width);
        let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.generation += 1;
        inner.tick += 1;
        let used = inner.tick;
        let entry = match result {
            Ok((rgba, width, height)) => {
                let bytes = rgba.len();
                inner.bytes += bytes;
                Entry::Ready { handle: Handle::from_rgba(width, height, rgba), width, height, bytes, used }
            }
            Err(e) => Entry::Failed(e),
        };
        inner.entries.insert(key.clone(), entry);
        inner.evict(Some(&key));
    }
}

/// Decode and downscale to at most `max_width` px wide (aspect kept).
pub(crate) fn decode(path: &Path, max_width: u32) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    let img = if img.width() > max_width {
        let h = (img.height() as u64 * max_width as u64 / img.width() as u64).max(1) as u32;
        img.resize_exact(max_width, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

/// Where an image `src` points on disk, relative to the document's folder.
/// `None` for remote URLs (not fetched) and empty sources.
pub fn resolve(src: &str, doc_path: Option<&Path>) -> Option<PathBuf> {
    let src = src.trim();
    if src.is_empty() || src.contains("://") && !src.starts_with("file://") || src.starts_with("data:") {
        return None;
    }
    let src = src.strip_prefix("file://").unwrap_or(src);
    let decoded = percent_decode(src);
    let path = PathBuf::from(decoded);
    if path.is_absolute() {
        return Some(path);
    }
    Some(doc_path.and_then(Path::parent).map_or_else(|| path.clone(), |dir| dir.join(&path)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn write_png(path: &Path, w: u32, h: u32) {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 80, 40, 255]));
        img.save(path).unwrap();
    }

    fn wait_ready(cache: &ImageCache, path: &Path, width: u32) -> ImageState {
        let started = Instant::now();
        loop {
            match cache.get(path, width) {
                ImageState::Loading if started.elapsed() < Duration::from_secs(10) => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                other => return other,
            }
        }
    }

    #[test]
    fn decodes_off_thread_and_downscales() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        write_png(&path, 1000, 500);
        let cache = ImageCache::new(DEFAULT_BUDGET);
        assert!(matches!(cache.get(&path, 256), ImageState::Loading));
        let ImageState::Ready { width, height, .. } = wait_ready(&cache, &path, 256) else { panic!() };
        assert_eq!((width, height), (256, 128));
        assert_eq!(cache.memory(), 256 * 128 * 4);
        assert!(cache.generation() >= 1);
    }

    #[test]
    fn evicts_least_recently_used_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..3).map(|i| dir.path().join(format!("{i}.png"))).collect();
        for p in &paths {
            write_png(p, 128, 128);
        }
        // Room for two 128×128 images.
        let cache = ImageCache::new(2 * 128 * 128 * 4);
        wait_ready(&cache, &paths[0], 128);
        wait_ready(&cache, &paths[1], 128);
        wait_ready(&cache, &paths[0], 128); // touch 0, so 1 is the oldest
        wait_ready(&cache, &paths[2], 128);
        assert_eq!(cache.memory(), 2 * 128 * 128 * 4);
        let inner = cache.lock();
        let ready: Vec<_> = inner
            .entries
            .iter()
            .filter(|(_, e)| matches!(e, Entry::Ready { .. }))
            .map(|(k, _)| k.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(ready.contains(&"0.png".to_string()) && ready.contains(&"2.png".to_string()), "{ready:?}");
    }

    #[test]
    fn missing_file_fails() {
        let cache = ImageCache::new(DEFAULT_BUDGET);
        let path = Path::new("/nonexistent/x.png");
        assert!(matches!(wait_ready(&cache, path, 100), ImageState::Failed(_)));
    }

    #[test]
    fn resolves_relative_to_document() {
        let doc = Path::new("/notes/doc.md");
        assert_eq!(resolve("assets/a%20b.png", Some(doc)), Some(PathBuf::from("/notes/assets/a b.png")));
        assert_eq!(resolve("/abs.png", Some(doc)), Some(PathBuf::from("/abs.png")));
        assert_eq!(resolve("https://x.org/a.png", Some(doc)), None);
    }
}
