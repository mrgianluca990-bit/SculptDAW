//! Sequential native file decoding on control/worker threads, without retaining the whole file
//! or whole decoded PCM. Decoder packet buffers remain codec-owned; host staging is bounded.

use crate::{
    AudioBuffer, AudioError, AudioInfo, Result,
    symph::{guarded, info_of, map_err},
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::SystemTime,
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{Decoder, DecoderOptions},
    errors::Error as SymError,
    formats::{FormatOptions, FormatReader},
    io::{MediaSourceStream, MediaSourceStreamOptions},
    meta::{Limit, MetadataOptions},
    probe::Hint,
};

const MAX_PACKET_BYTES: usize = 16 * 1024 * 1024;
const MAX_BLOCK_SAMPLES: usize = 1_048_576;

// Symphonia's AIFF duration hint may count SSND's eight-byte prefix as sample data.
// COMM carries the actual frame count; inspect bounded headers and seek past chunk bodies.
fn aiff_declared_frames(file: &mut File, length: u64) -> Result<u64> {
    let mut offset = 12u64;
    for _ in 0..4096 {
        if offset.checked_add(8).is_none_or(|end| end > length) {
            break;
        }
        file.seek(SeekFrom::Start(offset)).map_err(|e| AudioError::Io(e.to_string()))?;
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk).map_err(|e| AudioError::Io(e.to_string()))?;
        let size = u64::from(u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
        let end = offset
            .checked_add(8)
            .and_then(|n| n.checked_add(size))
            .filter(|end| *end <= length)
            .ok_or_else(|| AudioError::Malformed("truncated AIFF chunk".into()))?;
        if chunk.get(..4) == Some(b"COMM") {
            if size < 18 {
                return Err(AudioError::Malformed("short AIFF COMM chunk".into()));
            }
            let mut common = [0u8; 6];
            file.read_exact(&mut common).map_err(|e| AudioError::Io(e.to_string()))?;
            return Ok(u64::from(u32::from_be_bytes([common[2], common[3], common[4], common[5]])));
        }
        offset = end.checked_add(size & 1).ok_or_else(|| AudioError::TooLarge("AIFF chunk offset overflow".into()))?;
    }
    Err(AudioError::Malformed("AIFF COMM chunk missing or header limit exceeded".into()))
}

pub struct FileDecoder {
    backend: Backend,
}

enum Backend {
    Wave {
        reader: Box<crate::stream::WavReader<File>>,
        watch: File,
        length: u64,
        modified: Option<SystemTime>,
        frames: u64,
        finished: bool,
        failed: bool,
    },
    Codec(Box<CodecDecoder>),
}

