//! Multi-resolution min/max overview for waveform drawing.
//!
//! Level 0 holds the (min, max) of each base block (normally [`BLOCK`] samples); each following level merges
//! [`FANOUT`] blocks of the level below, until a level has a single block. Range queries combine
//! the coarsest whole blocks that fit inside the range, so a query costs O(log n) regardless of
//! zoom.

/// Samples per level-0 block.
pub const BLOCK: usize = 32;
/// Blocks of level `n` merged into one block of level `n + 1`.
pub const FANOUT: usize = 4;
/// Upper bound on the number of columns [`Peaks::columns`] will produce.
pub const MAX_COLUMNS: usize = 1 << 20;

/// Multi-resolution min/max overview. Resident builds use blocks of 32 samples; bounded builds
/// may use larger blocks. Each parent level merges four blocks.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Peaks {
    len: usize,
    #[serde(default = "default_block")]
    block: usize,
    levels: Vec<Vec<(f32, f32)>>,
}

const fn default_block() -> usize {
    BLOCK
}

const EMPTY: (f32, f32) = (f32::INFINITY, f32::NEG_INFINITY);

fn merge(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0.min(b.0), a.1.max(b.1))
}

fn finish(acc: (f32, f32)) -> (f32, f32) {
    if acc.0 > acc.1 { (0.0, 0.0) } else { acc }
}

impl Peaks {
    /// Build the overview of `samples` (one channel). NaN samples are ignored.
    pub fn build(samples: &[f32]) -> Peaks {
        let mut levels = Vec::new();
        let level0: Vec<(f32, f32)> = samples.chunks(BLOCK).map(|c| finish(c.iter().fold(EMPTY, |acc, &s| merge(acc, (s, s))))).collect();
        if level0.is_empty() {
            return Peaks { len: 0, block: BLOCK, levels };
        }
        levels.push(level0);
        while let Some(last) = levels.last() {
            if last.len() <= 1 {
                break;
            }
            let next: Vec<(f32, f32)> = last.chunks(FANOUT).map(|c| c.iter().fold(EMPTY, |acc, &b| merge(acc, b))).collect();
            levels.push(next);
        }
        Peaks { len: samples.len(), block: BLOCK, levels }
    }

    /// Number of samples the overview was built from.
    pub fn len_samples(&self) -> usize {
        self.len
    }

    /// Finest overview resolution; bounded disk overviews may be coarser than resident ones.
    pub fn resolution_samples(&self) -> usize {
        self.block.max(1)
    }

    /// Number of resolution levels (0 for an empty overview).
    pub fn num_levels(&self) -> usize {
        self.levels.len()
    }

    /// (min, max) over the sample range `[start, end)`, clamped to the data. Exact when both ends
    /// are multiples of `resolution_samples()` (or `end` is the data length); otherwise partial edge blocks
    /// contribute their whole block's extremes, so the result always contains the true range.
    /// Returns `(0.0, 0.0)` for an empty range.
    pub fn minmax(&self, start: usize, end: usize) -> (f32, f32) {
        let end = end.min(self.len);
        if start >= end {
            return (0.0, 0.0);
        }
        let block = self.block.max(1);
        let mut lo = start / block;
        let mut hi = end.div_ceil(block);
        let mut acc = EMPTY;
        let mut lvl = 0usize;
        while lo < hi {
            let Some(level) = self.levels.get(lvl) else { break };
            let has_parent = lvl + 1 < self.levels.len();
            if !has_parent || hi - lo < 2 * FANOUT {
                for b in level.get(lo..hi.min(level.len())).unwrap_or_default() {
                    acc = merge(acc, *b);
                }
                break;
            }
            while !lo.is_multiple_of(FANOUT) && lo < hi {
                acc = merge(acc, level.get(lo).copied().unwrap_or(EMPTY));
                lo += 1;
            }
            while !hi.is_multiple_of(FANOUT) && hi > lo {
                hi -= 1;
                acc = merge(acc, level.get(hi).copied().unwrap_or(EMPTY));
            }
            lo /= FANOUT;
            hi /= FANOUT;
            lvl += 1;
        }
        finish(acc)
    }

