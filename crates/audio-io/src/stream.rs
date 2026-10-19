//! Bounded, seekable native WAV/RF64 reading. This is blocking worker/offline I/O, never a
//! realtime callback API. Parsing skips unrelated chunk bodies and decoding reads only the
//! requested frames; memory does not depend on the duration of the file.

use crate::pcm::{PcmKind, deinterleave, le_u32, le_u64};
use crate::{AudioBuffer, AudioError, AudioInfo, Result, wav};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Maximum frames accepted by a single read. Larger transfers must be split by the worker.
pub const MAX_READ_FRAMES: usize = 65_536;
/// A format may contain more channels; the DAW's native streaming path supports up to 64.
pub const MAX_STREAM_CHANNELS: usize = 64;
const MAX_HEADER_CHUNKS: usize = 4096;
const MAX_HEADER_BYTES: usize = 1 << 20;

/// Seekable PCM/float WAV reader. The open file is owned by the worker, not the render thread.
#[derive(Debug)]
pub struct WavReader<R = File> {
    reader: R,
    info: AudioInfo,
    kind: PcmKind,
    data_offset: u64,
    frame_bytes: usize,
    scratch: Vec<u8>,
}

fn io(error: std::io::Error) -> AudioError {
    AudioError::Io(error.to_string())
}

fn malformed(message: &str) -> AudioError {
    AudioError::Malformed(message.into())
}

impl WavReader<File> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_reader(File::open(path).map_err(io)?)
    }

    /// Explicit recovery for this application's unfinished captures, whose PCM runs to EOF.
    pub fn open_unfinalized(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_unfinalized_reader(File::open(path).map_err(io)?)
    }
}

impl<R: Read + Seek> WavReader<R> {
    pub fn from_reader(reader: R) -> Result<Self> {
        Self::read_header(reader, false)
    }

    /// A zero-sized data chunk is normally empty, even if metadata follows it. Only a caller
    /// recovering a known unfinished capture may reinterpret the remaining bytes as PCM.
    pub fn from_unfinalized_reader(reader: R) -> Result<Self> {
        Self::read_header(reader, true)
    }