impl FileDecoder {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        guarded(|| {
            let path = path.as_ref();
            let mut file = File::open(path).map_err(|e| AudioError::Io(e.to_string()))?;
            let mut header = [0u8; 12];
            let wave = file.read_exact(&mut header).is_ok()
                && matches!(header.get(..4), Some(b"RIFF" | b"RF64" | b"BW64"))
                && header.get(8..12) == Some(b"WAVE");
            if wave {
                file.seek(SeekFrom::Start(0)).map_err(|e| AudioError::Io(e.to_string()))?;
                let watch = file.try_clone().map_err(|e| AudioError::Io(e.to_string()))?;
                let metadata = watch.metadata().map_err(|e| AudioError::Io(e.to_string()))?;
                match crate::stream::WavReader::from_reader(file) {
                    Ok(reader) => {
                        return Ok(Self {
                            backend: Backend::Wave {
                                reader: Box::new(reader),
                                watch,
                                length: metadata.len(),
                                modified: metadata.modified().ok(),
                                frames: 0,
                                finished: false,
                                failed: false,
                            },
                        });
                    }
                    Err(AudioError::Unsupported(_)) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(Self { backend: Backend::Codec(Box::new(CodecDecoder::open(path)?)) })
        })
    }
    /// Header-derived duration may be estimated for compressed streams until clean EOF.
    pub fn info(&self) -> &AudioInfo {
        match &self.backend {
            Backend::Wave { reader, .. } => reader.info(),
            Backend::Codec(decoder) => decoder.info(),
        }
    }
    pub fn frames_decoded(&self) -> u64 {
        match &self.backend {
            Backend::Wave { frames, .. } => *frames,
            Backend::Codec(decoder) => decoder.frames_decoded(),
        }
    }
    pub fn next_block(&mut self) -> Result<Option<AudioBuffer>> {
        match &mut self.backend {
            Backend::Codec(decoder) => decoder.next_block(),
            Backend::Wave { reader, watch, length, modified, frames, finished, failed } => {
                if *failed {
                    return Err(AudioError::Io("file decode has already failed".into()));
                }
                if *finished {
                    return Ok(None);
                }
                let result = guarded(|| {
                    if *frames == reader.info().frames {
                        let metadata = watch.metadata().map_err(|e| AudioError::Io(e.to_string()))?;
                        if metadata.len() != *length || metadata.modified().ok() != *modified {
                            return Err(AudioError::Io("file changed during decode".into()));
                        }
                        *finished = true;
                        return Ok(None);
                    }
                    let count = (reader.info().frames - *frames).min(65_536 / u64::from(reader.info().channels)) as usize;
                    let block = reader.read_frames(*frames, count)?;
                    if block.frames() != count {
                        return Err(AudioError::Malformed("incomplete WAV decode".into()));
                    }
                    *frames += count as u64;
                    Ok(Some(block))
                });
                if result.is_err() {
                    *failed = true;
                }
                result
            }
        }
    }
}

/// Canonical float WAV transfer at the original sample rate, without whole-file staging.
/// No SRC is performed. The caller owns output cleanup/publication; partial output must be
/// discarded on any error or cancellation. The final header is written only after clean EOF.
pub fn decode_file_to_wave<W: Write + Seek>(
    path: impl AsRef<Path>,
    destination: W,
    mut progress: impl FnMut(u64, Option<u64>) -> bool,
) -> Result<(W, AudioInfo)> {
    let mut decoder = FileDecoder::open(path)?;
    let hint = (decoder.info().frames > 0).then_some(decoder.info().frames);
    if !progress(0, hint) {
        return Err(AudioError::Io("audio transfer cancelled".into()));
    }
    let first = decoder.next_block()?.ok_or_else(|| AudioError::Malformed("input has no audio frames".into()))?;
    let mut writer = crate::stream_write::GrowingFloatWavWriter::new(destination, first.num_channels(), first.sample_rate)?;
    writer.write_frames(&first)?;
    if !progress(decoder.frames_decoded(), hint) {
        return Err(AudioError::Io("audio transfer cancelled".into()));
    }
    while let Some(block) = decoder.next_block()? {
        writer.write_frames(&block)?;
        if !progress(decoder.frames_decoded(), hint) {
            return Err(AudioError::Io("audio transfer cancelled".into()));
        }
    }
    let info = decoder.info().clone();
    if !progress(info.frames, Some(info.frames)) {
        return Err(AudioError::Io("audio transfer cancelled".into()));
    }
    Ok((writer.finish()?, info))
}

struct CodecDecoder {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    info: AudioInfo,
    expected: Option<u64>,
    frames: u64,
    samples: Option<SampleBuffer<f32>>,
    finished: bool,
    failed: bool,
    watch: File,
    length: u64,
    modified: Option<SystemTime>,
}

impl CodecDecoder {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        guarded(|| {
            let path = path.as_ref();
            let mut file = File::open(path).map_err(|e| AudioError::Io(e.to_string()))?;
            let metadata = file.metadata().map_err(|e| AudioError::Io(e.to_string()))?;
            let mut header = [0u8; 32];
            let count = file.read(&mut header).map_err(|e| AudioError::Io(e.to_string()))?;
            let aiff_frames = if header.get(..4) == Some(b"FORM") && matches!(header.get(8..12), Some(b"AIFF" | b"AIFC")) {
                Some(aiff_declared_frames(&mut file, metadata.len())?)
            } else {
                None
            };
            file.seek(SeekFrom::Start(0)).map_err(|e| AudioError::Io(e.to_string()))?;
            let watch = file.try_clone().map_err(|e| AudioError::Io(e.to_string()))?;
            let extension = path.extension().and_then(|x| x.to_str());
            let detected = crate::detect_format(header.get(..count).unwrap_or_default(), extension);
            let mut hint = Hint::new();
            if let Some(extension) = extension {
                hint.with_extension(extension);
            }
            let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
            let metadata_options = MetadataOptions { limit_metadata_bytes: Limit::Maximum(1 << 20), limit_visual_bytes: Limit::Maximum(1 << 20) };
            let reader = symphonia::default::get_probe()
                .format(&hint, stream, &FormatOptions { enable_gapless: true, ..FormatOptions::default() }, &metadata_options)
                .map_err(map_err)?
                .format;
            let (track, mut info) = info_of(reader.as_ref(), detected)?;
            if let Some(frames) = aiff_frames {
                info.frames = frames;
            }
            if info.channels > 64 {
                return Err(AudioError::Unsupported("file decoder supports at most 64 channels".into()));
            }
            let parameters = reader
                .tracks()
                .iter()
                .find(|t| t.id == track)
                .map(|t| t.codec_params.clone())
                .ok_or_else(|| AudioError::Malformed("decoder track disappeared".into()))?;
            let expected = aiff_frames.or_else(|| parameters.n_frames.filter(|n| *n > 0));
            let decoder = symphonia::default::get_codecs().make(&parameters, &DecoderOptions::default()).map_err(map_err)?;
            Ok(Self {
                reader,
                decoder,
                track,
                info,
                expected,
                frames: 0,
                samples: None,
                finished: false,
                failed: false,
                watch,
                length: metadata.len(),
                modified: metadata.modified().ok(),
            })
        })
    }

