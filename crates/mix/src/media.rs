//! Prepared disk media for a mix snapshot. Construction and `prepare` are control-thread work;
//! `request` and window acquisition only update atomics and pin existing PCM pages.
use soundcraft_audio_io::stream_cache::{CacheError, CacheWindow, StreamCache, StreamView};
use soundcraft_model::{Clip, ClipContent, ClipId, Session, SourceAudio, SourceId};
use soundcraft_time::{Range, Samples};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::time::{Duration, Instant};

struct Entry {
    clip: Clip,
    audio: Arc<SourceAudio>,
    view: StreamView,
}

const MAX_PREPARED_SAMPLE_VALUES: usize = (64 * 1024 * 1024) / 4;

struct InstrumentSample {
    original: Arc<SourceAudio>,
    resident: Arc<SourceAudio>,
}

pub struct PreparedMedia {
    entries: HashMap<ClipId, Entry>,
    samples: HashMap<SourceId, InstrumentSample>,
    error: AtomicU8,
    caches: Vec<Arc<StreamCache>>,
}

impl Default for PreparedMedia {
    fn default() -> Self {
        Self { entries: HashMap::new(), samples: HashMap::new(), error: AtomicU8::new(0), caches: Vec::new() }
    }
}

impl PreparedMedia {
    /// Create independent positions for every disk-backed clip, outside the callback. Muted and
    /// inactive clips do not demand PCM; a changed document needs a newly prepared snapshot.
    pub fn new(s: &Session) -> Result<Self, CacheError> {
        Self::with_previous(s, None)
    }

    /// Reuse instrument PCM when clip geometry changes. All reads/copies happen here on the
    /// control thread, never when the instrument is synchronized in the render callback.
    pub fn with_previous(s: &Session, previous: Option<&Self>) -> Result<Self, CacheError> {
        Self::with_sample_budget(s, previous, MAX_PREPARED_SAMPLE_VALUES)
    }

    fn with_sample_budget(s: &Session, previous: Option<&Self>, sample_budget: usize) -> Result<Self, CacheError> {
        let mut result = Self::default();
        let mut sample_values = 0usize;
        for track in s.tracks.iter().filter(|t| !t.inactive) {
            if let Some(id) = track.instrument.as_ref().and_then(|i| i.sample)
                && !result.samples.contains_key(&id)
            {
                let audio = s.pool.get(id).ok_or(CacheError::ReadFailed)?;
                if !audio.has_uniform_channels() || audio.frames() == 0 {
                    return Err(CacheError::ReadFailed);
                }
                let resident = if audio.is_resident() {
                    Arc::clone(audio)
                } else {
                    let size = audio.frames().checked_mul(audio.channels()).ok_or(CacheError::Capacity)?;
                    sample_values = sample_values.checked_add(size).filter(|n| *n <= sample_budget).ok_or(CacheError::Capacity)?;
                    if let Some(prepared) = previous.and_then(|p| p.samples.get(&id)).filter(|p| Arc::ptr_eq(&p.original, audio)) {
                        Arc::clone(&prepared.resident)
                    } else {
                        audio.resident_copy(size).map_err(|_| CacheError::ReadFailed)?
                    }
                };
                result.samples.insert(id, InstrumentSample { original: Arc::clone(audio), resident });
            }
            for clip in track.clips().iter().filter(|c| !c.muted) {
                let ClipContent::Audio { source, .. } = clip.content else { continue };
                let Some(audio) = s.pool.get(source) else { continue };
                let Some(source) = audio.stream_source() else { continue };
                if !result.caches.iter().any(|cache| Arc::ptr_eq(cache, source.cache())) {
                    if result.caches.len() >= 64 {
                        return Err(CacheError::Capacity);
                    }
                    result.caches.push(Arc::clone(source.cache()));
                }
                let entry = Entry { clip: clip.clone(), audio: Arc::clone(audio), view: source.view()? };
                result.entries.insert(clip.id, entry);
            }
        }
        Ok(result)
    }

