//! Line heights with O(log n) offset lookups (ARCHITECTURE 4.1).
//!
//! Lines are grouped in chunks of up to [`CHUNK_MAX`] lines; two Fenwick
//! trees over the chunks hold their line counts and summed heights. Finding
//! the line at a scroll offset, or the offset of a line, is a Fenwick search
//! plus a scan inside one chunk; inserting or removing lines only touches
//! one chunk (the chunk-level trees are rebuilt when chunks split or merge,
//! which is rare and costs O(n / CHUNK_MAX)).
//!
//! Every line height is either an *estimate* (not shaped yet) or *measured*.
//! Estimates are stored negated so the flag costs no memory.

/// Lines per chunk before it splits.
const CHUNK_MAX: usize = 1024;
/// Chunks smaller than this merge with a neighbour.
const CHUNK_MIN: usize = 128;

#[derive(Debug, Clone, Default)]
struct Chunk {
    /// Per-line height; negative values are estimates of `-h`.
    heights: Vec<f32>,
    sum: f64,
}

impl Chunk {
    fn from_heights(heights: Vec<f32>) -> Self {
        let sum = heights.iter().map(|h| h.abs() as f64).sum();
        Self { heights, sum }
    }
}

/// Fenwick (binary indexed) tree over `f64` values.
#[derive(Debug, Clone, Default)]
struct Fenwick {
    tree: Vec<f64>,
}

impl Fenwick {
    fn build(values: impl Iterator<Item = f64>) -> Self {
        let mut tree: Vec<f64> = values.collect();
        let n = tree.len();
        for i in 0..n {
            let parent = i | (i + 1);
            if parent < n {
                tree[parent] += tree[i];
            }
        }
        Self { tree }
    }

    fn add(&mut self, mut i: usize, delta: f64) {
        while i < self.tree.len() {
            self.tree[i] += delta;
            i |= i + 1;
        }
    }

    /// Sum of values `0..i`.
    fn prefix(&self, mut i: usize) -> f64 {
        let mut sum = 0.0;
        while i > 0 {
            sum += self.tree[i - 1];
            i &= i - 1;
        }
        sum
    }

    /// The index `i` whose cumulative range contains `target`, i.e. the
    /// largest `i` with `prefix(i) <= target`, together with `prefix(i)`.
    /// Clamped to `len - 1`.
    fn search(&self, target: f64) -> (usize, f64) {
        let n = self.tree.len();
        let mut pos = 0;
        let mut acc = 0.0;
        let mut step = n.next_power_of_two();
        while step > 0 {
            let next = pos + step;
            if next <= n && acc + self.tree[next - 1] <= target {
                pos = next;
                acc += self.tree[next - 1];
            }
            step >>= 1;
        }
        if pos >= n && n > 0 {
            let last = n - 1;
            return (last, self.prefix(last));
        }
        (pos, acc)
    }
}

/// Heights of every document line.
#[derive(Debug, Clone)]
pub struct HeightMap {
    chunks: Vec<Chunk>,
    heights: Fenwick,
    counts: Fenwick,
    len: usize,
}

impl Default for HeightMap {
    fn default() -> Self {
        Self::new(std::iter::empty())
    }
}

impl HeightMap {
    /// A map of estimated heights.
    pub fn new(estimates: impl IntoIterator<Item = f32>) -> Self {
        let mut chunks = Vec::new();
        let mut current = Vec::with_capacity(CHUNK_MAX / 2);
        for h in estimates {
            current.push(-h.max(0.0));
            if current.len() == CHUNK_MAX / 2 {
                chunks.push(Chunk::from_heights(std::mem::take(&mut current)));
            }
        }
        if !current.is_empty() || chunks.is_empty() {
            chunks.push(Chunk::from_heights(current));
        }
        let mut map = Self { chunks, heights: Fenwick::default(), counts: Fenwick::default(), len: 0 };
        map.rebuild();
        map
    }

