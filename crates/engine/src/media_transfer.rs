//! Blocking media preparation for native import workers. No engine/document edits, no callback
//! I/O. Output is reserved exclusively and removed on failure; filesystem publication is not
//! claimed to be crash-atomic. Originals are only opened for reading.
use crate::{EngineError, Result};
use soundcraft_audio_io::{
    AudioBuffer, AudioInfo,
    decode_stream::{FileDecoder, decode_file_to_wave},
    stream::WavReader,
    stream_write::FloatWavWriter,
};
use soundcraft_dsp::offline::WindowedResampler;
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStage {
    Decode,
    Resample,
}

#[derive(Debug, Clone, Copy)]
pub struct TransferProgress {
    pub stage: TransferStage,
    pub completed: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TransferReceipt {
    pub source: AudioInfo,
    pub output_rate: u32,
    pub output_frames: u64,
    pub output_bytes: u64,
}

struct OwnedOutput {
    path: PathBuf,
    keep: bool,
}
impl Drop for OwnedOutput {
    fn drop(&mut self) {
        if !self.keep
            && let Err(error) = std::fs::remove_file(&self.path)
        {
            log::warn!("media transfer cleanup {}: {error}", self.path.display());
        }
    }
}
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn intermediate(parent: &Path) -> Result<(OwnedOutput, File)> {
    for _ in 0..1024 {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".sculpt-decode-{}-{sequence}.partial", std::process::id()));
        match OpenOptions::new().read(true).write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((OwnedOutput { path, keep: false }, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(EngineError::Io(format!("{}: {error}", path.display()))),
        }
    }
    Err(EngineError::Io("no free media transfer temporary name".into()))
}

fn cancelled() -> EngineError {
    EngineError::Io("media transfer cancelled".into())
}

/// Convert to canonical float WAV at `rate`. The parent directory must exist and output must
/// not exist. A rate mismatch uses a temporary original-rate WAV, then bounded sinc windows.
/// Two passes trade disk I/O for bounded PCM memory. Call outside the UI/audio callback.
pub fn transfer_file(input: &Path, output: &Path, rate: u32, mut progress: impl FnMut(TransferProgress) -> bool) -> Result<TransferReceipt> {
    if rate == 0 {
        return Err(EngineError::Io("media transfer sample rate is zero".into()));
    }
    let file = OpenOptions::new().write(true).create_new(true).open(output).map_err(|e| EngineError::Io(format!("{}: {e}", output.display())))?;
    let mut owned = OwnedOutput { path: output.to_path_buf(), keep: false };
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let result = transfer_inner(input, parent, file, rate, &mut progress);
    if result.is_ok() {
        owned.keep = true;
    }
    result
}

fn transfer_inner(
    input: &Path,
    parent: &Path,
    mut output: File,
    rate: u32,
    progress: &mut impl FnMut(TransferProgress) -> bool,
) -> Result<TransferReceipt> {
    let error = |e| EngineError::Io(format!("{}: {e}", input.display()));
    let probe = FileDecoder::open(input).map_err(error)?;
    let source_rate = probe.info().sample_rate;
    drop(probe);
    if source_rate == rate {
        let (_, source) =
            decode_file_to_wave(input, &mut output, |completed, total| progress(TransferProgress { stage: TransferStage::Decode, completed, total }))
                .map_err(error)?;
        if source.sample_rate != rate {
            return Err(EngineError::Io("input rate changed before media transfer".into()));
        }
        output.sync_all().map_err(|e| EngineError::Io(e.to_string()))?;
        let output_bytes = output.metadata().map_err(|e| EngineError::Io(e.to_string()))?.len();
        return Ok(TransferReceipt { output_rate: rate, output_frames: source.frames, output_bytes, source });
    }
    let (_temporary, mut decoded) = intermediate(parent)?;
    let (_, source) =
        decode_file_to_wave(input, &mut decoded, |completed, total| progress(TransferProgress { stage: TransferStage::Decode, completed, total }))
            .map_err(error)?;
    let mut reader = WavReader::from_reader(decoded).map_err(error)?;
    let converter = WindowedResampler::new(source.sample_rate, rate, source.frames, usize::from(source.channels)).map_err(EngineError::Io)?;
    let total = converter.output_frames();
    if total == 0 {
        return Err(EngineError::Io("audio is shorter than one destination sample".into()));
    }
    if !progress(TransferProgress { stage: TransferStage::Resample, completed: 0, total: Some(total) }) {
        return Err(cancelled());
    }
    let mut writer = FloatWavWriter::new(output, usize::from(source.channels), rate, total).map_err(error)?;
    let mut position = 0u64;
    while position < total {
        let count = (total - position).min(converter.recommended_block_frames() as u64) as usize;
        let range = converter.input_window(position, count).map_err(EngineError::Io)?;
        let input_frames = usize::try_from(range.end - range.start).map_err(|_| EngineError::Io("SRC window frame overflow".into()))?;
        let block = reader.read_frames(range.start, input_frames).map_err(error)?;
        if block.frames() != input_frames {
            return Err(EngineError::Io("incomplete SRC file window".into()));
        }
        let channels = converter.process_window(&block.channels, range.start, position, count).map_err(EngineError::Io)?;
        writer.write_frames(&AudioBuffer { sample_rate: rate, channels }).map_err(error)?;
        position += count as u64;
        if !progress(TransferProgress { stage: TransferStage::Resample, completed: position, total: Some(total) }) {
            return Err(cancelled());
        }
    }
    let output = writer.finish().map_err(error)?;
    output.sync_all().map_err(|e| EngineError::Io(e.to_string()))?;
    let output_bytes = output.metadata().map_err(|e| EngineError::Io(e.to_string()))?.len();
    Ok(TransferReceipt { source, output_rate: rate, output_frames: total, output_bytes })
}