    /// Compare media identities and clip geometry on the control thread. Mixer/automation-only
    /// edits reuse prepared views rather than registering new positions every UI frame.
    pub fn matches(&self, s: &Session) -> bool {
        let mut count = 0usize;
        let mut sample_ids = std::collections::HashSet::new();
        for track in s.tracks.iter().filter(|t| !t.inactive) {
            if let Some(id) = track.instrument.as_ref().and_then(|i| i.sample) {
                sample_ids.insert(id);
                if !s.pool.get(id).is_some_and(|audio| self.samples.get(&id).is_some_and(|sample| Arc::ptr_eq(&sample.original, audio))) {
                    return false;
                }
            }
            for clip in track.clips().iter().filter(|c| !c.muted) {
                let ClipContent::Audio { source, .. } = clip.content else { continue };
                let Some(audio) = s.pool.get(source) else { continue };
                if audio.stream_source().is_none() {
                    continue;
                }
                count += 1;
                if !self.entries.get(&clip.id).is_some_and(|entry| entry.clip == *clip && Arc::ptr_eq(&entry.audio, audio)) {
                    return false;
                }
            }
        }
        count == self.entries.len() && sample_ids.len() == self.samples.len()
    }

    pub(crate) fn instrument_sample(&self, id: SourceId, original: &Arc<SourceAudio>) -> Option<&Arc<SourceAudio>> {
        self.samples.get(&id).filter(|sample| Arc::ptr_eq(&sample.original, original)).map(|sample| &sample.resident)
    }

    /// Request current and approaching clips, and a loop destination. No waits, I/O or allocation.
    /// Inactive requests are released so an entire timeline does not accumulate in the pool.
    pub fn request(&self, pos: Samples, frames: usize, anchor: Option<Samples>) -> Result<(), CacheError> {
        self.request_inner(pos, frames, anchor, true)
    }
    pub(crate) fn request_exact(&self, pos: Samples, frames: usize, anchor: Option<Samples>) -> Result<(), CacheError> {
        self.request_inner(pos, frames, anchor, false)
    }
    fn request_inner(&self, pos: Samples, frames: usize, anchor: Option<Samples>, prefetch: bool) -> Result<(), CacheError> {
        let _batches: [Option<_>; 64] = std::array::from_fn(|i| self.caches.get(i).map(|cache| cache.begin_requests()));
        for entry in self.entries.values() {
            let current = demand(&entry.clip, &entry.audio, pos, frames);
            let approaching = if prefetch && current.is_none() && entry.clip.start > pos && entry.clip.start <= pos.saturating_add(16_384) {
                demand(&entry.clip, &entry.audio, entry.clip.start, frames)
            } else {
                None
            };
            let target = anchor.and_then(|at| demand(&entry.clip, &entry.audio, at, frames));
            if let Some((start, count)) = current.or(approaching) {
                entry.view.request(start, count)?;
                entry.view.set_anchor(target)?;
            } else if let Some(target) = target {
                entry.view.request(target.0, target.1)?;
                entry.view.set_anchor(None)?;
            } else {
                entry.view.deactivate();
            }
        }
        Ok(())
    }

