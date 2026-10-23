//! Blocking float WAV writer with bounded scratch memory. Never use in the audio callback.

use crate::{AudioBuffer, AudioError, Result, pcm::finite_float, wav_float_header};
use std::io::Write;

const SCRATCH_BYTES: usize = 262_144;

/// A known-length float32 WAV/RF64 transfer. The caller owns destination publication,
/// cleanup on error and filesystem synchronization. `finish` verifies completeness and flushes.
pub struct FloatWavWriter<W> {
    writer: W,
    channels: usize,
    rate: u32,
    expected: u64,
    written: u64,
    failed: bool,
    scratch: Vec<u8>,
}

impl<W: Write> FloatWavWriter<W> {
    pub fn new(mut writer: W, channels: usize, rate: u32, frames: u64) -> Result<Self> {
        if !(1..=64).contains(&channels) {
            return Err(AudioError::Unsupported("stream writer requires 1–64 channels".into()));
        }
        let header = wav_float_header(channels, rate, frames)?;
        let mut scratch = Vec::new();
        scratch.try_reserve_exact(SCRATCH_BYTES).map_err(|_| AudioError::TooLarge("WAV writer scratch allocation failed".into()))?;
        writer.write_all(&header).map_err(|e| AudioError::Io(e.to_string()))?;
        Ok(Self { writer, channels, rate, expected: frames, written: 0, failed: false, scratch })
    }

    /// Write a uniform planar block. Validation precedes all output; I/O errors latch failure
    /// because a partially written block cannot safely be retried.
    pub fn write_frames(&mut self, block: &AudioBuffer) -> Result<()> {
        if self.failed {
            return Err(AudioError::Io("WAV transfer has already failed".into()));
        }
        if block.sample_rate != self.rate || block.num_channels() != self.channels || block.channels.iter().any(|c| c.len() != block.frames()) {
            return Err(AudioError::Encode("WAV block format or channel lengths differ".into()));
        }
        let frames = u64::try_from(block.frames()).map_err(|_| AudioError::TooLarge("WAV block frame count overflow".into()))?;
        let end = self
            .written
            .checked_add(frames)
            .filter(|n| *n <= self.expected)
            .ok_or_else(|| AudioError::Encode("WAV block exceeds declared length".into()))?;
        let chunk_frames = SCRATCH_BYTES / (self.channels * 4);
        for start in (0..block.frames()).step_by(chunk_frames) {
            self.scratch.clear();
            let stop = start.saturating_add(chunk_frames).min(block.frames());
            for frame in start..stop {
                for channel in &block.channels {
                    let sample = channel.get(frame).ok_or_else(|| AudioError::Encode("incomplete WAV block".into()))?;
                    self.scratch.extend_from_slice(&finite_float(*sample).to_le_bytes());
                }
            }
            if let Err(error) = self.writer.write_all(&self.scratch) {
                self.failed = true;
                return Err(AudioError::Io(error.to_string()));
            }
        }
        self.written = end;
        Ok(())
    }

    pub fn frames_written(&self) -> u64 {
        self.written
    }

    pub fn finish(mut self) -> Result<W> {
        if self.failed || self.written != self.expected {
            return Err(AudioError::Encode("incomplete WAV transfer".into()));
        }
        self.writer.flush().map_err(|e| AudioError::Io(e.to_string()))?;
        Ok(self.writer)
    }
}