    /// Header-derived duration may be estimated until EOF. At EOF it is the decoded count.
    pub fn info(&self) -> &AudioInfo {
        &self.info
    }
    pub fn frames_decoded(&self) -> u64 {
        self.frames
    }

    /// Corrupt packets, stream changes and incomplete declared media are errors, never skipped
    /// samples. After failure the transfer cannot be resumed or mistaken for a clean EOF.
    pub fn next_block(&mut self) -> Result<Option<AudioBuffer>> {
        if self.failed {
            return Err(AudioError::Io("file decode has already failed".into()));
        }
        if self.finished {
            return Ok(None);
        }
        let result = guarded(|| self.decode_block());
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn decode_block(&mut self) -> Result<Option<AudioBuffer>> {
        for _ in 0..4096 {
            let packet = match self.reader.next_packet() {
                Ok(packet) => packet,
                Err(SymError::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    let metadata = self.watch.metadata().map_err(|e| AudioError::Io(e.to_string()))?;
                    if metadata.len() != self.length || metadata.modified().ok() != self.modified {
                        return Err(AudioError::Io("file changed during decode".into()));
                    }
                    if self.expected.is_some_and(|n| n != self.frames) {
                        return Err(AudioError::Malformed(format!(
                            "{:?}: decoded {} frames, declared {:?}",
                            self.info.format, self.frames, self.expected
                        )));
                    }
                    self.info.frames = self.frames;
                    self.finished = true;
                    return Ok(None);
                }
                Err(error) => return Err(map_err(error)),
            };
            if packet.track_id() != self.track {
                continue;
            }
            if packet.data.len() > MAX_PACKET_BYTES {
                return Err(AudioError::TooLarge("decoder packet exceeds 16 MiB".into()));
            }
            let decoded = self.decoder.decode(&packet).map_err(map_err)?;
            let spec = *decoded.spec();
            let channels = spec.channels.count();
            if !(1..=64).contains(&channels)
                || spec.rate != self.info.sample_rate
                || (self.info.channels != 0 && usize::from(self.info.channels) != channels)
            {
                return Err(AudioError::Unsupported("audio format changed during decode".into()));
            }
            if decoded.capacity().checked_mul(channels).is_none_or(|n| n > MAX_BLOCK_SAMPLES) {
                return Err(AudioError::TooLarge("decoded block exceeds staging budget".into()));
            }
            self.info.channels = channels as u16;
            if decoded.frames() == 0 {
                continue;
            }
            self.frames =
                self.frames.checked_add(decoded.frames() as u64).ok_or_else(|| AudioError::TooLarge("decoded frame count overflow".into()))?;
            let staging = match &mut self.samples {
                Some(staging) if staging.capacity() >= decoded.capacity() * channels => staging,
                _ => self.samples.insert(SampleBuffer::new(decoded.capacity() as u64, spec)),
            };
            staging.copy_interleaved_ref(decoded);
            let mut block = AudioBuffer::from_interleaved(spec.rate, channels, staging.samples());
            for sample in block.channels.iter_mut().flatten() {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
            }
            return Ok(Some(block));
        }
        Err(AudioError::Malformed("too many packets without decoded audio".into()))
    }
}