    /// Wait once for the whole snapshot using one shared deadline. All requests are published
    /// before waiting, so budget conflicts are detected across the complete active mix.
    pub fn prepare(&self, pos: Samples, frames: usize, anchor: Option<Samples>, timeout: Duration) -> Result<(), CacheError> {
        self.prepare_inner(pos, frames, anchor, timeout, true)
    }
    pub(crate) fn prepare_exact(&self, pos: Samples, frames: usize, anchor: Option<Samples>, timeout: Duration) -> Result<(), CacheError> {
        self.prepare_inner(pos, frames, anchor, timeout, false)
    }
    fn prepare_inner(&self, pos: Samples, frames: usize, anchor: Option<Samples>, timeout: Duration, prefetch: bool) -> Result<(), CacheError> {
        self.request_inner(pos, frames, anchor, prefetch)?;
        let began = Instant::now();
        for entry in self.entries.values() {
            let current = demand(&entry.clip, &entry.audio, pos, frames);
            let approaching = if prefetch && current.is_none() && entry.clip.start > pos && entry.clip.start <= pos.saturating_add(16_384) {
                demand(&entry.clip, &entry.audio, entry.clip.start, frames)
            } else {
                None
            };
            let target = anchor.and_then(|at| demand(&entry.clip, &entry.audio, at, frames));
            if let Some((start, count)) = current.or(approaching).or(target) {
                entry.view.prepare(start, count, timeout.saturating_sub(began.elapsed()))?;
            }
        }
        Ok(())
    }

    pub fn deactivate(&self) {
        for entry in self.entries.values() {
            entry.view.deactivate();
        }
    }

    pub(crate) fn window(&self, clip: &Clip, audio: &Arc<SourceAudio>, pos: Samples, frames: usize) -> Result<Option<CacheWindow<'_>>, CacheError> {
        let Some((start, count)) = demand(clip, audio, pos, frames) else { return Ok(None) };
        let entry = self.entries.get(&clip.id).filter(|e| Arc::ptr_eq(&e.audio, audio)).ok_or(CacheError::NotReady)?;
        entry.view.try_window(start, count).map(Some)
    }

    pub(crate) fn fail(&self, error: CacheError) {
        let code = match error {
            CacheError::NotReady => 1,
            CacheError::ReadFailed => 2,
            CacheError::MediaChanged => 3,
            CacheError::Capacity => 4,
            CacheError::Stopped => 5,
            CacheError::InvalidRange => 6,
            CacheError::Timeout => 7,
        };
        let _ = self.error.compare_exchange(0, code, Ordering::AcqRel, Ordering::Relaxed);
    }

    pub fn take_error(&self) -> Option<CacheError> {
        match self.error.swap(0, Ordering::AcqRel) {
            1 => Some(CacheError::NotReady),
            2 => Some(CacheError::ReadFailed),
            3 => Some(CacheError::MediaChanged),
            4 => Some(CacheError::Capacity),
            5 => Some(CacheError::Stopped),
            6 => Some(CacheError::InvalidRange),
            7 => Some(CacheError::Timeout),
            _ => None,
        }
    }
    pub(crate) fn failed(&self) -> bool {
        self.error.load(Ordering::Acquire) != 0
    }
}