    fn read_header(mut reader: R, recover_unfinalized: bool) -> Result<Self> {
        let length = reader.seek(SeekFrom::End(0)).map_err(io)?;
        reader.seek(SeekFrom::Start(0)).map_err(io)?;
        let mut root = [0u8; 12];
        reader.read_exact(&mut root).map_err(io)?;
        if !wav::is_wav(&root) {
            return Err(malformed("not a RIFF/RF64 WAVE file"));
        }
        let rf64 = root.get(..4) != Some(b"RIFF");
        let mut header = root.to_vec();
        let mut position = 12u64;
        let mut ds64_data = None;
        let mut seen_fmt = false;
        for _ in 0..MAX_HEADER_CHUNKS {
            if position.checked_add(8).is_none_or(|end| end > length) {
                return Err(malformed("WAV has no complete data chunk"));
            }
            reader.seek(SeekFrom::Start(position)).map_err(io)?;
            let mut chunk = [0u8; 8];
            reader.read_exact(&mut chunk).map_err(io)?;
            let size32 = le_u32(&chunk, 4).ok_or_else(|| malformed("short WAV chunk header"))?;
            let body = position.checked_add(8).ok_or_else(|| malformed("WAV offset overflow"))?;
            if chunk.get(..4) == Some(b"data") {
                if !seen_fmt {
                    return Err(malformed("streaming WAV needs fmt before data"));
                }
                let data_bytes = if rf64 && size32 == u32::MAX {
                    ds64_data.ok_or_else(|| malformed("RF64 data size is missing from ds64"))?
                } else if recover_unfinalized && !rf64 && size32 == 0 {
                    // A capture interrupted before its first header update can still be recovered.
                    length.saturating_sub(body)
                } else {
                    u64::from(size32)
                };
                if data_bytes > length.saturating_sub(body) {
                    return Err(malformed("WAV audio is shorter than its declared data size"));
                }
                header.extend_from_slice(&chunk);
                let layout = match wav::parse(&header)? {
                    wav::WavParse::Native(layout) => layout,
                    wav::WavParse::Foreign { .. } => return Err(AudioError::Unsupported("streaming WAV requires PCM or IEEE float".into())),
                };
                let channels = usize::from(layout.info.channels);
                if channels > MAX_STREAM_CHANNELS {
                    return Err(AudioError::Unsupported(format!("streaming WAV supports at most {MAX_STREAM_CHANNELS} channels")));
                }
                let frame_bytes = channels.checked_mul(layout.kind.bytes()).filter(|&n| n > 0).ok_or_else(|| malformed("invalid WAV frame size"))?;
                if !data_bytes.is_multiple_of(frame_bytes as u64) {
                    return Err(malformed("WAV data ends in a partial sample frame"));
                }
                let mut info = layout.info;
                info.frames = data_bytes / frame_bytes as u64;
                return Ok(Self { reader, info, kind: layout.kind, data_offset: body, frame_bytes, scratch: Vec::new() });
            }
            let end = body.checked_add(u64::from(size32)).ok_or_else(|| malformed("WAV chunk length overflow"))?;
            if end > length {
                return Err(malformed("WAV metadata chunk extends beyond the file"));
            }
            // Retain only metadata the shared WAV parser understands. In particular, never
            // allocate/read a huge JUNK chunk just to discover the following data chunk.
            let keep = match chunk.get(..4) {
                Some(b"fmt ") => {
                    if seen_fmt {
                        return Err(malformed("ambiguous duplicate WAV fmt chunk"));
                    }
                    seen_fmt = true;
                    usize::try_from(size32).unwrap_or(usize::MAX).min(256)
                }
                Some(b"ds64") => {
                    if ds64_data.is_some() || size32 < 28 {
                        return Err(malformed("invalid or duplicate RF64 ds64 chunk"));
                    }
                    28
                }
                Some(b"bext") => usize::try_from(size32).unwrap_or(usize::MAX).min(602),
                _ => 0,
            };
            if keep > 0 {
                let padded = keep + (keep & 1);
                if header.len().checked_add(8 + padded).is_none_or(|n| n > MAX_HEADER_BYTES) {
                    return Err(AudioError::TooLarge("WAV streaming metadata exceeds the header budget".into()));
                }
                header.extend_from_slice(chunk.get(..4).unwrap_or_default());
                header.extend_from_slice(&(keep as u32).to_le_bytes());
                let start = header.len();
                header.resize(start + padded, 0);
                reader.read_exact(header.get_mut(start..start + keep).ok_or_else(|| malformed("invalid metadata buffer"))?).map_err(io)?;
                if chunk.get(..4) == Some(b"ds64") {
                    ds64_data = le_u64(header.get(start..start + keep).unwrap_or_default(), 8);
                }
            }
            position = end.checked_add(u64::from(size32 & 1)).ok_or_else(|| malformed("WAV padding overflow"))?;
        }
        Err(AudioError::TooLarge(format!("WAV streaming header exceeds {MAX_HEADER_CHUNKS} chunks")))
    }

    pub fn info(&self) -> &AudioInfo {
        &self.info
    }

    /// Decode a bounded range at its original rate, clamped to EOF. `read_exact` handles short
    /// underlying reads; truncation/errors after opening are reported rather than filled with zeros.
    pub fn read_frames(&mut self, start: u64, count: usize) -> Result<AudioBuffer> {
        if count > MAX_READ_FRAMES {
            return Err(AudioError::TooLarge(format!("split streaming reads into at most {MAX_READ_FRAMES} frames")));
        }
        let frames = self.info.frames.saturating_sub(start).min(count as u64) as usize;
        let size = frames.checked_mul(self.frame_bytes).ok_or_else(|| AudioError::TooLarge("stream read size overflow".into()))?;
        if size > 0 {
            let offset = start
                .checked_mul(self.frame_bytes as u64)
                .and_then(|n| self.data_offset.checked_add(n))
                .ok_or_else(|| malformed("stream seek offset overflow"))?;
            self.reader.seek(SeekFrom::Start(offset)).map_err(io)?;
            if size > self.scratch.capacity() {
                self.scratch
                    .try_reserve_exact(size - self.scratch.len())
                    .map_err(|e| AudioError::TooLarge(format!("cannot allocate stream read buffer: {e}")))?;
            }
            self.scratch.resize(size, 0);
            self.reader.read_exact(&mut self.scratch).map_err(io)?;
        } else {
            self.scratch.clear();
        }
        let channels = deinterleave(&self.scratch, self.kind, usize::from(self.info.channels), frames as u64)?;
        Ok(AudioBuffer { sample_rate: self.info.sample_rate, channels })
    }