    /// Per-column (min, max) for drawing `cols` pixel columns spanning the sample range
    /// `[start, end)`. Columns that fall outside the data are `(0.0, 0.0)`. Each column covers at
    /// least one sample, so zoomed-in views repeat the containing sample's block.
    pub fn columns(&self, start: f64, end: f64, cols: usize) -> Vec<(f32, f32)> {
        let cols = cols.min(MAX_COLUMNS);
        let mut out = vec![(0.0, 0.0); cols];
        if cols == 0 || !start.is_finite() || !end.is_finite() || end <= start || self.len == 0 {
            return out;
        }
        let span = (end - start) / cols as f64;
        let len = self.len as f64;
        for (c, slot) in out.iter_mut().enumerate() {
            let s = start + span * c as f64;
            let e = start + span * (c + 1) as f64;
            if e <= 0.0 || s >= len {
                continue;
            }
            let si = s.max(0.0).floor();
            let ei = e.min(len).ceil().max(si + 1.0);
            *slot = self.minmax(si as usize, ei as usize);
        }
        out
    }
}

/// Incremental overview builder with an explicit level-zero block budget. Its resolution is
/// chosen before reading, so neither PCM nor overview storage grows with file duration.
pub struct PeaksBuilder {
    expected: usize,
    received: usize,
    block: usize,
    partial: usize,
    extrema: (f32, f32),
    blocks: Vec<(f32, f32)>,
}
impl PeaksBuilder {
    /// At most 1,048,576 base blocks (8 MiB); parent levels add less than one third.
    pub fn new(frames: usize, max_blocks: usize) -> crate::Result<Self> {
        if !(1..=MAX_COLUMNS).contains(&max_blocks) {
            return Err(crate::AudioError::TooLarge("invalid waveform block budget".into()));
        }
        let needed = frames.div_ceil(max_blocks).max(BLOCK);
        let block = needed.checked_next_power_of_two().ok_or_else(|| crate::AudioError::TooLarge("waveform frame count overflow".into()))?;
        let mut blocks = Vec::new();
        blocks.try_reserve_exact(frames.div_ceil(block)).map_err(|e| crate::AudioError::TooLarge(e.to_string()))?;
        Ok(Self { expected: frames, received: 0, block, partial: 0, extrema: EMPTY, blocks })
    }
    pub fn push(&mut self, samples: &[f32]) -> crate::Result<()> {
        if samples.len() > self.expected.saturating_sub(self.received) {
            return Err(crate::AudioError::Malformed("waveform input exceeds its declared length".into()));
        }
        self.received += samples.len();
        for &sample in samples {
            self.extrema = merge(self.extrema, (sample, sample));
            self.partial += 1;
            if self.partial == self.block {
                self.blocks.push(finish(self.extrema));
                self.partial = 0;
                self.extrema = EMPTY;
            }
        }
        Ok(())
    }
    pub fn finish(mut self) -> crate::Result<Peaks> {
        if self.received != self.expected {
            return Err(crate::AudioError::Malformed("waveform input ended before its declared length".into()));
        }
        if self.partial > 0 {
            self.blocks.push(finish(self.extrema));
        }
        let mut levels = Vec::new();
        if !self.blocks.is_empty() {
            levels.push(self.blocks);
        }
        while let Some(last) = levels.last() {
            if last.len() <= 1 {
                break;
            }
            levels.push(last.chunks(FANOUT).map(|c| c.iter().fold(EMPTY, |acc, &b| merge(acc, b))).collect());
        }
        Ok(Peaks { len: self.expected, block: self.block, levels })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brute(samples: &[f32], start: usize, end: usize) -> (f32, f32) {
        let s = &samples[start..end];
        (s.iter().copied().fold(f32::INFINITY, f32::min), s.iter().copied().fold(f32::NEG_INFINITY, f32::max))
    }

    fn signal(n: usize) -> Vec<f32> {
        (0..n).map(|i| ((i as f32 * 0.037).sin() * (i as f32 * 0.0011).cos()) + if i % 997 == 0 { 0.5 } else { 0.0 }).collect()
    }

    #[test]
    fn aligned_ranges_are_exact() {
        let x = signal(100_003);
        let p = Peaks::build(&x);
        for &(a, b) in &[(0, 32), (0, 100_003), (32, 64_000), (3200, 3232), (96, 99_968), (64, 100_003)] {
            assert_eq!(p.minmax(a, b), brute(&x, a, b), "range {a}..{b}");
        }
    }

    #[test]
    fn unaligned_ranges_contain_truth_and_match_block_cover() {
        let x = signal(50_000);
        let p = Peaks::build(&x);
        let mut seed = 12345u64;
        for _ in 0..2000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let a = (seed >> 33) as usize % 50_000;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let b = a + 1 + (seed >> 33) as usize % (50_000 - a);
            let got = p.minmax(a, b);
            let truth = brute(&x, a, b);
            assert!(got.0 <= truth.0 && got.1 >= truth.1, "{a}..{b}: {got:?} vs {truth:?}");
            let cover = brute(&x, a / BLOCK * BLOCK, (b.div_ceil(BLOCK) * BLOCK).min(x.len()));
            assert_eq!(got, cover, "{a}..{b}");
        }
    }

