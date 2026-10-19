//! Audio sources (files in the session's Audio Files folder) and their decoded data, and the
//! movies video clips reference.

use crate::SourceId;
use soundcraft_audio_io::{AudioBuffer, FileFormat, peaks::Peaks};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Metadata for an audio file used by the session (serialised).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub name: String,
    /// Path relative to the session folder (or absolute for referenced media).
    pub path: String,
    pub channels: u16,
    pub frames: u64,
    pub sample_rate: u32,
    pub format: FileFormat,
    /// Original BWF time stamp (samples), if any.
    #[serde(default)]
    pub time_reference: u64,
    /// True when the file has not been written to disk yet (recorded/rendered this session).
    #[serde(default)]
    pub unsaved: bool,
}

/// A movie used by Video track clips (serialised). The pictures themselves are never stored in
/// the session: the UI opens the movie from `path` and decodes frames on demand.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VideoSource {
    pub id: SourceId,
    pub name: String,
    /// Absolute path of the movie (referenced, never copied into the session folder).
    pub path: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Frames per second (0 = unknown).
    #[serde(default)]
    pub frame_rate: f64,
    /// Picture length in seconds.
    #[serde(default)]
    pub duration: f64,
    /// Codec id (`h264`, `prores`, `mjpeg`, `hevc`, …).
    #[serde(default)]
    pub codec: String,
}

impl VideoSource {
    /// Display aspect ratio (width / height), 16:9 when unknown.
    pub fn aspect(&self) -> f64 {
        if self.width == 0 || self.height == 0 { 16.0 / 9.0 } else { f64::from(self.width) / f64::from(self.height) }
    }
}

/// Resident or disk-backed audio plus waveform overviews (not serialised).
#[derive(Debug)]
pub struct SourceAudio {
    /// Legacy resident PCM. Disk sources keep only the sample rate here; use backend-aware
    /// metadata/reads and explicit offline residency instead of inspecting channels.
    pub buffer: AudioBuffer,
    pub peaks: Vec<Peaks>,
    #[cfg(not(target_arch = "wasm32"))]
    stream: Option<soundcraft_audio_io::stream_cache::StreamSource>,
}

impl SourceAudio {
    pub fn new(buffer: AudioBuffer) -> Self {
        let peaks = buffer.channels.iter().map(|c| Peaks::build(c)).collect();
        SourceAudio {
            buffer,
            peaks,
            #[cfg(not(target_arch = "wasm32"))]
            stream: None,
        }
    }
    pub fn frames(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(source) = &self.stream {
            return usize::try_from(source.info().frames).unwrap_or(usize::MAX);
        }
        self.buffer.frames()
    }
    pub fn channels(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(source) = &self.stream {
            return usize::from(source.info().channels);
        }
        self.buffer.num_channels()
    }
    pub fn sample_rate(&self) -> u32 {
        self.buffer.sample_rate
    }
    pub fn is_resident(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.stream.is_some() {
            return false;
        }
        true
    }
    /// Metadata/audio validity without fetching disk PCM or allocating on the render thread.
    pub fn has_uniform_channels(&self) -> bool {
        if !self.is_resident() {
            return self.channels() > 0;
        }
        !self.buffer.channels.is_empty() && self.buffer.channels.iter().all(|ch| ch.len() == self.frames())
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stream_source(&self) -> Option<&soundcraft_audio_io::stream_cache::StreamSource> {
        self.stream.as_ref()
    }

    /// Worker/control-thread construction. Disk media retains metadata and bounded overviews,
    /// not a whole-file PCM vector. Callers must use backend-aware reads/render views.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_stream(
        source: soundcraft_audio_io::stream_cache::StreamSource,
        overview_blocks: usize,
        progress: impl FnMut(u64, u64) -> bool,
    ) -> soundcraft_audio_io::Result<Self> {
        usize::try_from(source.info().frames).map_err(|_| soundcraft_audio_io::AudioError::TooLarge("source exceeds host frame range".into()))?;
        let mut reader = source.open_reader()?;
        let peaks = soundcraft_audio_io::stream::build_overviews(&mut reader, overview_blocks, progress)?;
        Ok(Self { buffer: AudioBuffer { sample_rate: source.info().sample_rate, channels: Vec::new() }, peaks, stream: Some(source) })
    }