    /// Current reusable byte storage, independent of the length of the source file.
    pub fn buffered_bytes(&self) -> usize {
        self.scratch.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BitDepth, BwfInfo, EncodeOptions};
    use std::io::{Cursor, Write};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn fixture(channels: usize, bits: BitDepth) -> Vec<u8> {
        let buffer = AudioBuffer {
            sample_rate: 88_200,
            channels: (0..channels).map(|c| (0..4103).map(|f| ((f * 13 + c * 71) as f32 * 0.015).sin() * 0.75).collect()).collect(),
        };
        crate::encode(
            &buffer,
            &EncodeOptions {
                bit_depth: bits,
                dither: false,
                bwf: Some(BwfInfo { description: "Streaming fixture".into(), time_reference: 123_456, ..BwfInfo::default() }),
                ..EncodeOptions::default()
            },
        )
        .unwrap()
    }

    fn primitive_wave(tag: u16, bits: u16, channels: u16, data: &[u8]) -> Vec<u8> {
        let align = channels * (bits / 8);
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36u32 + data.len() as u32 + data.len() as u32 % 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&96_000u32.to_le_bytes());
        bytes.extend_from_slice(&(96_000u32 * u32::from(align)).to_le_bytes());
        bytes.extend_from_slice(&align.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        if !data.len().is_multiple_of(2) {
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn eight_bit_pcm_and_float64_use_the_same_decode_and_nonfinite_policy() {
        let pcm = primitive_wave(1, 8, 1, &[0, 64, 128, 192, 255]);
        let data: Vec<u8> = [f64::NAN, f64::INFINITY, -0.5, 0.25, 1.0, -1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let float = primitive_wave(3, 64, 2, &data);
        for bytes in [pcm, float] {
            let (_, decoded) = crate::decode(&bytes, None).unwrap();
            let mut reader = WavReader::from_reader(Cursor::new(bytes)).unwrap();
            assert_eq!(reader.read_frames(0, 16).unwrap(), decoded);
            let tail = reader.read_frames(1, 16).unwrap();
            assert_eq!(tail.channels, decoded.channels.iter().map(|c| c[1..].to_vec()).collect::<Vec<_>>());
            assert!(tail.channels.iter().flatten().all(|s| s.is_finite()));
        }
    }

    #[test]
    fn zero_data_size_recovers_frames_but_rf64_requires_a_real_64_bit_size() {
        let mut bytes = fixture(2, BitDepth::Float32);
        let wav::WavParse::Native(layout) = wav::parse(&bytes).unwrap() else { panic!("expected a native WAV fixture") };
        bytes[layout.data_start - 4..layout.data_start].copy_from_slice(&0u32.to_le_bytes());
        let mut empty = WavReader::from_reader(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(empty.info().frames, 0, "metadata/tail after an empty chunk is not invented audio");
        assert_eq!(empty.read_frames(0, 16).unwrap().frames(), 0);
        let mut reader = WavReader::from_unfinalized_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.info().frames, 4103);
        assert_eq!(reader.read_frames(0, 4103).unwrap().frames(), 4103);
        let mut missing = primitive_wave(1, 16, 1, &[0, 0]);
        missing[..4].copy_from_slice(b"RF64");
        missing[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(WavReader::from_reader(Cursor::new(missing)), Err(AudioError::Malformed(_))));
    }

    #[test]
    fn random_windows_match_full_decode_for_every_native_depth_and_multichannel() {
        for bits in [BitDepth::Int16, BitDepth::Int24, BitDepth::Int32, BitDepth::Float32] {
            for channels in [1, 2, 7] {
                let bytes = fixture(channels, bits);
                let (info, decoded) = crate::decode(&bytes, None).unwrap();
                let mut reader = WavReader::from_reader(Cursor::new(bytes)).unwrap();
                assert_eq!(reader.info(), &info);
                for start in [0, 1, 31, 1023, 4095, 4102, 4103, u64::MAX] {
                    let out = reader.read_frames(start, 127).unwrap();
                    let first = (start.min(decoded.frames() as u64)) as usize;
                    let last = (first + 127).min(decoded.frames());
                    assert_eq!(out.sample_rate, 88_200);
                    for (a, b) in out.channels.iter().zip(&decoded.channels) {
                        assert_eq!(a.as_slice(), &b[first..last], "{bits:?}, {channels} channels, start {start}");
                    }
                }
                assert!(reader.buffered_bytes() <= 127 * channels * 8);
                assert!(reader.read_frames(0, MAX_READ_FRAMES + 1).is_err());
            }
        }
    }

    struct ShortReader {
        inner: Cursor<Vec<u8>>,
        read_bytes: Arc<AtomicUsize>,
    }
    impl Read for ShortReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let limit = out.len().min(3);
            let count = self.inner.read(&mut out[..limit])?;
            self.read_bytes.fetch_add(count, Ordering::Relaxed);
            Ok(count)
        }
    }
    impl Seek for ShortReader {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(position)
        }
    }