    #[test]
    fn degenerate_inputs() {
        let p = Peaks::build(&[]);
        assert_eq!(p.len_samples(), 0);
        assert_eq!(p.minmax(0, 10), (0.0, 0.0));
        assert_eq!(p.columns(0.0, 10.0, 4), vec![(0.0, 0.0); 4]);
        let p = Peaks::build(&[0.5, -0.25]);
        assert_eq!(p.minmax(0, 2), (-0.25, 0.5));
        assert_eq!(p.minmax(5, 3), (0.0, 0.0));
        assert_eq!(p.minmax(0, usize::MAX), (-0.25, 0.5));
        assert!(p.columns(f64::NAN, 1.0, 3).iter().all(|&c| c == (0.0, 0.0)));
        assert_eq!(p.columns(0.0, 1.0, 0).len(), 0);
        let p = Peaks::build(&[f32::NAN, 1.0]);
        assert_eq!(p.minmax(0, 2), (1.0, 1.0));
    }

    #[test]
    fn columns_cover_range() {
        let x = signal(10_000);
        let p = Peaks::build(&x);
        let cols = p.columns(0.0, 10_000.0, 100);
        assert_eq!(cols.len(), 100);
        for (c, &(lo, hi)) in cols.iter().enumerate() {
            let truth = brute(&x, c * 100, c * 100 + 100);
            assert!(lo <= truth.0 && hi >= truth.1);
        }
        let all = cols.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |a, &c| (a.0.min(c.0), a.1.max(c.1)));
        assert_eq!(all, brute(&x, 0, 10_000));
        // Past the end of the data.
        let cols = p.columns(9_000.0, 11_000.0, 2);
        assert_eq!(cols[1], (0.0, 0.0));
        // Zoomed in further than one sample per column.
        assert_eq!(p.columns(0.0, 4.0, 8).len(), 8);
    }
}

#[cfg(test)]
mod bounded_tests {
    use super::*;
    #[test]
    fn incremental_chunks_match_existing_overview_exactly() {
        let samples: Vec<_> = (0..4103).map(|f| (f as f32 * 0.017).sin()).collect();
        let mut builder = PeaksBuilder::new(samples.len(), 200).unwrap();
        for chunk in samples.chunks(13) {
            builder.push(chunk).unwrap();
        }
        assert_eq!(builder.finish().unwrap(), Peaks::build(&samples));
    }
    #[test]
    fn bounded_overview_contains_true_extrema_and_never_exceeds_budget() {
        let samples: Vec<_> = (0..100_003).map(|f| (f as f32 * 0.17).sin()).collect();
        let mut builder = PeaksBuilder::new(samples.len(), 64).unwrap();
        for chunk in samples.chunks(777) {
            builder.push(chunk).unwrap();
        }
        let peaks = builder.finish().unwrap();
        assert!(peaks.levels[0].len() <= 64);
        assert!(peaks.levels.iter().map(Vec::len).sum::<usize>() <= 90);
        assert_eq!(peaks.len_samples(), samples.len());
        for start in (0..samples.len()).step_by(1013) {
            let end = (start + 701).min(samples.len());
            let (lo, hi) = peaks.minmax(start, end);
            assert!(samples[start..end].iter().all(|&x| x >= lo && x <= hi));
        }
        assert_eq!(peaks.columns(0.0, samples.len() as f64, 100).len(), 100);
    }
    #[test]
    fn legacy_serialization_defaults_to_original_resolution() {
        let peaks = Peaks::build(&[0.3, -0.7, 0.9]);
        let mut serialized = serde_json::to_value(&peaks).unwrap();
        serialized.as_object_mut().unwrap().remove("block");
        let restored: Peaks = serde_json::from_value(serialized).unwrap();
        assert_eq!(restored, peaks);
    }
    #[test]
    fn incomplete_excessive_and_hostile_lengths_are_rejected() {
        assert!(PeaksBuilder::new(100, 0).is_err());
        assert!(PeaksBuilder::new(100, usize::MAX).is_err());
        assert!(PeaksBuilder::new(usize::MAX, 1).is_err());
        let mut builder = PeaksBuilder::new(5, 1).unwrap();
        assert!(builder.push(&[1.0; 6]).is_err());
        builder.push(&[1.0; 3]).unwrap();
        assert!(builder.finish().is_err());
        let empty = PeaksBuilder::new(0, 1).unwrap().finish().unwrap();
        assert_eq!(empty.len_samples(), 0);
        assert_eq!(empty.minmax(0, 100), (0.0, 0.0));
    }
}
