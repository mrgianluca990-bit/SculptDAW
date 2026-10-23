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

    /// Rebind a verified canonical copy without rescanning its waveform. Control thread only.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn rebind_stream(&self, source: soundcraft_audio_io::stream_cache::StreamSource) -> soundcraft_audio_io::Result<Self> {
        if source.info().frames != self.frames() as u64
            || usize::from(source.info().channels) != self.channels()
            || source.info().sample_rate != self.sample_rate()
        {
            return Err(soundcraft_audio_io::AudioError::Malformed("copied source metadata differs".into()));
        }
        Ok(Self { buffer: AudioBuffer { sample_rate: self.sample_rate(), channels: Vec::new() }, peaks: self.peaks.clone(), stream: Some(source) })
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

    /// Write canonical float WAV/RF64 without a whole-file PCM or encoded byte copy.
    /// Blocking control/worker operation. Callers publish only after success and discard the
    /// destination on failure/cancellation; this does not make destination files atomic.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write_wave<W: std::io::Write>(&self, destination: W, mut progress: impl FnMut(u64, u64) -> bool) -> soundcraft_audio_io::Result<W> {
        use soundcraft_audio_io::{AudioError, stream_write::FloatWavWriter};
        if !self.has_uniform_channels() || !(1..=64).contains(&self.channels()) {
            return Err(AudioError::Encode("invalid source for WAV transfer".into()));
        }
        let total = u64::try_from(self.frames()).map_err(|_| AudioError::TooLarge("source frame count overflow".into()))?;
        if !progress(0, total) {
            return Err(AudioError::Io("WAV transfer cancelled".into()));
        }
        let mut reader = self.stream.as_ref().map(|s| s.open_reader()).transpose()?;
        let mut writer = FloatWavWriter::new(destination, self.channels(), self.sample_rate(), total)?;
        let chunk = 65_536 / self.channels();
        let mut offset = 0usize;
        while offset < self.frames() {
            let frames = chunk.min(self.frames() - offset);
            let block = if let Some(reader) = &mut reader { reader.read_frames(offset as u64, frames)? } else { self.read_frames(offset, frames)? };
            if block.frames() != frames {
                return Err(AudioError::Io("incomplete source during WAV transfer".into()));
            }
            writer.write_frames(&block)?;
            offset += frames;
            if !progress(offset as u64, total) {
                return Err(AudioError::Io("WAV transfer cancelled".into()));
            }
        }
        // Revalidate media even if its final chunk was cached by the OS or a caller edited it
        // from the progress hook. Never publish an apparently successful mixed-version copy.
        if let Some(source) = &self.stream {
            source.open_reader()?;
        }
        writer.finish()
    }

    /// Blocking region preparation with an explicit total sample budget. Only samples beyond
    /// the known source end are padded; incomplete in-file reads fail without partial output.
    pub fn read_region(&self, start: usize, count: usize, max_samples: usize) -> soundcraft_audio_io::Result<AudioBuffer> {
        self.read_region_with_progress(start, count, max_samples, |_, _| true)
    }

    /// Control/worker-thread variant with cancellation before allocation and between chunks.
    pub fn read_region_with_progress(
        &self,
        start: usize,
        count: usize,
        max_samples: usize,
        mut progress: impl FnMut(usize, usize) -> bool,
    ) -> soundcraft_audio_io::Result<AudioBuffer> {
        use soundcraft_audio_io::AudioError;
        let channels = self.channels();
        if channels == 0 || channels > 64 || !self.has_uniform_channels() {
            return Err(AudioError::Malformed("offline source channels are invalid".into()));
        }
        if count.checked_mul(channels).is_none_or(|n| n > max_samples) {
            return Err(AudioError::TooLarge("offline region exceeds the PCM budget".into()));
        }
        if !progress(0, count) {
            return Err(AudioError::Io("audio region preparation cancelled".into()));
        }
        let available = self.frames().saturating_sub(start).min(count);
        #[cfg(not(target_arch = "wasm32"))]
        let mut reader = if available > 0 { self.stream.as_ref().map(|source| source.open_reader()).transpose()? } else { None };
        let mut out = Vec::new();
        out.try_reserve_exact(channels).map_err(|e| AudioError::TooLarge(e.to_string()))?;
        for _ in 0..channels {
            let mut channel = Vec::new();
            channel.try_reserve_exact(count).map_err(|e| AudioError::TooLarge(e.to_string()))?;
            channel.resize(count, 0.0);
            out.push(channel);
        }
        let mut done = 0usize;
        while done < available {
            if done > 0 && !progress(done, count) {
                return Err(AudioError::Io("audio region preparation cancelled".into()));
            }
            let n = (available - done).min(65_536 / channels);
            let at = start.checked_add(done).ok_or_else(|| AudioError::TooLarge("audio region offset overflow".into()))?;
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(reader) = &mut reader {
                let block = reader.read_frames(at as u64, n)?;
                if block.frames() != n || block.num_channels() != channels {
                    return Err(AudioError::Malformed("incomplete source region".into()));
                }
                for (dst, src) in out.iter_mut().zip(block.channels) {
                    let dst = dst.get_mut(done..done + n).ok_or_else(|| AudioError::Malformed("offline output region bounds".into()))?;
                    if src.len() != dst.len() {
                        return Err(AudioError::Malformed("incomplete source channel".into()));
                    }
                    dst.copy_from_slice(&src);
                }
                done += n;
                continue;
            }
            for (dst, src) in out.iter_mut().zip(&self.buffer.channels) {
                let input = src.get(at..at + n).ok_or_else(|| AudioError::Malformed("incomplete resident source region".into()))?;
                let dst = dst.get_mut(done..done + n).ok_or_else(|| AudioError::Malformed("offline output region bounds".into()))?;
                dst.copy_from_slice(input);
            }
            done += n;
        }
        if count > 0 && !progress(count, count) {
            return Err(AudioError::Io("audio region preparation cancelled".into()));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if reader.is_some()
            && let Some(source) = &self.stream
        {
            // Detect replacement/truncation even when it occurred after the final read.
            source.open_reader()?;
        }
        Ok(AudioBuffer { sample_rate: self.sample_rate(), channels: out })
    }

    /// Explicit compatibility copy for offline APIs that require resident PCM. The caller owns
    /// the sample budget and temporary result; neither this source nor Undo snapshots change.
    pub fn resident_copy(&self, max_samples: usize) -> soundcraft_audio_io::Result<Arc<Self>> {
        let buffer = self.read_region(0, self.frames(), max_samples)?;
        Ok(Arc::new(Self {
            buffer,
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
    #[cfg(not(target_arch = "wasm32"))]
    cache: Arc<ProjectCache>,
}

/// Shared even by snapshots taken before the first disk source is loaded. Undo cannot create
/// a second cache for the same project merely by returning to an empty source pool.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct ProjectCache(std::sync::Mutex<Option<Arc<soundcraft_audio_io::stream_cache::StreamCache>>>);

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for ProjectCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProjectCache")
    }
}

impl SourcePool {
    /// Lazy project-owned cache. Control/worker threads only: this may lock, allocate and
    /// start a disk worker. PCM sources and undo snapshots share this owner.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stream_cache(&self) -> soundcraft_audio_io::Result<Arc<soundcraft_audio_io::stream_cache::StreamCache>> {
        let mut owner = self.cache.0.lock().map_err(|_| soundcraft_audio_io::AudioError::Io("project cache lock failed".into()))?;
        if let Some(cache) = owner.as_ref() {
            return Ok(Arc::clone(cache));
        }
        if let Some(source) = self.map.values().find_map(|audio| audio.stream_source()) {
            let cache = Arc::clone(source.cache());
            *owner = Some(Arc::clone(&cache));
            return Ok(cache);
        }
        let cache = soundcraft_audio_io::stream_cache::StreamCache::new(soundcraft_audio_io::stream_cache::CacheConfig::default())?;
        *owner = Some(Arc::clone(&cache));
        Ok(cache)
    }
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
        self.resident_snapshot_for(self.ids(), max_samples)
    }

    /// Prepare only requested sources, preserving all other identities/backends. Duplicate
    /// ids count once; missing ids and the full allocation budget are checked before reads.
    pub fn resident_snapshot_for(&self, ids: impl IntoIterator<Item = SourceId>, max_samples: usize) -> soundcraft_audio_io::Result<Self> {
        let ids: std::collections::BTreeSet<_> = ids.into_iter().collect();
        if ids.iter().any(|id| !self.map.contains_key(id)) {
            return Err(soundcraft_audio_io::AudioError::Malformed("offline snapshot source is missing".into()));
        }
        let needed = ids
            .iter()
            .filter_map(|id| self.map.get(id))
            .filter(|a| !a.is_resident())
            .try_fold(0usize, |total, audio| total.checked_add(audio.frames().checked_mul(audio.channels())?));
        if needed.is_none_or(|n| n > max_samples) {
            return Err(soundcraft_audio_io::AudioError::TooLarge("offline pool residency exceeds the PCM budget".into()));
        }
        let mut snapshot = self.clone();
        for id in ids {
            let audio = self.map.get(&id).ok_or_else(|| soundcraft_audio_io::AudioError::Malformed("offline snapshot source is missing".into()))?;
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
    #[test]
    fn project_cache_is_lazy_shared_by_empty_undo_snapshots_and_unique_per_project() {
        let pool = SourcePool::default();
        let before = pool.clone();
        assert!(pool.cache.0.lock().unwrap().is_none());
        let other = before.clone();
        let worker = std::thread::spawn(move || other.stream_cache().unwrap());
        let cache = pool.stream_cache().unwrap();
        assert!(Arc::ptr_eq(&cache, &worker.join().unwrap()));
        assert!(Arc::ptr_eq(&cache, &before.stream_cache().unwrap()));
        let next_project = SourcePool::default();
        assert!(!Arc::ptr_eq(&pool.cache, &next_project.cache));
        assert!(next_project.cache.0.lock().unwrap().is_none());
        assert_eq!(pool, before);
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
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
    fn wave_transfer_matches_disk_and_resident_and_rejects_cancelled_or_changed_media() {
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let disk = SourceAudio::from_stream(cache.open_source(&fixture.0).unwrap(), 70, |_, _| true).unwrap();
        let resident = SourceAudio::new(audio());
        let expected = std::fs::read(&fixture.0).unwrap();
        assert_eq!(disk.write_wave(Vec::new(), |_, _| true).unwrap(), expected);
        assert_eq!(resident.write_wave(Vec::new(), |_, _| true).unwrap(), expected);
        assert!(disk.buffer.channels.is_empty());
        let mut cancelled = Vec::new();
        assert!(disk.write_wave(&mut cancelled, |_, _| false).is_err());
        assert!(cancelled.is_empty());
        assert!(disk.write_wave(Vec::new(), |done, _| done == 0).is_err());
        assert!(
            disk.write_wave(Vec::new(), |done, total| {
                if done == total {
                    std::fs::OpenOptions::new().write(true).open(&fixture.0).unwrap().set_len(100).unwrap();
                }
                true
            })
            .is_err()
        );
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn wave_transfer_is_chunked_and_rejects_ragged_sources() {
        let resident = SourceAudio::new(AudioBuffer::new(48_000, 7, 30_000));
        let mut calls = Vec::new();
        let bytes = resident
            .write_wave(Vec::new(), |done, total| {
                calls.push((done, total));
                true
            })
            .unwrap();
        assert!(calls.len() > 3);
        assert!(calls.windows(2).all(|w| w[1].0 - w[0].0 <= 65_536 / 7));
        assert_eq!(calls.last(), Some(&(30_000, 30_000)));
        assert_eq!(soundcraft_audio_io::decode(&bytes, None).unwrap().1, resident.buffer);
        let ragged = SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 2], vec![0.0]] });
        let mut destination = Vec::new();
        assert!(ragged.write_wave(&mut destination, |_, _| true).is_err());
        assert!(destination.is_empty());
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
    fn selected_regions_match_disk_and_resident_with_explicit_eof_padding_and_budget() {
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let disk = SourceAudio::from_stream(cache.open_source(&fixture.0).unwrap(), 70, |_, _| true).unwrap();
        let resident = SourceAudio::new(audio());
        for (start, count) in [(137, 600), (4000, 1000), (usize::MAX, 10), (0, 70_000)] {
            let result = disk.read_region(start, count, count * 7).unwrap();
            assert_eq!(result, resident.read_region(start, count, count * 7).unwrap());
            assert_eq!(result.frames(), count);
        }
        let padded = disk.read_region(4000, 1000, 7000).unwrap();
        assert!(padded.channels.iter().all(|ch| ch[103..].iter().all(|&x| x == 0.0)));
        assert!(disk.read_region(0, usize::MAX, usize::MAX).is_err());
        assert!(disk.read_region(0, 600, 4199).is_err());
        assert!(!disk.is_resident());
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn region_cancellation_and_changes_at_completion_return_no_partial_result() {
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let disk = SourceAudio::from_stream(cache.open_source(&fixture.0).unwrap(), 70, |_, _| true).unwrap();
        assert!(disk.read_region_with_progress(0, 600, 4200, |_, _| false).is_err());
        assert!(disk.read_region_with_progress(0, 600, 4200, |at, total| at != total).is_err());
        let result = disk.read_region_with_progress(0, 600, 4200, |at, total| {
            if at == total {
                std::fs::OpenOptions::new().write(true).open(&fixture.0).unwrap().set_len(100).unwrap();
            }
            true
        });
        assert!(result.is_err());
        assert!(!disk.is_resident());
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
    fn selected_snapshot_counts_duplicate_ids_once_and_preserves_unrequested_backends() {
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut pool = SourcePool::default();
        for id in [7, 19] {
            pool.insert(SourceId(id), Arc::new(SourceAudio::from_stream(cache.open_source(&fixture.0).unwrap(), 70, |_, _| true).unwrap()));
        }
        let selected = pool.resident_snapshot_for([SourceId(7), SourceId(7)], 4103 * 7).unwrap();
        assert_eq!(selected.get(SourceId(7)).unwrap().buffer, audio());
        assert!(Arc::ptr_eq(selected.get(SourceId(19)).unwrap(), pool.get(SourceId(19)).unwrap()));
        assert!(!pool.get(SourceId(7)).unwrap().is_resident());
        assert!(pool.resident_snapshot_for([SourceId(7), SourceId(19)], 4103 * 7).is_err());
        assert!(pool.resident_snapshot_for([SourceId(999)], usize::MAX).is_err());
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