    #[test]
    fn short_reads_are_completed_and_large_unknown_metadata_is_skipped() {
        let bytes = fixture(2, BitDepth::Float32);
        let (_, reference) = crate::decode(&bytes, None).unwrap();
        let mut large = bytes[..12].to_vec();
        large.extend_from_slice(b"JUNK");
        large.extend_from_slice(&(2u32 << 20).to_le_bytes());
        large.resize(large.len() + (2 << 20), 0);
        large.extend_from_slice(&bytes[12..]);
        let riff_size = (large.len() - 8) as u32;
        large[4..8].copy_from_slice(&riff_size.to_le_bytes());
        let reads = Arc::new(AtomicUsize::new(0));
        let mut reader = WavReader::from_reader(ShortReader { inner: Cursor::new(large), read_bytes: Arc::clone(&reads) }).unwrap();
        assert!(reads.load(Ordering::Relaxed) < 1024, "open reads metadata, never the 2 MiB JUNK body");
        let out = reader.read_frames(10, 777).unwrap();
        assert_eq!(out.channels, reference.channels.iter().map(|c| c[10..787].to_vec()).collect::<Vec<_>>());
    }

    #[test]
    fn sparse_rf64_larger_than_four_gib_is_seeked_without_loading_it() {
        let path = std::env::temp_dir().join(format!("sculpt-stream-rf64-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let frames = (1u64 << 32) / 8 + 19;
        let header = crate::wav_float_header(2, 48_000, frames).unwrap();
        assert_eq!(&header[..4], b"RF64");
        let mut file = File::create_new(&path).unwrap();
        file.write_all(&header).unwrap();
        file.set_len(header.len() as u64 + frames * 8).unwrap();
        file.seek(SeekFrom::End(-8)).unwrap();
        file.write_all(&[0.25f32.to_le_bytes(), (-0.75f32).to_le_bytes()].concat()).unwrap();
        let mut reader = WavReader::open(&path).unwrap();
        assert_eq!(reader.info().frames, frames);
        assert_eq!(reader.read_frames(frames - 1, 128).unwrap().channels, vec![vec![0.25], vec![-0.75]]);
        assert!(reader.buffered_bytes() <= 8);
        drop(reader);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn damaged_headers_and_partial_frames_are_rejected_before_decode() {
        let bytes = fixture(2, BitDepth::Int24);
        for cut in [0, 4, 12, bytes.len() - 1, bytes.len() - 7] {
            assert!(WavReader::from_reader(Cursor::new(bytes[..cut].to_vec())).is_err(), "truncation at {cut}");
        }
        let mut huge_junk = b"RIFF\0\0\0\0WAVEJUNK".to_vec();
        huge_junk.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(WavReader::from_reader(Cursor::new(huge_junk)).is_err());
        let mut repeated = b"RIFF\0\0\0\0WAVE".to_vec();
        for _ in 0..=MAX_HEADER_CHUNKS {
            repeated.extend_from_slice(b"JUNK\0\0\0\0");
        }
        assert!(matches!(WavReader::from_reader(Cursor::new(repeated)), Err(AudioError::TooLarge(_))));
    }

    #[test]
    fn truncation_after_open_is_an_io_error_not_silent_audio() {
        let path = std::env::temp_dir().join(format!("sculpt-stream-truncate-{}.wav", std::process::id()));
        std::fs::write(&path, fixture(2, BitDepth::Float32)).unwrap();
        let mut reader = WavReader::open(&path).unwrap();
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(32).unwrap();
        assert!(matches!(reader.read_frames(0, 32), Err(AudioError::Io(_))));
        drop(reader);
        std::fs::remove_file(path).unwrap();
    }
}