    /// Blocking bounded offline read. Never invoke from the audio callback; realtime clients
    /// must prepare an independent StreamView and acquire a cache window instead.
    pub fn read_frames(&self, start: usize, count: usize) -> soundcraft_audio_io::Result<AudioBuffer> {
        if self.channels() > 64 {
            return Err(soundcraft_audio_io::AudioError::Unsupported("source read supports up to 64 channels".into()));
        }
        if count > 65_536 {
            return Err(soundcraft_audio_io::AudioError::TooLarge("source read exceeds 65,536 frames".into()));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(source) = &self.stream {
            return source.open_reader()?.read_frames(start as u64, count);
        }
        let end = start.saturating_add(count).min(self.frames());
        let channels = self.buffer.channels.iter().map(|ch| ch.get(start..end).unwrap_or_default().to_vec()).collect();
        Ok(AudioBuffer { sample_rate: self.sample_rate(), channels })
    }

    /// Explicit compatibility copy for offline APIs that require resident PCM. The caller owns
    /// the sample budget and temporary result; neither this source nor Undo snapshots change.
    pub fn resident_copy(&self, max_samples: usize) -> soundcraft_audio_io::Result<Arc<Self>> {
        if self.channels() > 64 || !self.has_uniform_channels() {
            return Err(soundcraft_audio_io::AudioError::Malformed("offline source channels are invalid".into()));
        }
        let frames = self.frames();
        if frames.checked_mul(self.channels()).is_none_or(|n| n > max_samples) {
            return Err(soundcraft_audio_io::AudioError::TooLarge("offline source residency exceeds the PCM budget".into()));
        }
        let mut channels = Vec::new();
        for _ in 0..self.channels() {
            let mut ch = Vec::new();
            ch.try_reserve_exact(frames).map_err(|e| soundcraft_audio_io::AudioError::TooLarge(e.to_string()))?;
            channels.push(ch);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let mut reader = self.stream.as_ref().map(|source| source.open_reader()).transpose()?;
        let mut start = 0usize;
        while start < frames {
            let count = (frames - start).min(65_536 / self.channels().max(1));
            #[cfg(not(target_arch = "wasm32"))]
            let block = if let Some(reader) = &mut reader { reader.read_frames(start as u64, count)? } else { self.read_frames(start, count)? };
            #[cfg(target_arch = "wasm32")]
            let block = self.read_frames(start, count)?;
            if block.frames() != count || block.num_channels() != self.channels() {
                return Err(soundcraft_audio_io::AudioError::Malformed("incomplete source read during offline preparation".into()));
            }
            for (dst, samples) in channels.iter_mut().zip(block.channels) {
                dst.extend_from_slice(&samples);
            }
            start += count;
        }
        Ok(Arc::new(Self {
            buffer: AudioBuffer { sample_rate: self.sample_rate(), channels },
            peaks: self.peaks.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            stream: None,
        }))
    }
    pub fn channel(&self, ch: usize) -> Option<&[f32]> {
        self.buffer.channels.get(ch).map(Vec::as_slice)
    }
}

/// Shared decoded audio, keyed by source id.
#[derive(Debug, Clone, Default)]
pub struct SourcePool {
    map: BTreeMap<SourceId, Arc<SourceAudio>>,
}

impl SourcePool {
    pub fn insert(&mut self, id: SourceId, audio: Arc<SourceAudio>) {
        self.map.insert(id, audio);
    }
    pub fn get(&self, id: SourceId) -> Option<&Arc<SourceAudio>> {
        self.map.get(&id)
    }
    pub fn remove(&mut self, id: SourceId) -> Option<Arc<SourceAudio>> {
        self.map.remove(&id)
    }
    pub fn contains(&self, id: SourceId) -> bool {
        self.map.contains_key(&id)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    /// Temporary offline compatibility snapshot, preserving every source id and sharing already
    /// resident sources. Validate the total new PCM allocation before reading any source.
    pub fn resident_snapshot(&self, max_samples: usize) -> soundcraft_audio_io::Result<Self> {
        let needed = self
            .map
            .values()
            .filter(|a| !a.is_resident())
            .try_fold(0usize, |total, audio| total.checked_add(audio.frames().checked_mul(audio.channels())?));
        if needed.is_none_or(|n| n > max_samples) {
            return Err(soundcraft_audio_io::AudioError::TooLarge("offline pool residency exceeds the PCM budget".into()));
        }
        let mut snapshot = self.clone();
        for (&id, audio) in &self.map {
            if !audio.is_resident() {
                snapshot.map.insert(id, audio.resident_copy(max_samples)?);
            }
        }
        Ok(snapshot)
    }
    pub fn ids(&self) -> impl Iterator<Item = SourceId> + '_ {
        self.map.keys().copied()
    }
}

impl PartialEq for SourcePool {
    fn eq(&self, other: &Self) -> bool {
        self.map.len() == other.map.len() && self.map.iter().zip(other.map.iter()).all(|((a, x), (b, y))| a == b && Arc::ptr_eq(x, y))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)]
mod backend_tests {
    use super::*;
    use soundcraft_audio_io::{
        BitDepth, EncodeOptions,
        stream_cache::{CacheConfig, StreamCache},
    };
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("sculpt-source-{}-{}.wav", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
            let audio = audio();
            let bytes =
                soundcraft_audio_io::encode(&audio, &EncodeOptions { bit_depth: BitDepth::Float32, dither: false, ..EncodeOptions::default() })
                    .unwrap();
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn audio() -> AudioBuffer {
        AudioBuffer { sample_rate: 48_000, channels: (0..7).map(|ch| (0..4103).map(|f| (f as f32 * 0.017 + ch as f32).sin()).collect()).collect() }
    }
    #[test]
    fn disk_source_metadata_ranges_and_residency_are_exact() {
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let source = cache.open_source(&fixture.0).unwrap();
        let disk = SourceAudio::from_stream(source, 70, |_, _| true).unwrap();
        let original = audio();
        assert_eq!(disk.frames(), 4103);
        assert_eq!(disk.channels(), 7);
        assert_eq!(disk.sample_rate(), 48_000);
        assert!(disk.buffer.channels.is_empty());
        assert!(!disk.is_resident());
        assert!(disk.channel(0).is_none());
        assert!(disk.has_uniform_channels());
        let part = disk.read_frames(137, 600).unwrap();
        for (actual, expected) in part.channels.iter().zip(&original.channels) {
            assert_eq!(actual, &expected[137..737]);
        }
        assert!(disk.resident_copy(4103 * 7 - 1).is_err());
        let resident = disk.resident_copy(4103 * 7).unwrap();
        assert_eq!(resident.buffer, original);
        assert_eq!(resident.peaks, disk.peaks);
        assert!(resident.is_resident());
        assert!(!disk.is_resident());
        assert!(disk.buffer.channels.is_empty());
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn offline_pool_snapshot_preserves_ids_shares_resident_and_leaves_original_unchanged() {
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut pool = SourcePool::default();
        let resident = Arc::new(SourceAudio::new(audio()));
        pool.insert(SourceId(3), Arc::clone(&resident));
        for id in [7, 19] {
            let source = cache.open_source(&fixture.0).unwrap();
            pool.insert(SourceId(id), Arc::new(SourceAudio::from_stream(source, 70, |_, _| true).unwrap()));
        }
        assert!(pool.resident_snapshot(4103 * 7 * 2 - 1).is_err());
        let snapshot = pool.resident_snapshot(4103 * 7 * 2).unwrap();
        assert_eq!(snapshot.ids().collect::<Vec<_>>(), vec![SourceId(3), SourceId(7), SourceId(19)]);
        assert!(Arc::ptr_eq(snapshot.get(SourceId(3)).unwrap(), &resident));
        for id in [7, 19] {
            assert_eq!(snapshot.get(SourceId(id)).unwrap().buffer, audio());
            assert!(!pool.get(SourceId(id)).unwrap().is_resident());
        }
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn invalid_ranges_cancelled_preparation_and_media_changes_are_errors() {
        let resident = SourceAudio::new(audio());
        assert!(resident.read_frames(0, 65_537).is_err());
        assert_eq!(resident.read_frames(usize::MAX, 10).unwrap().frames(), 0);
        let malformed = SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 2], vec![0.0; 1]] });
        assert!(!malformed.has_uniform_channels());
        assert!(malformed.resident_copy(100).is_err());
        let wide = SourceAudio::new(AudioBuffer::new(48_000, 65_537, 1));
        assert!(wide.resident_copy(usize::MAX).is_err());
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let source = cache.open_source(&fixture.0).unwrap();
        assert!(SourceAudio::from_stream(source.clone(), 70, |_, _| false).is_err());
        let disk = SourceAudio::from_stream(source, 70, |_, _| true).unwrap();
        std::fs::OpenOptions::new().write(true).open(&fixture.0).unwrap().set_len(100).unwrap();
        assert!(disk.read_frames(0, 10).is_err());
        assert!(disk.resident_copy(4103 * 7).is_err());
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
}