/// Conservative source span including the interpolation neighbour. Negative/out-of-file parts
/// retain the existing clip-bounds silence semantics; missing in-file pages are explicit errors.
fn demand(clip: &Clip, audio: &SourceAudio, pos: Samples, frames: usize) -> Option<(u64, usize)> {
    let ClipContent::Audio { offset, .. } = clip.content else { return None };
    let block = Range { start: pos, end: pos.saturating_add(i64::try_from(frames).unwrap_or(i64::MAX)) };
    let isect = clip.range().intersect(&block)?;
    let stretch = if clip.stretch.is_finite() && clip.stretch > 0.0 { clip.stretch } else { 1.0 };
    let first = offset as f64 + isect.start.saturating_sub(clip.start) as f64 / stretch;
    let last = offset as f64 + isect.end.saturating_sub(clip.start).saturating_sub(1) as f64 / stretch;
    let extra = if (stretch - 1.0).abs() < 1e-9 { 1.0 } else { 2.0 };
    let start = first.floor().max(0.0).min(audio.frames() as f64) as usize;
    let end = (last.floor() + extra).max(0.0).min(audio.frames() as f64) as usize;
    (end > start).then_some((start as u64, end.saturating_sub(start)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{MixEngine, render_range_checked};
    use soundcraft_audio_io::{
        AudioBuffer, BitDepth, EncodeOptions,
        stream_cache::{CacheConfig, StreamCache},
    };
    use soundcraft_model::{ChannelFormat, SourceId, TrackKind};
    use std::{path::PathBuf, sync::atomic::AtomicU64};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        path: PathBuf,
        audio: AudioBuffer,
    }
    impl Fixture {
        fn new(channels: usize) -> Self {
            let path = std::env::temp_dir().join(format!("sculpt-mix-media-{}-{}.wav", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
            let audio = AudioBuffer {
                sample_rate: 48_000,
                channels: (0..channels).map(|ch| (0..80_000).map(|f| (f as f32 * 0.017 + ch as f32).sin() * 0.3).collect()).collect(),
            };
            let bytes =
                soundcraft_audio_io::encode(&audio, &EncodeOptions { bit_depth: BitDepth::Float32, dither: false, ..EncodeOptions::default() })
                    .unwrap();
            std::fs::write(&path, bytes).unwrap();
            Self { path, audio }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
    fn session(audio: Arc<SourceAudio>, two: bool) -> Session {
        let mut s = Session::default();
        let channels = audio.channels();
        s.pool.insert(SourceId(900), audio);
        for (start, offset, stretch) in if two { vec![(0, 13, 0.7), (4200, 30_000, 1.3)] } else { vec![(0, 0, 1.0)] } {
            let track = s.add_track(TrackKind::Audio, ChannelFormat::for_channels(channels), None);
            let mut clip = Clip::audio(s.new_clip_id(), "source", SourceId(900), offset, start, if two { 10_000 } else { 80_000 });
            clip.stretch = stretch;
            if two {
                clip.gain_db = -3.7;
                clip.fade_in.len = 193;
                clip.fade_out.len = 257;
            }
            s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
        }
        s
    }
    fn disk(fixture: &Fixture, cache: &Arc<StreamCache>) -> Arc<SourceAudio> {
        Arc::new(SourceAudio::from_stream(cache.open_source(&fixture.path).unwrap(), 140, |_, _| true).unwrap())
    }
    fn instrument_session(audio: Arc<SourceAudio>) -> Session {
        let mut s = Session::default();
        s.pool.insert(SourceId(900), audio);
        for _ in 0..2 {
            let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
            let mut instrument = soundcraft_model::Insert::new("sampler");
            instrument.sample = Some(SourceId(900));
            s.track_mut(track).unwrap().instrument = Some(instrument);
        }
        s
    }

    #[test]
    fn disk_sampler_audio_matches_resident_and_unprepared_disk_is_an_error() {
        let fixture = Fixture::new(2);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut disk_session = instrument_session(disk(&fixture, &cache));
        let ids: Vec<_> = disk_session.tracks.iter().map(|t| t.id).collect();
        for id in ids {
            let sequence = soundcraft_midi::Sequence {
                notes: vec![soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 }],
                ctrls: Vec::new(),
            };
            let clip = Clip::midi(disk_session.new_clip_id(), "Sampler", 0, 4096, sequence);
            disk_session.track_mut(id).unwrap().playlist_mut().unwrap().clips.push(clip);
        }
        let mut resident_session = disk_session.clone();
        resident_session.pool.insert(SourceId(900), Arc::new(SourceAudio::new(fixture.audio.clone())));
        let disk_audio = crate::render_range_checked(&disk_session, Range::new(0, 4096), 256).unwrap();
        let resident_audio = crate::render_range_checked(&resident_session, Range::new(0, 4096), 256).unwrap();
        assert_eq!(disk_audio, resident_audio);
        assert!(disk_audio.iter().flatten().any(|v| v.abs() > 0.01));
        let mut mix = crate::MixEngine::new(48_000.0, 256);
        let mut output = vec![vec![1.0; 256]; 2];
        mix.render(&disk_session, 0, 256, &mut output);
        assert_eq!(mix.take_media_error(), Some(CacheError::NotReady));
        assert!(output.iter().flatten().all(|v| *v == 0.0));
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn disk_instrument_samples_are_prepared_once_reused_and_handed_to_sampler() {
        let fixture = Fixture::new(2);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut s = instrument_session(disk(&fixture, &cache));
        let prepared = Arc::new(PreparedMedia::with_sample_budget(&s, None, 160_000).unwrap());
        assert_eq!(prepared.samples.len(), 1);
        let original = s.pool.get(SourceId(900)).unwrap();
        let resident = prepared.instrument_sample(SourceId(900), original).unwrap();
        assert_eq!(resident.buffer, fixture.audio);
        assert!(!original.is_resident());
        s.tracks[0].mixer.volume_db = -9.0;
        assert!(prepared.matches(&s));
        let next = PreparedMedia::with_previous(&s, Some(&prepared)).unwrap();
        assert!(Arc::ptr_eq(resident, next.instrument_sample(SourceId(900), original).unwrap()));
        let mut mix = crate::MixEngine::new(48_000.0, 512);
        mix.set_media(Arc::clone(&prepared));
        mix.sync(&s);
        for track in &s.tracks {
            let strip = mix.strips.get(&track.id).unwrap();
            assert!(strip.instrument.is_some());
            assert!(Arc::ptr_eq(strip.sample.as_ref().unwrap(), resident));
        }
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn instrument_sample_budget_missing_changed_and_inactive_are_explicit() {
        let fixture = Fixture::new(2);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut s = instrument_session(disk(&fixture, &cache));
        assert!(matches!(PreparedMedia::with_sample_budget(&s, None, 159_999), Err(CacheError::Capacity)));
        let prepared = PreparedMedia::new(&s).unwrap();
        s.pool.remove(SourceId(900));
        assert!(!prepared.matches(&s));
        assert!(matches!(PreparedMedia::new(&s), Err(CacheError::ReadFailed)));
        for track in &mut s.tracks {
            track.inactive = true;
        }
        assert!(PreparedMedia::new(&s).unwrap().samples.is_empty());
        s.pool.insert(SourceId(900), disk(&fixture, &cache));
        for track in &mut s.tracks {
            track.inactive = false;
        }
        assert!(!prepared.matches(&s));
        std::fs::OpenOptions::new().write(true).open(&fixture.path).unwrap().set_len(100).unwrap();
        assert!(matches!(PreparedMedia::new(&s), Err(CacheError::ReadFailed)));
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn disk_and_resident_bounces_match_exactly_with_overlaps_offsets_stretch_and_fades() {
        for channels in [1, 2, 7] {
            let fixture = Fixture::new(channels);
            let cache = StreamCache::new(CacheConfig::default()).unwrap();
            let disk = session(disk(&fixture, &cache), true);
            let resident = session(Arc::new(SourceAudio::new(fixture.audio.clone())), true);
            let expected = render_range_checked(&resident, Range::new(0, 15_000), 512).unwrap();
            let actual = render_range_checked(&disk, Range::new(0, 15_000), 512).unwrap();
            assert!(expected.iter().flatten().any(|&x| x != 0.0));
            assert_eq!(actual, expected);
            cache.shutdown(Duration::from_secs(1)).unwrap();
        }
    }
    #[test]
    fn unprepared_media_produces_explicit_error_and_no_partial_output_or_disk_read() {
        let fixture = Fixture::new(2);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut s = session(disk(&fixture, &cache), false);
        let resident_track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(fixture.audio.clone())));
        let clip = Clip::audio(s.new_clip_id(), "resident", SourceId(901), 0, 0, 80_000);
        s.track_mut(resident_track).unwrap().playlist_mut().unwrap().clips.push(clip);
        let mut mix = MixEngine::new(48_000.0, 512);
        let mut out = vec![vec![1.0; 512]; 2];
        let before = cache.stats().read_bytes;
        mix.render(&s, 0, 512, &mut out);
        assert!(out.iter().flatten().all(|&x| x == 0.0));
        assert_eq!(mix.take_media_error(), Some(CacheError::NotReady));
        assert_eq!(cache.stats().read_bytes, before);
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn prepared_loop_destination_and_approaching_clip_are_ready_without_waiting_in_render() {
        let fixture = Fixture::new(2);
        let cache = StreamCache::new(CacheConfig { page_count: 8, lookahead_pages: 0, ..CacheConfig::default() }).unwrap();
        let s = session(disk(&fixture, &cache), false);
        let resident = session(Arc::new(SourceAudio::new(fixture.audio.clone())), false);
        let media = Arc::new(PreparedMedia::new(&s).unwrap());
        media.prepare(0, 512, Some(70_000), Duration::from_secs(2)).unwrap();
        let mut mix = MixEngine::new(48_000.0, 512);
        mix.set_media(Arc::clone(&media));
        mix.set_media_anchor(Some(70_000));
        let mut reference = MixEngine::new(48_000.0, 512);
        let mut out = vec![vec![0.0; 512]; 2];
        let mut expected = out.clone();
        for pos in [0, 70_000] {
            mix.render(&s, pos, 512, &mut out);
            reference.render(&resident, pos, 512, &mut expected);
            assert_eq!(out, expected);
            assert_eq!(mix.take_media_error(), None);
        }
        media.deactivate();
        let mut future = s.clone();
        future.tracks[0].playlist_mut().unwrap().clips[0].start = 16_000;
        let future_media = Arc::new(PreparedMedia::new(&future).unwrap());
        future_media.prepare(0, 512, None, Duration::from_secs(2)).unwrap();
        mix.set_media(future_media);
        mix.set_media_anchor(None);
        mix.render(&future, 16_000, 512, &mut out);
        assert!(out.iter().flatten().any(|&x| x != 0.0));
        assert_eq!(mix.take_media_error(), None);
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn offline_bounce_does_not_read_unrelated_future_media() {
        let fixture = Fixture::new(2);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut s = session(Arc::new(SourceAudio::new(fixture.audio.clone())), false);
        let future = disk(&fixture, &cache);
        s.pool.insert(SourceId(901), future);
        let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let clip = Clip::audio(s.new_clip_id(), "future", SourceId(901), 0, 1000, 10_000);
        s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
        std::fs::OpenOptions::new().write(true).open(&fixture.path).unwrap().set_len(100).unwrap();
        let out = render_range_checked(&s, Range::new(0, 512), 512).unwrap();
        assert!(out.iter().flatten().any(|&x| x != 0.0));
        assert_eq!(cache.stats().read_bytes, 0);
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn offline_consumers_match_resident_audio_with_unaligned_fades_and_clip_fx() {
        use crate::{render_bus_checked, render_clips_checked, render_track_pre_fader_checked};
        use soundcraft_model::{Insert, Route};
        for channels in [1, 2, 7] {
            let fixture = Fixture::new(channels);
            let cache = StreamCache::new(CacheConfig::default()).unwrap();
            let mut resident = session(Arc::new(SourceAudio::new(fixture.audio.clone())), true);
            for t in &mut resident.tracks {
                let clip = &mut t.playlist_mut().unwrap().clips[0];
                clip.gain_env = vec![(0, -2.0), (1137, -11.0), (5749, 0.0)];
            }
            let clip_id = resident.tracks[1].clips()[0].id.0;
            resident.edit.values.insert(format!("clip_fx.{clip_id}.gain"), -4.0);
            resident.edit.values.insert(format!("clip_fx.{clip_id}.comp.threshold"), -20.0);
            let mut streamed = resident.clone();
            streamed.pool.insert(SourceId(900), disk(&fixture, &cache));
            let range = Range::new(17, 15_000);
            for track in &resident.tracks {
                let expected = render_clips_checked(&resident, track.id, range).unwrap();
                let actual = render_clips_checked(&streamed, track.id, range).unwrap();
                assert_eq!(actual, expected, "consolidate {channels}ch");
                assert_eq!(
                    render_track_pre_fader_checked(&streamed, track.id, range).unwrap(),
                    render_track_pre_fader_checked(&resident, track.id, range).unwrap()
                );
            }
            let bus = resident.add_bus("Stem", ChannelFormat::for_channels(channels));
            for track in &mut resident.tracks {
                track.mixer.output = Route::Bus(bus);
                track.mixer.inserts[0] = Some(Insert::new("gain"));
            }
            let mut streamed = resident.clone();
            streamed.pool.insert(SourceId(900), disk(&fixture, &cache));
            assert_eq!(render_bus_checked(&streamed, bus, range, 512).unwrap(), render_bus_checked(&resident, bus, range, 512).unwrap());
            assert!(!streamed.pool.get(SourceId(900)).unwrap().is_resident());
            cache.shutdown(Duration::from_secs(1)).unwrap();
        }
    }
    #[test]
    fn checked_offline_consumers_reject_incoherent_pcm_instead_of_baking_silence_or_wrong_rate() {
        use crate::{render_bus_checked, render_clips_checked, render_track_pre_fader_checked};
        for audio in [
            AudioBuffer { sample_rate: 44_100, channels: vec![vec![0.2; 1024]; 2] },
            AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.2; 1024], vec![0.2; 512]] },
            AudioBuffer { sample_rate: 48_000, channels: vec![Vec::new(); 2] },
        ] {
            let mut s = session(Arc::new(SourceAudio::new(audio)), false);
            let track = s.tracks[0].id;
            let bus = s.add_bus("Bus", ChannelFormat::Stereo);
            s.tracks[0].mixer.output = soundcraft_model::Route::Bus(bus);
            let range = Range::new(0, 512);
            assert!(render_range_checked(&s, range, 512).is_err());
            assert!(render_clips_checked(&s, track, range).is_err());
            assert!(render_track_pre_fader_checked(&s, track, range).is_err());
            assert!(render_bus_checked(&s, bus, range, 512).is_err());
        }
    }
    #[test]
    fn checked_offline_consumers_reject_media_errors_and_oversize_without_partial_audio() {
        use crate::{MAX_RENDER_SAMPLES, render_bus_checked, render_clips_checked, render_track_pre_fader_checked};
        let fixture = Fixture::new(2);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut s = session(disk(&fixture, &cache), false);
        let track = s.tracks[0].id;
        let bus = s.add_bus("Stem", ChannelFormat::Stereo);
        s.tracks[0].mixer.output = soundcraft_model::Route::Bus(bus);
        let excessive = Range::new(0, (MAX_RENDER_SAMPLES / 2 + 1) as i64);
        assert!(render_clips_checked(&s, track, excessive).is_err());
        assert!(render_track_pre_fader_checked(&s, track, excessive).is_err());
        assert!(render_bus_checked(&s, bus, excessive, 512).is_err());
        assert!(render_range_checked(&s, excessive, 512).is_err());
        assert_eq!(cache.stats().read_bytes, 0);
        std::fs::OpenOptions::new().write(true).open(&fixture.path).unwrap().set_len(100).unwrap();
        let range = Range::new(0, 512);
        assert!(render_clips_checked(&s, track, range).is_err());
        assert!(render_track_pre_fader_checked(&s, track, range).is_err());
        assert!(render_bus_checked(&s, bus, range, 512).is_err());
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn changed_media_is_rejected_during_offline_preparation() {
        let fixture = Fixture::new(2);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let s = session(disk(&fixture, &cache), false);
        std::fs::OpenOptions::new().write(true).open(&fixture.path).unwrap().set_len(100).unwrap();
        assert!(render_range_checked(&s, Range::new(0, 1024), 512).is_err());
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
}
