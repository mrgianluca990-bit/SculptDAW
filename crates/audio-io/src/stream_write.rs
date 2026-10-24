//! Blocking float WAV writer with bounded scratch memory. Never use in the audio callback.

use crate::{AudioBuffer, AudioError, Result, pcm::finite_float, wav_float_header};
use std::io::{Seek, SeekFrom, Write};

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

/// Float WAV/RF64 transfer whose length is learned while decoding. Requires an empty seekable
/// destination. Only `finish` writes the final duration; the caller must discard partial output
/// after any decoder/cancellation error and synchronize/publish the file after success.
pub struct GrowingFloatWavWriter<W> {
    inner: FloatWavWriter<W>,
    header_bytes: u64,
}

impl<W: Write + Seek> GrowingFloatWavWriter<W> {
    pub fn new(mut destination: W, channels: usize, rate: u32) -> Result<Self> {
        if destination.seek(SeekFrom::End(0)).map_err(|e| AudioError::Io(e.to_string()))? != 0 {
            return Err(AudioError::Encode("growing WAV destination must be empty".into()));
        }
        let mut inner = FloatWavWriter::new(destination, channels, rate, 0)?;
        let header_bytes = inner.writer.stream_position().map_err(|e| AudioError::Io(e.to_string()))?;
        inner.expected = u64::MAX;
        Ok(Self { inner, header_bytes })
    }

    /// All transfer errors latch, including invalid blocks. A caller cannot ignore an error
    /// and finalize a shorter, apparently successful import.
    pub fn write_frames(&mut self, block: &AudioBuffer) -> Result<()> {
        let result = (|| {
            if self.inner.failed {
                return Err(AudioError::Io("growing WAV transfer has already failed".into()));
            }
            let end = self.inner.written.checked_add(block.frames() as u64).ok_or_else(|| AudioError::TooLarge("WAV frame count overflow".into()))?;
            wav_float_header(self.inner.channels, self.inner.rate, end)?;
            self.inner.write_frames(block)
        })();
        if result.is_err() {
            self.inner.failed = true;
        }
        result
    }

    pub fn frames_written(&self) -> u64 {
        self.inner.written
    }

    pub fn finish(mut self) -> Result<W> {
        if self.inner.failed {
            return Err(AudioError::Encode("growing WAV transfer failed".into()));
        }
        let header = wav_float_header(self.inner.channels, self.inner.rate, self.inner.written)?;
        if header.len() as u64 != self.header_bytes {
            return Err(AudioError::Encode("WAV final header changed size".into()));
        }
        let expected_end = self
            .inner
            .written
            .checked_mul(self.inner.channels as u64)
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_add(self.header_bytes))
            .ok_or_else(|| AudioError::TooLarge("WAV output size overflow".into()))?;
        self.inner.writer.flush().map_err(|e| AudioError::Io(e.to_string()))?;
        let end = self.inner.writer.stream_position().map_err(|e| AudioError::Io(e.to_string()))?;
        let length = self.inner.writer.seek(SeekFrom::End(0)).map_err(|e| AudioError::Io(e.to_string()))?;
        if end != expected_end || length != expected_end {
            return Err(AudioError::Encode("incomplete growing WAV payload".into()));
        }
        self.inner.writer.seek(SeekFrom::Start(0)).map_err(|e| AudioError::Io(e.to_string()))?;
        self.inner.writer.write_all(&header).map_err(|e| AudioError::Io(e.to_string()))?;
        self.inner.writer.seek(SeekFrom::Start(end)).map_err(|e| AudioError::Io(e.to_string()))?;
        self.inner.writer.flush().map_err(|e| AudioError::Io(e.to_string()))?;
        Ok(self.inner.writer)
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod growing_large_file_tests {
    use super::*;
    use std::{
        fs::OpenOptions,
        io::Read,
        sync::atomic::{AtomicU64, Ordering},
    };
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    struct FileGuard(std::path::PathBuf);
    impl Drop for FileGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn simulated_large_payload_finalizes_rf64_and_reopens_sparse_file() {
        let path = FileGuard(std::env::temp_dir().join(format!(
            "sculpt-growing-rf64-{}-{}.wav",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        )));
        let file = OpenOptions::new().write(true).read(true).create_new(true).open(&path.0).unwrap();
        let mut writer = GrowingFloatWavWriter::new(file, 1, 48_000).unwrap();
        // Exercise finalization beyond 4 GiB without spending the test run encoding gigabytes.
        // The sparse payload is valid zero PCM; normal block transfers are tested separately.
        let frames = 1_100_000_000u64;
        writer.inner.written = frames;
        let end = writer.header_bytes + frames * 4;
        writer.inner.writer.set_len(end).unwrap();
        writer.inner.writer.seek(SeekFrom::Start(end)).unwrap();
        let mut file = writer.finish().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut signature = [0u8; 4];
        file.read_exact(&mut signature).unwrap();
        assert_eq!(&signature, b"RF64");
        let mut reader = crate::stream::WavReader::from_reader(file).unwrap();
        assert_eq!(reader.info().frames, frames);
        assert_eq!(reader.read_frames(frames - 1, 1).unwrap().channels[0], vec![0.0]);
    }
}
