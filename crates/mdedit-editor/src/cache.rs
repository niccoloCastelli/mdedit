//! LRU of shaped lines (ARCHITECTURE 4.1), keyed by a hash of the line
//! content, its syntax state, the wrap width and the style generation.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use iced::advanced::graphics::text::Paragraph;

use crate::style::LineKind;

/// About 2000 entries (ARCHITECTURE 4.1 and 10).
pub(crate) const CAPACITY: usize = 2000;

/// A shaped document line.
#[derive(Debug)]
pub(crate) struct ShapedLine {
    pub paragraph: Paragraph,
    pub height: f32,
    pub kind: LineKind,
    /// Byte ranges of inline code, drawn on a `syntax-code-bg` quad.
    pub code_ranges: Vec<Range<usize>>,
}

#[derive(Debug, Default)]
pub(crate) struct LineCache {
    map: HashMap<u64, (Rc<ShapedLine>, u64)>,
    tick: u64,
    hits: u64,
    misses: u64,
}

impl LineCache {
    pub fn get_or_shape(&mut self, key: u64, shape: impl FnOnce() -> ShapedLine) -> Rc<ShapedLine> {
        self.tick += 1;
        if let Some((line, used)) = self.map.get_mut(&key) {
            *used = self.tick;
            self.hits += 1;
            return line.clone();
        }
        self.misses += 1;
        if self.map.len() >= CAPACITY {
            self.evict();
        }
        let line = Rc::new(shape());
        self.map.insert(key, (line.clone(), self.tick));
        line
    }

    /// Drop the least recently used quarter, so eviction cost is amortised
    /// over many insertions.
    fn evict(&mut self) {
        let mut ticks: Vec<u64> = self.map.values().map(|(_, t)| *t).collect();
        let cut = ticks.len() / 4;
        let (_, &mut threshold, _) = ticks.select_nth_unstable(cut);
        self.map.retain(|_, (_, t)| *t > threshold);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy() -> ShapedLine {
        ShapedLine { paragraph: Paragraph::default(), height: 1.0, kind: LineKind::Prose, code_ranges: vec![] }
    }

    #[test]
    fn stays_bounded_and_keeps_recent() {
        let mut cache = LineCache::default();
        for k in 0..10_000u64 {
            cache.get_or_shape(k, dummy);
            // Key 7 is used constantly and must survive eviction.
            cache.get_or_shape(7, dummy);
        }
        assert!(cache.len() <= CAPACITY);
        let misses = cache.stats().1;
        cache.get_or_shape(7, dummy);
        assert_eq!(cache.stats().1, misses);
    }
}