    fn rebuild(&mut self) {
        self.heights = Fenwick::build(self.chunks.iter().map(|c| c.sum));
        self.counts = Fenwick::build(self.chunks.iter().map(|c| c.heights.len() as f64));
        self.len = self.chunks.iter().map(|c| c.heights.len()).sum();
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn total_height(&self) -> f64 {
        self.heights.prefix(self.chunks.len())
    }

    /// Chunk holding `line` and the line index where that chunk starts.
    fn locate(&self, line: usize) -> (usize, usize) {
        let (chunk, start) = self.counts.search(line as f64);
        (chunk, start as usize)
    }

    pub fn height(&self, line: usize) -> f32 {
        let (c, start) = self.locate(line);
        self.chunks[c].heights[line - start].abs()
    }

    pub fn is_measured(&self, line: usize) -> bool {
        let (c, start) = self.locate(line);
        self.chunks[c].heights[line - start] >= 0.0
    }

    /// Distance from the top of the document to the top of `line`
    /// (`line == len` gives the total height).
    pub fn offset_of(&self, line: usize) -> f64 {
        if line >= self.len {
            return self.total_height();
        }
        let (c, start) = self.locate(line);
        let before: f64 = self.chunks[c].heights[..line - start].iter().map(|h| h.abs() as f64).sum();
        self.heights.prefix(c) + before
    }

    /// The line at offset `y` and the offset of its top. Offsets past the end
    /// give the last line.
    pub fn line_at(&self, y: f64) -> (usize, f64) {
        if self.len == 0 {
            return (0, 0.0);
        }
        let y = y.max(0.0);
        let (c, mut acc) = self.heights.search(y);
        let line_start = self.counts.prefix(c) as usize;
        let chunk = &self.chunks[c];
        for (i, h) in chunk.heights.iter().enumerate() {
            let h = h.abs() as f64;
            if acc + h > y || i + 1 == chunk.heights.len() {
                return (line_start + i, acc);
            }
            acc += h;
        }
        // Empty chunk (only possible for an empty map).
        (line_start, acc)
    }

    /// Record a measured height. Returns the change in total height.
    pub fn set_measured(&mut self, line: usize, height: f32) -> f32 {
        self.set_raw(line, height.max(0.0))
    }

    /// Replace a line's height with an estimate (e.g. after its text changed
    /// off-screen). Returns the change in total height.
    pub fn set_estimate(&mut self, line: usize, height: f32) -> f32 {
        self.set_raw(line, -height.max(0.0))
    }

    fn set_raw(&mut self, line: usize, raw: f32) -> f32 {
        let (c, start) = self.locate(line);
        let slot = &mut self.chunks[c].heights[line - start];
        let delta = raw.abs() - slot.abs();
        *slot = raw;
        if delta != 0.0 {
            self.chunks[c].sum += delta as f64;
            self.heights.add(c, delta as f64);
        }
        delta
    }

    /// Replace lines `range` with `estimates` (all unmeasured). Used when an
    /// edit adds or removes lines.
    pub fn splice(&mut self, range: std::ops::Range<usize>, estimates: impl IntoIterator<Item = f32>) {
        let new: Vec<f32> = estimates.into_iter().map(|h| -h.max(0.0)).collect();
        let range = range.start.min(self.len)..range.end.min(self.len);
        // Fast path: everything inside one chunk.
        let (c, start) = self.locate(range.start.min(self.len.saturating_sub(1)));
        let chunk_len = self.chunks[c].heights.len();
        let fits = range.start >= start && range.end <= start + chunk_len;
        if fits {
            let chunk = &mut self.chunks[c];
            let local = range.start - start..range.end - start;
            let removed: f64 = chunk.heights[local.clone()].iter().map(|h| h.abs() as f64).sum();
            let added: f64 = new.iter().map(|h| h.abs() as f64).sum();
            let count_delta = new.len() as f64 - local.len() as f64;
            chunk.heights.splice(local, new);
            chunk.sum += added - removed;
            let n = chunk.heights.len();
            if (CHUNK_MIN..=CHUNK_MAX).contains(&n) || (self.chunks.len() == 1 && n <= CHUNK_MAX) {
                self.heights.add(c, added - removed);
                self.counts.add(c, count_delta);
                self.len = (self.len as f64 + count_delta) as usize;
                return;
            }
            self.rebalance(c);
            return;
        }
        // Slow path: flatten the affected chunks.
        let (c_end, _) = self.locate(range.end.saturating_sub(1).max(range.start).min(self.len - 1));
        let first_start = start;
        let mut flat: Vec<f32> = self.chunks[c..=c_end].iter().flat_map(|ch| ch.heights.iter().copied()).collect();
        flat.splice(range.start - first_start..range.end - first_start, new);
        let replacement: Vec<Chunk> = flat.chunks(CHUNK_MAX / 2).map(|s| Chunk::from_heights(s.to_vec())).collect();
        self.chunks.splice(c..=c_end, replacement);
        if self.chunks.is_empty() {
            self.chunks.push(Chunk::default());
        }
        self.rebuild();
    }

    /// Split or merge chunk `c` after it grew or shrank out of bounds.
    fn rebalance(&mut self, c: usize) {
        let n = self.chunks[c].heights.len();
        if n > CHUNK_MAX {
            let chunk = self.chunks.remove(c);
            let parts: Vec<Chunk> = chunk.heights.chunks(CHUNK_MAX / 2).map(|s| Chunk::from_heights(s.to_vec())).collect();
            self.chunks.splice(c..c, parts);
        } else if self.chunks.len() > 1 {
            let other = if c + 1 < self.chunks.len() { c + 1 } else { c - 1 };
            let (a, b) = (c.min(other), c.max(other));
            let second = self.chunks.remove(b);
            let mut heights = std::mem::take(&mut self.chunks[a].heights);
            heights.extend(second.heights);
            if heights.len() > CHUNK_MAX {
                let parts: Vec<Chunk> = heights.chunks(CHUNK_MAX / 2).map(|s| Chunk::from_heights(s.to_vec())).collect();
                self.chunks.splice(a..=a, parts);
            } else {
                self.chunks[a] = Chunk::from_heights(heights);
            }
        }
        self.rebuild();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_offset(m: &[f32], line: usize) -> f64 {
        m[..line].iter().map(|&h| h as f64).sum()
    }

    #[test]
    fn offsets_and_lookup() {
        let map = HeightMap::new((0..5000).map(|i| (i % 3 + 1) as f32 * 10.0));
        let model: Vec<f32> = (0..5000).map(|i| (i % 3 + 1) as f32 * 10.0).collect();
        assert_eq!(map.len(), 5000);
        for line in [0, 1, 511, 512, 513, 2000, 4999] {
            assert_eq!(map.offset_of(line), model_offset(&model, line));
            let y = model_offset(&model, line) + 5.0;
            assert_eq!(map.line_at(y), (line, model_offset(&model, line)));
        }
        assert_eq!(map.line_at(1e12).0, 4999);
        assert_eq!(map.line_at(-3.0), (0, 0.0));
    }

    #[test]
    fn measure_and_estimate_flags() {
        let mut map = HeightMap::new([10.0; 10]);
        assert!(!map.is_measured(3));
        assert_eq!(map.set_measured(3, 27.0), 17.0);
        assert!(map.is_measured(3));
        assert_eq!(map.height(3), 27.0);
        assert_eq!(map.total_height(), 117.0);
        map.set_estimate(3, 10.0);
        assert!(!map.is_measured(3));
        assert_eq!(map.total_height(), 100.0);
    }

    #[test]
    fn random_splices_match_model() {
        // Small deterministic LCG; no extra dev-dependency needed.
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut rand = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n.max(1) as u64) as usize
        };
        let mut model: Vec<f32> = (0..3000).map(|i| (i % 5) as f32 + 1.0).collect();
        let mut map = HeightMap::new(model.iter().copied());
        for _ in 0..2000 {
            let start = rand(model.len() + 1);
            let end = (start + rand(if rand(10) == 0 { 2500 } else { 4 })).min(model.len());
            let new: Vec<f32> = (0..rand(if rand(10) == 0 { 1500 } else { 4 })).map(|_| rand(30) as f32).collect();
            model.splice(start..end, new.iter().copied());
            map.splice(start..end, new);
            if !model.is_empty() {
                let l = rand(model.len());
                let h = rand(50) as f32;
                model[l] = h;
                map.set_measured(l, h);
            }
            assert_eq!(map.len(), model.len());
        }
        let total: f64 = model.iter().map(|&h| h as f64).sum();
        assert!((map.total_height() - total).abs() < 1e-6);
        for line in (0..model.len()).step_by(37) {
            assert!((map.offset_of(line) - model_offset(&model, line)).abs() < 1e-6);
            if model[line] > 0.0 {
                assert_eq!(map.line_at(model_offset(&model, line) + 0.01).0, line);
            }
        }
    }
}
