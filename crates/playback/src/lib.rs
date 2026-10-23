//! Realtime playback through the system audio device (cpal: CoreAudio, WASAPI, ALSA/Pulse/
//! PipeWire via ALSA, JACK, WebAudio).
//!
//! The UI thread owns a [`Player`]; the audio callback owns the [`MixEngine`] and a snapshot of
//! the session (`Arc<Session>`) that the UI swaps whenever the document changes. Commands go to
//! the callback through a channel; position and meters come back through atomics and a
//! `try_lock`ed snapshot, so the callback never blocks. When no device is available the player
//! falls back to a silent clock thread so the transport, meters and automation still run.
//!
//! Surround: the device is opened with as many output channels as it offers, up to the width of
//! the session's main format. Main channels (SMPTE/WAV order) go to device channels in order; when
//! the device has fewer channels than the main mix, the mix is folded down to stereo (ITU-R
//! BS.775) on the first two device channels.
//!
//! Plugins and threads: hosted third-party plugins (CLAP/VST3) are created, given their stored
//! state and prepared on the thread that calls [`Player::update_session`] (the UI thread), and
//! travel to the audio thread together with the session snapshot that needs them; the mix
//! engine adopts them (see `soundcraft_mix`'s crate docs). Instances the engine retires come back
//! on a return channel and are dropped on the UI thread ([`Player::idle`]). Plugin states for
//! saving are read on request ([`Player::capture_states`]: a command with a reply; the audio
//! thread answers between two blocks, never blocking). Plugin editors are driven on the UI thread
//! through handles taken from each instance before it leaves ([`Player::open_editor`]).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod record;
pub mod sculpt_audio_settings; // SCULPT_CORE22_AUDIO_MODULE

#[cfg(not(target_arch = "wasm32"))]
use soundcraft_audio_io::stream_cache::CacheError;
use soundcraft_dsp::{Plugin, PluginEditor};
#[cfg(not(target_arch = "wasm32"))]
use soundcraft_mix::media::PreparedMedia;
use soundcraft_mix::{MAX_CHANNELS, MixEngine, PreparedInstance, StripMeter, itu_stereo_fold, main_channels};
use soundcraft_model::{Session, TrackId};
use soundcraft_time::{Range, Samples};
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};

const BLOCK: usize = 512;

/// The realtime audio thread marker lives in the mix engine, which carries it into the strips it
/// renders on worker threads; the cpal callbacks here set it.
pub use soundcraft_mix::{mark_audio_thread, on_audio_thread};

/// Meter values for the UI.
#[derive(Debug, Clone, Default)]
pub struct MeterSnapshot {
    pub tracks: HashMap<TrackId, StripMeter>,
    pub main: StripMeter,
}

/// Plugin states read from the live instances: `(track, slot or INSTRUMENT_SLOT, state)`.
pub type PluginStates = Vec<(TrackId, usize, Vec<u8>)>;

/// A parameter edit made in a plugin's own editor: `(track, slot, param id, value)`.
pub type EditorEdit = (TrackId, usize, String, f32);

enum Cmd {
    /// A new snapshot plus the third-party instances it needs (created on the UI thread).
    #[cfg(any(test, target_arch = "wasm32"))]
    Session(Arc<Session>, Vec<PreparedInstance>),
    #[cfg(not(target_arch = "wasm32"))]
    PreparedSession(Arc<Session>, Vec<PreparedInstance>, Arc<PreparedMedia>),
    #[cfg(not(target_arch = "wasm32"))]
    PreparedPlay {
        from: Samples,
        end: Option<Samples>,
        looped: Option<Range>,
        media: Arc<PreparedMedia>,
    },
    #[cfg(not(target_arch = "wasm32"))]
    PreparedLocate(Samples, Arc<PreparedMedia>),
    /// Read every plugin state and reply.
    CaptureStates(Sender<PluginStates>),
    /// Return every hosted plugin to the UI thread, then acknowledge cleanup.
    Shutdown(Sender<()>),
    Input(Arc<record::InputRing>),
    Recording(bool),
    #[cfg(any(test, target_arch = "wasm32"))]
    Play {
        from: Samples,
        end: Option<Samples>,
        looped: Option<Range>,
    },
    Stop,
    #[cfg(target_arch = "wasm32")]
    Locate(Samples),
}

struct Shared {
    position: AtomicI64,
    playing: AtomicBool,
    /// Speed × 1000.
    speed: AtomicU32,
    meters: Mutex<MeterSnapshot>,
    callback_frames: AtomicU32,
    stream_failed: AtomicBool,
    #[cfg(not(target_arch = "wasm32"))]
    media_error: AtomicU8,
}

/// State owned by the audio callback.
struct AudioState {
    rx: Receiver<Cmd>,
    shared: Arc<Shared>,
    session: Arc<Session>,
    mix: MixEngine,
    pos: Samples,
    playing: bool,
    end: Option<Samples>,
    looped: Option<Range>,
    /// Rendered frames waiting to be consumed (planar, main channels) and the fractional read index.
    pending: Vec<Vec<f32>>,
    /// Stereo fold-down gains per main channel (used when the device has fewer channels).
    fold: [(f32, f32); MAX_CHANNELS],
    pending_len: usize,
    read: f64,
    device_rate: f64,
    device_output_channels: usize, // SCULPT_CORE21_PLAY_DEVICE_FIELD
    input: Option<Arc<record::InputRing>>,
    recording: bool,
    /// Retired plugin instances go back to the UI thread to be dropped there.
    garbage: Sender<Vec<Box<dyn Plugin>>>,
}

fn new_engine(sr: f32) -> MixEngine {
    let mut m = MixEngine::new(sr, BLOCK);
    m.set_external_instances(true);
    m.metronome = true;
    m
}

impl AudioState {
    fn new(rx: Receiver<Cmd>, shared: Arc<Shared>, session: Arc<Session>, device_rate: f64, garbage: Sender<Vec<Box<dyn Plugin>>>) -> Self {
        let sr = session.sample_rate.as_f64() as f32;
        let nch = main_channels(&session);
        AudioState {
            garbage,
            fold: fold_gains(&session),
            rx,
            shared,
            mix: new_engine(sr),
            session,
            pos: 0,
            playing: false,
            end: None,
            looped: None,
            pending: vec![vec![0.0; BLOCK]; nch],
            pending_len: 0,
            read: 0.0,
            device_rate,
            device_output_channels: nch, // SCULPT_CORE21_PLAY_DEVICE_DEFAULT
            input: None,
            recording: false,
        }
    }

    fn adopt_session(&mut self, s: Arc<Session>, instances: Vec<PreparedInstance>) {
        if (s.sample_rate.as_f64() - self.session.sample_rate.as_f64()).abs() > 0.5 {
            let mut old = std::mem::replace(&mut self.mix, new_engine(s.sample_rate.as_f64() as f32));
            let _ = self.garbage.send(old.drain_plugins());
        }
        self.mix.adopt(instances);
        // The main format changed (a document edit, not steady state): resize.
        // SCULPT_CORE21_PLAY_RESIZE: preserve physical channels on
        // a session swap (especially when Show opens a multistem cue).
        let nch = main_channels(&s).max(self.device_output_channels);
        if self.pending.len() != nch {
            self.pending = vec![vec![0.0; BLOCK]; nch];
            self.pending_len = 0;
            self.read = 0.0;
        }
        self.fold = fold_gains(&s);
        self.session = s;
    }

    fn begin_play(&mut self, from: Samples, end: Option<Samples>, looped: Option<Range>) {
        self.pos = from;
        self.end = end;
        self.looped = looped.filter(|r| r.len() > 64);
        self.playing = true;
        self.pending_len = 0;
        self.read = 0.0;
        self.mix.reset();
    }

    fn drain(&mut self) {
        let mut changed = false;
        while let Ok(c) = self.rx.try_recv() {
            match c {
                Cmd::Input(r) => self.input = Some(r),
                Cmd::Recording(r) => self.recording = r,
                Cmd::Shutdown(reply) => {
                    self.playing = false;
                    self.recording = false;
                    self.input = None;
                    self.end = None;
                    self.looped = None;
                    self.pending_len = 0;
                    let rate = self.session.sample_rate.as_f64() as f32;
                    let mut old = std::mem::replace(&mut self.mix, new_engine(rate));
                    let plugins = old.drain_plugins();
                    if !plugins.is_empty() {
                        let _ = self.garbage.send(plugins);
                    }
                    let _ = reply.send(());
                }
                Cmd::CaptureStates(reply) => {
                    self.mix.ensure_synced(&self.session);
                    let _ = reply.send(self.mix.capture_states());
                }
                #[cfg(any(test, target_arch = "wasm32"))]
                Cmd::Session(s, instances) => {
                    changed = true;
                    self.adopt_session(s, instances);
                }
                #[cfg(not(target_arch = "wasm32"))]
                Cmd::PreparedSession(s, instances, media) => {
                    changed = true;
                    self.adopt_session(s, instances);
                    self.mix.set_media(media);
                    self.mix.set_media_anchor(self.looped.map(|r| r.start));
                }
                #[cfg(not(target_arch = "wasm32"))]
                Cmd::PreparedPlay { from, end, looped, media } => {
                    self.mix.set_media(media);
                    self.mix.set_media_anchor(looped.map(|r| r.start));
                    self.begin_play(from, end, looped);
                }
                #[cfg(not(target_arch = "wasm32"))]
                Cmd::PreparedLocate(at, media) => {
                    self.mix.set_media(media);
                    self.pos = at;
                    self.pending_len = 0;
                    self.read = 0.0;
                }
                #[cfg(any(test, target_arch = "wasm32"))]
                Cmd::Play { from, end, looped } => {
                    self.begin_play(from, end, looped);
                }
                Cmd::Stop => {
                    self.pending_len = 0;
                    self.read = 0.0;
                    self.playing = false;
                    self.end = None;
                    self.looped = None;
                    self.mix.reset();
                }
                #[cfg(target_arch = "wasm32")]
                Cmd::Locate(at) => {
                    self.pos = at;
                    self.pending_len = 0;
                    self.read = 0.0;
                }
            }
        }
        if changed {
            // Place adopted instances now (not only once playback starts), so state captures and
            // editors see them; `sync` creates no third-party plugins here.
            self.mix.ensure_synced(&self.session);
        }
        self.send_garbage();
        self.shared.playing.store(self.playing, Ordering::Relaxed);
    }

    fn send_garbage(&mut self) {
        let g = self.mix.take_retired();
        if !g.is_empty() {
            let _ = self.garbage.send(g);
        }
    }

    /// Render the next block at the session rate into `pending`.
    fn render_next(&mut self, quantum: usize) {
        let mut n = quantum.clamp(1, BLOCK);
        if let Some(l) = self.looped
            && self.pos >= l.end
        {
            self.pos = l.start;
        }
        if let Some(l) = self.looped {
            n = n.min(usize::try_from((l.end - self.pos).max(1)).unwrap_or(BLOCK));
        } else if let Some(e) = self.end {
            if self.pos >= e {
                self.playing = false;
                self.end = None;
                self.shared.playing.store(false, Ordering::Relaxed);
                n = 0;
            } else {
                n = n.min(usize::try_from(e - self.pos).unwrap_or(BLOCK));
            }
        }
        if n == 0 {
            for c in &mut self.pending {
                c.iter_mut().for_each(|x| *x = 0.0);
            }
            self.pending_len = BLOCK;
            return;
        }
        if let Some(ring) = &self.input {
            let ch = ring.channels.load(Ordering::Relaxed).max(1);
            if self.mix.input.len() != ch {
                self.mix.input = vec![vec![0.0; BLOCK]; ch];
            }
            ring.pop_into(&mut self.mix.input, n, self.session.sample_rate.hz());
        }
        self.mix.monitor_only = !self.playing;
        self.mix.recording = self.recording;
        self.mix.set_device_output_channels(self.device_output_channels); // SCULPT_CORE21_PLAY_ROUTE_DEVICE
        self.mix.render(&self.session, self.pos, n, &mut self.pending);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(error) = self.mix.take_media_error() {
            self.shared.media_error.store(media_error_code(error), Ordering::Release);
            self.playing = false;
            self.end = None;
            self.looped = None;
            self.shared.playing.store(false, Ordering::Release);
        }
        self.send_garbage();
        self.pending_len = n;
        if let Ok(mut m) = self.shared.meters.try_lock() {
            m.main.copy_from(&self.mix.main_meter);
            for (k, v) in &self.mix.meters {
                match m.tracks.get_mut(k) {
                    Some(d) => d.copy_from(v),
                    None => {
                        m.tracks.insert(*k, v.clone());
                    }
                }
            }
        }
        if self.playing {
            self.pos += n as i64;
            self.shared.position.store(self.pos, Ordering::Relaxed);
        }
    }

    /// Fill an interleaved device buffer.
    fn fill(&mut self, out: &mut [f32], channels: usize) {
        self.drain();
        let channels = channels.max(1);
        let monitoring = self.input.is_some() && self.session.needs_hardware_input();
        if !self.playing && !monitoring {
            out.iter_mut().for_each(|x| *x = 0.0);
            if let Ok(mut m) = self.shared.meters.try_lock() {
                m.main = StripMeter::default();
                m.tracks.clear();
            }
            return;
        }
        let speed = f64::from(self.shared.speed.load(Ordering::Relaxed)) / 1000.0;
        let step = self.session.sample_rate.as_f64() / self.device_rate.max(1.0) * speed.clamp(0.1, 4.0);
        let callback_frames = out.len() / channels;
        let quantum = ((callback_frames as f64 * step).ceil() as usize).clamp(1, BLOCK);
        for frame in out.chunks_mut(channels) {
            while self.read >= self.pending_len as f64 {
                self.read -= self.pending_len as f64;
                if !self.playing && !monitoring {
                    break;
                }
                self.render_next(quantum);
                if self.pending_len == 0 {
                    break;
                }
            }
            let i0 = self.read.floor() as usize;
            let fr = (self.read - i0 as f64) as f32;
            let sounding = self.playing || monitoring;
            let nch = self.pending.len();
            let sample = |c: usize| {
                let ch = self.pending.get(c);
                let a = ch.and_then(|v| v.get(i0)).copied().unwrap_or(0.0);
                let b = ch.and_then(|v| v.get(i0 + 1)).copied().unwrap_or(a);
                a + (b - a) * fr
            };
            if nch > 2 && channels < nch {
                // Fewer device channels than the main mix: ITU fold-down to stereo.
                let (mut l, mut r) = (0.0f32, 0.0f32);
                if sounding {
                    for (c, (gl, gr)) in self.fold.iter().enumerate().take(nch) {
                        let v = sample(c);
                        l += v * gl;
                        r += v * gr;
                    }
                }
                for (c, o) in frame.iter_mut().enumerate() {
                    *o = match c {
                        0 => l.clamp(-1.0, 1.0),
                        1 => r.clamp(-1.0, 1.0),
                        _ => 0.0,
                    };
                }
            } else {
                for (c, o) in frame.iter_mut().enumerate() {
                    // Stereo mixes on a mono device play the left channel (as before).
                    *o = if c < nch.max(1) && sounding { sample(c).clamp(-1.0, 1.0) } else { 0.0 };
                }
            }
            self.read += step;
        }
    }
}

/// UI-thread bookkeeping of the third-party instances the audio thread holds.
struct Side {
    /// What was last created for each slot: (plugin id, channels, created ok).
    shadow: HashMap<(TrackId, usize), (String, usize, bool)>,
    /// Session sample rate the instances were prepared for.
    rate: f32,
    /// Editor handles of the live instances.
    editors: HashMap<(TrackId, usize), Box<dyn PluginEditor>>,
    garbage: Receiver<Vec<Box<dyn Plugin>>>,
    #[cfg(not(target_arch = "wasm32"))]
    media: Arc<PreparedMedia>,
    #[cfg(not(target_arch = "wasm32"))]
    session: Arc<Session>,
    #[cfg(not(target_arch = "wasm32"))]
    retired_media: Vec<Arc<PreparedMedia>>,
    #[cfg(not(target_arch = "wasm32"))]
    retired_sessions: Vec<Arc<Session>>,
    #[cfg(not(target_arch = "wasm32"))]
    media_problem: Option<CacheError>,
    #[cfg(not(target_arch = "wasm32"))]
    rejected_session: bool,
    #[cfg(not(target_arch = "wasm32"))]
    loop_anchor: Option<Samples>,
}

impl Side {
    fn new(session: Arc<Session>, garbage: Receiver<Vec<Box<dyn Plugin>>>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let (media, media_problem) = match PreparedMedia::new(&session) {
            Ok(media) => (Arc::new(media), None),
            Err(error) => (Arc::new(PreparedMedia::default()), Some(error)),
        };
        Self {
            shadow: HashMap::new(),
            rate: session.sample_rate.as_f64() as f32,
            editors: HashMap::new(),
            garbage,
            #[cfg(not(target_arch = "wasm32"))]
            media,
            #[cfg(not(target_arch = "wasm32"))]
            session,
            #[cfg(not(target_arch = "wasm32"))]
            media_problem,
            #[cfg(not(target_arch = "wasm32"))]
            rejected_session: false,
            #[cfg(not(target_arch = "wasm32"))]
            retired_media: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            retired_sessions: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            loop_anchor: None,
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn replace_media(&mut self, media: Arc<PreparedMedia>) {
        if Arc::ptr_eq(&self.media, &media) {
            return;
        }
        let old = std::mem::replace(&mut self.media, media);
        self.retired_media.push(old);
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn prepare_media(&mut self, at: Samples, anchor: Option<Samples>) -> Result<Arc<PreparedMedia>, CacheError> {
        if self.rejected_session {
            return Err(self.media_problem.unwrap_or(CacheError::ReadFailed));
        }
        if self.media_problem.is_some() {
            match PreparedMedia::with_previous(&self.session, Some(&self.media)) {
                Ok(media) => {
                    self.replace_media(Arc::new(media));
                    self.media_problem = None;
                }
                Err(error) => {
                    self.media_problem = Some(error);
                    return Err(error);
                }
            }
        }
        match self.media.prepare(at, BLOCK, anchor, std::time::Duration::from_secs(3)) {
            Ok(()) => Ok(Arc::clone(&self.media)),
            Err(error) => {
                self.media_problem = Some(error);
                Err(error)
            }
        }
    }

    /// Drops retired instances (closing their editors first, inside the plugin's drop).
    fn collect_garbage(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.retired_media.retain(|media| Arc::strong_count(media) > 1);
            self.retired_sessions.retain(|session| Arc::strong_count(session) > 1);
        }
        while let Ok(g) = self.garbage.try_recv() {
            drop(g);
        }
    }

    fn forget(&mut self, key: (TrackId, usize)) {
        self.shadow.remove(&key);
        if let Some(mut e) = self.editors.remove(&key) {
            e.close();
        }
    }
}

/// The transport and audio device.
pub struct Player {
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
    side: Mutex<Side>,
    #[cfg(not(target_arch = "wasm32"))]
    _stream: Option<cpal::Stream>,
    #[cfg(target_arch = "wasm32")]
    _stream: Option<cpal::Stream>,
    pub device_name: String,
    pub device_rate: u32,
    /// Output channels the device was opened with (0 for the silent clock).
    pub device_channels: u16,
    /// Requested hardware buffer; None means device default (not a measured latency).
    pub device_buffer: Option<u32>,
    pub open_error: Option<String>,
    /// True when no audio device could be opened (silent clock).
    pub silent: bool,
}

impl Player {
    /// Open the default output device. Never fails: without a device it runs a silent clock.
    pub fn new(session: Arc<Session>) -> Player {
        Self::with_audio_settings(session, &sculpt_audio_settings::AudioSettings::read())
    }

    /// Trial configuration does not change the shared profile.
    pub fn with_audio_settings(session: Arc<Session>, settings: &sculpt_audio_settings::AudioSettings) -> Player {
        let mut p = Player::open(Arc::clone(&session), settings);
        // Hand the initial session's third-party plugins over.
        if let Err(error) = p.update_session(session) {
            p.open_error = Some(error);
        }
        p
    }

    /// Most recent hardware callback size; not a round-trip latency measurement.
    pub fn callback_frames(&self) -> u32 {
        self.shared.callback_frames.load(Ordering::Relaxed)
    }

    pub fn stream_failed(&self) -> bool {
        self.shared.stream_failed.load(Ordering::Relaxed)
    }

    fn side(&self) -> std::sync::MutexGuard<'_, Side> {
        self.side.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn open(session: Arc<Session>, settings: &sculpt_audio_settings::AudioSettings) -> Player {
        let (gtx, grx) = channel();
        let side = Mutex::new(Side::new(Arc::clone(&session), grx));
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
            callback_frames: AtomicU32::new(0),
            stream_failed: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            media_error: AtomicU8::new(0),
        });
        let (tx, rx) = channel();
        let want = session.sample_rate.hz();
        // SCULPT_CORE21_PLAY_REQUEST64: take hardware endpoints into account
        // before opening CoreAudio rather than silently truncating to Main 1-2.
        let want_ch = u16::try_from(sculpt_required_output_channels(&session)).unwrap_or(2);
        match open_device(want, want_ch, settings) {
            Ok((device, config, name)) => {
                let rate = config.sample_rate.0;
                let channels = usize::from(config.channels);
                let mut state = AudioState::new(rx, Arc::clone(&shared), session, f64::from(rate), gtx);
                #[cfg(not(target_arch = "wasm32"))]
                state.mix.set_media(Arc::clone(&side.lock().unwrap_or_else(PoisonError::into_inner).media));
                state.device_output_channels = channels;
                let render_channels = channels.max(main_channels(&state.session));
                state.pending.resize_with(render_channels, || vec![0.0; BLOCK]); // SCULPT_CORE21_PLAY_64_BUFFER
                // cpal may call this on the audio thread (on ALSA in a loop while the device
                // stays broken): marked, so the logger hands the record off instead of writing it.
                let errors = Arc::clone(&shared);
                let err_fn = move |e: cpal::StreamError| {
                    errors.stream_failed.store(true, Ordering::Relaxed);
                    mark_audio_thread();
                    log::warn!("audio stream error: {e}");
                };
                use cpal::traits::{DeviceTrait, StreamTrait};
                match device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        mark_audio_thread();
                        state.shared.callback_frames.store(u32::try_from(data.len() / channels.max(1)).unwrap_or(u32::MAX), Ordering::Relaxed);
                        state.fill(data, channels);
                    },
                    err_fn,
                    None,
                ) {
                    Ok(stream) => {
                        if let Err(e) = stream.play() {
                            let (tx2, rx2) = channel();
                            let (gtx2, grx2) = channel();
                            let mut side = side;
                            side.get_mut().unwrap_or_else(PoisonError::into_inner).garbage = grx2;
                            let mut failed = Player::silent_clock(shared, tx2, rx2, want, side, gtx2);
                            failed.open_error = Some(format!("Audio stream failed to start: {e}"));
                            return failed;
                        }
                        log::info!("audio device: {name} @ {rate} Hz, {channels} ch");
                        Player {
                            shared,
                            tx,
                            side,
                            _stream: Some(stream),
                            device_name: name,
                            device_rate: rate,
                            device_channels: config.channels,
                            device_buffer: match config.buffer_size {
                                cpal::BufferSize::Fixed(n) => Some(n),
                                _ => None,
                            },
                            open_error: None,
                            silent: false,
                        }
                    }
                    Err(e) => {
                        log::warn!("cannot open audio stream: {e}; using a silent clock");
                        let (tx2, rx2) = channel();
                        let (gtx2, grx2) = channel();
                        // The state (and its receivers) moved into the failed closure; rebuild.
                        let mut side = side;
                        side.get_mut().unwrap_or_else(PoisonError::into_inner).garbage = grx2;
                        let mut failed = Player::silent_clock(shared, tx2, rx2, want, side, gtx2);
                        failed.open_error = Some(e.to_string());
                        failed
                    }
                }
            }
            Err(e) => {
                log::warn!("no audio output: {e}; using a silent clock");
                let s2 = Arc::clone(&shared);
                let mut failed = Player::silent_clock_with(s2, tx, rx, session, side, gtx);
                failed.open_error = Some(e);
                failed
            }
        }
    }

    fn silent_clock(
        shared: Arc<Shared>,
        tx: Sender<Cmd>,
        rx: Receiver<Cmd>,
        rate: u32,
        side: Mutex<Side>,
        gtx: Sender<Vec<Box<dyn Plugin>>>,
    ) -> Player {
        #[cfg(not(target_arch = "wasm32"))]
        let s = Arc::clone(&side.lock().unwrap_or_else(PoisonError::into_inner).session);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = rate;
        #[cfg(target_arch = "wasm32")]
        let s = Arc::new(Session::new("Untitled", soundcraft_time::SampleRate::new(rate).unwrap_or_default()));
        Player::silent_clock_with(shared, tx, rx, s, side, gtx)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn silent_clock_with(
        shared: Arc<Shared>,
        tx: Sender<Cmd>,
        rx: Receiver<Cmd>,
        session: Arc<Session>,
        side: Mutex<Side>,
        gtx: Sender<Vec<Box<dyn Plugin>>>,
    ) -> Player {
        let rate = session.sample_rate.hz();
        let mut state = AudioState::new(rx, Arc::clone(&shared), session, f64::from(rate), gtx);
        #[cfg(not(target_arch = "wasm32"))]
        state.mix.set_media(Arc::clone(&side.lock().unwrap_or_else(PoisonError::into_inner).media));
        let spawn = std::thread::Builder::new().name("soundcraft-clock".into()).spawn(move || {
            let mut buf = vec![0.0f32; 4096];
            let mut last = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(10));
                let now = std::time::Instant::now();
                let frames = ((now - last).as_secs_f64() * state.device_rate) as usize;
                last = now;
                let n = (frames * 2).min(buf.len());
                if let Some(b) = buf.get_mut(..n) {
                    state.fill(b, 2);
                }
                if Arc::strong_count(&state.shared) <= 1 {
                    break;
                }
            }
        });
        if let Err(e) = spawn {
            log::warn!("clock thread failed: {e}");
        }
        Player {
            shared,
            tx,
            side,
            _stream: None,
            device_name: "No audio device (silent)".into(),
            device_rate: rate,
            device_channels: 0,
            device_buffer: None,
            open_error: None,
            silent: true,
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn silent_clock_with(
        shared: Arc<Shared>,
        tx: Sender<Cmd>,
        _rx: Receiver<Cmd>,
        session: Arc<Session>,
        side: Mutex<Side>,
        _gtx: Sender<Vec<Box<dyn Plugin>>>,
    ) -> Player {
        Player {
            shared,
            tx,
            side,
            _stream: None,
            device_name: "No audio device".into(),
            device_rate: session.sample_rate.hz(),
            device_channels: 0,
            device_buffer: None,
            open_error: None,
            silent: true,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn fail_media(&self, error: CacheError) {
        self.shared.media_error.store(media_error_code(error), Ordering::Release);
        self.shared.playing.store(false, Ordering::Release);
        let _ = self.tx.send(Cmd::Stop);
    }
    /// Control-thread error acknowledgement; retry rebuilds disk views before starting again.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn take_media_error(&self) -> Option<CacheError> {
        let error = media_error_from_code(self.shared.media_error.swap(0, Ordering::AcqRel));
        if let Some(error) = error {
            self.side().media_problem = Some(error);
        }
        error
    }

    /// Sends a new session snapshot to the audio thread, with freshly created (state-restored,
    /// prepared) instances of any third-party plugin it newly needs. Call on the UI thread.
    pub fn update_session(&self, s: Arc<Session>) -> Result<(), String> {
        let mut side = self.side();
        #[cfg(not(target_arch = "wasm32"))]
        {
            side.rejected_session = true;
            let media = if side.media_problem.is_none() && side.media.matches(&s) {
                Arc::clone(&side.media)
            } else {
                match PreparedMedia::with_previous(&s, Some(&side.media)) {
                    Ok(media) => Arc::new(media),
                    Err(error) => {
                        side.media_problem = Some(error);
                        self.fail_media(error);
                        return Err(error.to_string());
                    }
                }
            };
            let at = self.position();
            if self.is_playing()
                && let Err(error) = media.prepare(at, BLOCK, side.loop_anchor, std::time::Duration::from_secs(3))
            {
                side.media_problem = Some(error);
                self.fail_media(error);
                return Err(error.to_string());
            }
            // Do not mark plugin instances as live until media preparation has succeeded.
            let instances = side.prepare(&s);
            side.replace_media(Arc::clone(&media));
            if !Arc::ptr_eq(&side.session, &s) {
                let old = std::mem::replace(&mut side.session, Arc::clone(&s));
                side.retired_sessions.push(old);
            }
            side.media_problem = None;
            self.tx.send(Cmd::PreparedSession(s, instances, media)).map_err(|_| "audio command receiver is stopped".to_string())?;
            side.rejected_session = false;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let instances = side.prepare(&s);
            self.tx.send(Cmd::Session(s, instances)).map_err(|_| "audio command receiver is stopped".to_string())?;
        }
        Ok(())
    }

    /// Reads the state of every live third-party plugin (for saving the session). Waits briefly
    /// for the audio thread to answer between two blocks; empty when it does not answer.
    pub fn capture_states(&self) -> PluginStates {
        self.side().collect_garbage();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = channel();
            if self.tx.send(Cmd::CaptureStates(tx)).is_err() {
                return Vec::new();
            }
            rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap_or_default()
        }
        #[cfg(target_arch = "wasm32")]
        {
            Vec::new()
        }
    }

    /// Per-UI-frame housekeeping: drops retired instances and services plugin editors. Returns
    /// the parameter edits made in open editors since the last call.
    pub fn idle(&self) -> Vec<EditorEdit> {
        let mut side = self.side();
        side.collect_garbage();
        let mut out = Vec::new();
        for (&(t, slot), e) in side.editors.iter_mut() {
            for (id, v) in e.idle() {
                out.push((t, slot, id, v));
            }
        }
        out
    }

    /// True when the plugin in this slot has its own editor.
    pub fn has_editor(&self, track: TrackId, slot: usize) -> bool {
        self.side().editors.contains_key(&(track, slot))
    }

    /// Opens the plugin's own editor window. Main thread only.
    pub fn open_editor(&self, track: TrackId, slot: usize) -> Result<(), String> {
        match self.side().editors.get_mut(&(track, slot)) {
            Some(e) => e.open(),
            None => Err("this plugin has no editor (or it is not loaded)".into()),
        }
    }

    pub fn close_editor(&self, track: TrackId, slot: usize) {
        if let Some(e) = self.side().editors.get_mut(&(track, slot)) {
            e.close();
        }
    }

    pub fn editor_open(&self, track: TrackId, slot: usize) -> bool {
        self.side().editors.get(&(track, slot)).is_some_and(|e| e.is_open())
    }

    /// Close native plugin windows on the UI thread before replacing this player.
    pub fn close_editors(&self) {
        for editor in self.side().editors.values_mut() {
            editor.close();
        }
    }

    pub fn any_editor_open(&self) -> bool {
        self.side().editors.values().any(|e| e.is_open())
    }

    /// Shows a parameter change made elsewhere in the plugin's open editor.
    pub fn editor_set_param(&self, track: TrackId, slot: usize, id: &str, value: f32) {
        if let Some(e) = self.side().editors.get_mut(&(track, slot)) {
            e.set_param(id, value);
        }
    }

    /// Start playback at `from`, stopping at `end`, or looping `looped`.
    pub fn play(&self, from: Samples, end: Option<Samples>, looped: Option<Range>) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        let media = {
            let mut side = self.side();
            side.loop_anchor = looped.filter(|r| r.len() > 64).map(|r| r.start);
            let anchor = side.loop_anchor;
            match side.prepare_media(from, anchor) {
                Ok(media) => media,
                Err(error) => {
                    self.fail_media(error);
                    return Err(error.to_string());
                }
            }
        };
        self.shared.position.store(from, Ordering::Relaxed);
        self.shared.playing.store(true, Ordering::Relaxed);
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.shared.media_error.store(0, Ordering::Release);
            self.tx.send(Cmd::PreparedPlay { from, end, looped, media }).map_err(|_| {
                self.shared.playing.store(false, Ordering::Release);
                "audio command receiver is stopped".to_string()
            })?;
        }
        #[cfg(target_arch = "wasm32")]
        self.tx.send(Cmd::Play { from, end, looped }).map_err(|_| {
            self.shared.playing.store(false, Ordering::Release);
            "audio command receiver is stopped".to_string()
        })?;
        Ok(())
    }

    /// Feed live input (from a `record::Recorder`) into the mix for input monitoring.
    pub fn set_input(&self, ring: Arc<record::InputRing>) {
        let _ = self.tx.send(Cmd::Input(ring));
    }

    pub fn set_recording(&self, on: bool) {
        let _ = self.tx.send(Cmd::Recording(on));
    }

    /// Graceful application shutdown: release editors on the main thread,
    /// drain audio-thread plugin instances back to the main thread,
    /// and only then destroy the audio stream.
    /// SCULPT_ORDERLY_PLUGIN_SHUTDOWN_V1
    pub fn shutdown(&mut self) {
        self.stop();
        {
            let mut side = self.side();
            for editor in side.editors.values_mut() {
                editor.close();
            }
            side.editors.clear();
            side.shadow.clear();
        }
        let (reply, done) = channel();
        if self.tx.send(Cmd::Shutdown(reply)).is_ok() && done.recv_timeout(std::time::Duration::from_secs(2)).is_err() {
            log::warn!("audio shutdown timed out; leaving still-referenced VST3 modules loaded");
        }
        // Dropping the stream stops further callbacks. The callback has
        // already surrendered plugin instances in the acknowledged path.
        self._stream.take();
        self.side().collect_garbage();
    }

    pub fn stop(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Stop);
    }

    pub fn locate(&self, at: Samples) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut side = self.side();
            let anchor = side.loop_anchor;
            let media = if self.is_playing() {
                match side.prepare_media(at, anchor) {
                    Ok(media) => media,
                    Err(error) => {
                        self.fail_media(error);
                        return;
                    }
                }
            } else {
                Arc::clone(&side.media)
            };
            self.shared.position.store(at, Ordering::Relaxed);
            let _ = self.tx.send(Cmd::PreparedLocate(at, media));
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.shared.position.store(at, Ordering::Relaxed);
            let _ = self.tx.send(Cmd::Locate(at));
        }
    }

    pub fn set_speed(&self, speed: f32) {
        let v = if speed.is_finite() { (speed.clamp(0.1, 4.0) * 1000.0) as u32 } else { 1000 };
        self.shared.speed.store(v, Ordering::Relaxed);
    }

    /// Playback rate last requested, 1.0 at full speed.
    pub fn speed(&self) -> f32 {
        self.shared.speed.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn position(&self) -> Samples {
        self.shared.position.load(Ordering::Relaxed)
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn meters(&self) -> MeterSnapshot {
        self.shared.meters.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Copy meters into reusable UI storage. Once track/channel counts are stable this performs
    /// no allocation, unlike cloning a new snapshot every video frame.
    pub fn copy_meters(&self, out: &mut MeterSnapshot) {
        let source = self.shared.meters.lock().unwrap_or_else(PoisonError::into_inner);
        copy_meter_snapshot(&source, out);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn media_error_code(error: CacheError) -> u8 {
    match error {
        CacheError::NotReady => 1,
        CacheError::ReadFailed => 2,
        CacheError::MediaChanged => 3,
        CacheError::Capacity => 4,
        CacheError::Stopped => 5,
        CacheError::InvalidRange => 6,
        CacheError::Timeout => 7,
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn media_error_from_code(code: u8) -> Option<CacheError> {
    match code {
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

fn copy_meter_snapshot(source: &MeterSnapshot, out: &mut MeterSnapshot) {
    out.tracks.retain(|id, _| source.tracks.contains_key(id));
    for (id, meter) in &source.tracks {
        out.tracks.entry(*id).or_default().copy_from(meter);
    }
    out.main.copy_from(&source.main);
}

impl Side {
    /// Creates the third-party instances `s` needs that the audio thread does not have yet.
    fn prepare(&mut self, s: &Session) -> Vec<PreparedInstance> {
        self.collect_garbage();
        let sr = s.sample_rate.as_f64() as f32;
        if (sr - self.rate).abs() > 0.5 {
            // The audio thread rebuilds its engine: everything is created anew.
            let keys: Vec<(TrackId, usize)> = self.shadow.keys().copied().collect();
            for k in keys {
                self.forget(k);
            }
            self.rate = sr;
        }
        let specs = soundcraft_mix::instance_specs(s);
        let stale: Vec<(TrackId, usize)> = self.shadow.keys().filter(|k| !specs.iter().any(|x| (x.track, x.slot) == **k)).copied().collect();
        for k in stale {
            self.forget(k);
        }
        let mut out = Vec::new();
        for spec in specs {
            let key = (spec.track, spec.slot);
            if self.shadow.get(&key).is_some_and(|(id, ch, _)| *id == spec.id && *ch == spec.ch) {
                continue;
            }
            self.forget(key);
            let made = soundcraft_mix::create_instance(&spec.id, spec.state.as_deref(), sr, BLOCK, spec.ch);
            self.shadow.insert(key, (spec.id.clone(), spec.ch, made.is_some()));
            if let Some(mut plugin) = made {
                if let Some(e) = plugin.editor() {
                    self.editors.insert(key, e);
                }
                out.push(PreparedInstance { track: spec.track, slot: spec.slot, id: spec.id, ch: spec.ch, plugin });
            }
        }
        out
    }
}

/// ITU stereo fold-down gains for each main channel of a session.
fn fold_gains(s: &Session) -> [(f32, f32); MAX_CHANNELS] {
    let mut g = [(0.0, 0.0); MAX_CHANNELS];
    let f = s.main_format();
    let sp = f.speakers();
    if sp.len() == f.channels() {
        for (o, x) in g.iter_mut().zip(sp.iter()) {
            *o = itu_stereo_fold(*x);
        }
    } else {
        // Ambisonics: W only, at -3 dB per side.
        g[0] = (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);
    }
    g
}

/// SCULPT_CORE21_PLAY_DESIRED_CHANNELS: requested physical I/O width (max 64).
pub fn sculpt_required_output_channels(s: &Session) -> usize {
    let mut want = main_channels(s);
    for t in &s.tracks {
        if let soundcraft_model::Route::Hardware(path) = &t.mixer.output
            && let Some(p) = s.outputs.iter().find(|o| o.name.eq_ignore_ascii_case(path))
        {
            want = want.max(usize::from(p.first_channel).saturating_add(p.format.channels()).min(64));
        }
    }
    for bus in &s.busses {
        if let Some(path) = bus.output.as_deref().and_then(|key| s.output_path(key)) {
            want = want.max(usize::from(path.first_channel).saturating_add(path.format.channels()));
        }
    }
    if let Some(main) = s.outputs.first() {
        want = want.max(usize::from(main.first_channel).saturating_add(main.format.channels()));
    }
    want.clamp(2, 64)
}

/// Open the default output device: f32 at the session rate if possible, with as many channels as
/// it offers up to `want_ch` (at least stereo when available).
fn open_device(
    want_rate: u32,
    _want_ch: u16,
    settings: &sculpt_audio_settings::AudioSettings,
) -> Result<(cpal::Device, cpal::StreamConfig, String), String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    // SCULPT_CORE22_EXPLICIT_DEVICE: select the same named CoreAudio device as Show.
    // CPAL 0.16 has no portable device UID; reject ambiguous duplicate names.
    let device = if settings.output_name.trim().is_empty() {
        host.default_output_device().ok_or_else(|| "no default output device".to_string())?
    } else {
        #[allow(deprecated)]
        let mut matches = host.output_devices().map_err(|e| e.to_string())?.filter(|d| d.name().is_ok_and(|name| name == settings.output_name));
        let match_one = matches.next().ok_or_else(|| format!("Selected CoreAudio interface disconnected: {}", settings.output_name))?;
        if matches.next().is_some() {
            return Err(format!("Ambiguous interface name: {} (use a uniquely named CoreAudio device)", settings.output_name));
        }
        match_one
    };
    #[allow(deprecated)]
    let name = device.name().unwrap_or_else(|_| "Audio device".into());
    let rate = if settings.sample_rate > 0 { settings.sample_rate } else { want_rate };
    let config = sculpt_audio_settings::stream_config(&device, true, rate, settings.buffer_frames)?;
    Ok((device, config, name)) // SCULPT_CORE22_BUFFER_APPLIED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reusable_meter_snapshot_updates_and_drops_stale_tracks() {
        let keep = TrackId(1);
        let stale = TrackId(2);
        let mut source = MeterSnapshot::default();
        source.tracks.insert(keep, StripMeter { peak: [0.5, 0.25], peaks: vec![0.5, 0.25], gain_reduction_db: 3.0 });
        source.main.peaks = vec![0.75, 0.5];
        let mut out = MeterSnapshot::default();
        out.tracks.insert(stale, StripMeter::default());
        copy_meter_snapshot(&source, &mut out);
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[&keep], source.tracks[&keep]);
        assert_eq!(out.main, source.main);
    }

    #[test]
    fn audio_state_plays_and_stops_at_end() {
        let s = Arc::new(soundcraft_model::Session::default());
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
            callback_frames: AtomicU32::new(0),
            stream_failed: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            media_error: AtomicU8::new(0),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: Some(1000), looped: None }).unwrap();
        let mut out = vec![0.0f32; 4096];
        st.fill(&mut out, 2);
        assert!(shared.position.load(Ordering::Relaxed) >= 1000);
        assert!(!shared.playing.load(Ordering::Relaxed));
    }

    #[test]
    fn short_device_callbacks_render_short_mix_blocks() {
        let s = Arc::new(soundcraft_model::Session::default());
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
            callback_frames: AtomicU32::new(0),
            stream_failed: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            media_error: AtomicU8::new(0),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        let mut out = vec![0.0f32; 2 * 64];
        st.fill(&mut out, 2);
        assert_eq!(shared.position.load(Ordering::Relaxed), 64);
    }

    #[test]
    fn surround_main_maps_to_device_channels_or_folds_down() {
        use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
        let mut s = soundcraft_model::Session::default();
        s.outputs[0].format = ChannelFormat::Surround51;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Surround51, None);
        let chans: Vec<Vec<f32>> = (0..6).map(|c| vec![0.1 * (c + 1) as f32; 48_000]).collect();
        s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: chans })));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(5), 0, 0, 48_000));
        let s = Arc::new(s);
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
            callback_frames: AtomicU32::new(0),
            stream_failed: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            media_error: AtomicU8::new(0),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::clone(&s), 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: None }).unwrap();
        // An 8-channel device: channels in order, the rest silent.
        let mut out = vec![0.0f32; 8 * 256];
        st.fill(&mut out, 8);
        let f = &out[8 * 100..8 * 101];
        for c in 0..6 {
            assert!((f[c] - 0.1 * (c + 1) as f32).abs() < 1e-5, "{f:?}");
        }
        assert!(f[6] == 0.0 && f[7] == 0.0);
        // A stereo device: ITU fold-down.
        let mut out = vec![0.0f32; 2 * 256];
        st.fill(&mut out, 2);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let l = (0.1 + h * 0.3 + h * 0.5f32).min(1.0);
        assert!((out[200] - l).abs() < 1e-4, "{} vs {l}", out[200]);
        assert_eq!(shared.meters.lock().unwrap().main.peaks.len(), 6);
    }

    struct Stateful(Box<dyn Plugin>);

    impl Plugin for Stateful {
        fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
            self.0.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.0.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.0.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.0.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.0.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.0.process(io, frames);
        }
        fn save_state(&mut self) -> Option<Vec<u8>> {
            Some(b"blob".to_vec())
        }
    }

    #[test]
    fn instances_arrive_with_the_session_and_leave_through_the_return_channel() {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().mixer.inserts[1] = Some(Insert::new("clap:test.fake"));
        let s = Arc::new(s);
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
            callback_frames: AtomicU32::new(0),
            stream_failed: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            media_error: AtomicU8::new(0),
        });
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(soundcraft_model::Session::default()), 48_000.0, gtx);
        let plugin: Box<dyn Plugin> = Box::new(Stateful(soundcraft_dsp::create("gain").unwrap()));
        tx.send(Cmd::Session(Arc::clone(&s), vec![PreparedInstance { track: t, slot: 1, id: "clap:test.fake".into(), ch: 2, plugin }])).unwrap();
        let (rtx, rrx) = channel();
        tx.send(Cmd::CaptureStates(rtx)).unwrap();
        // Stopped: the callback only drains commands (adopting the instance) and answers.
        let mut out = vec![0.0f32; 256];
        st.fill(&mut out, 2);
        assert_eq!(rrx.try_recv().unwrap(), vec![(t, 1, b"blob".to_vec())]);
        assert!(grx.try_recv().is_err(), "nothing retired yet");
        // A session without the insert retires the instance to the return channel.
        tx.send(Cmd::Session(Arc::new(soundcraft_model::Session::default()), Vec::new())).unwrap();
        st.fill(&mut out, 2);
        assert_eq!(grx.try_recv().unwrap().len(), 1);
        // A sample-rate change rebuilds the engine without dropping plugins on this thread.
        tx.send(Cmd::Session(
            Arc::clone(&s),
            vec![PreparedInstance {
                track: t,
                slot: 1,
                id: "clap:test.fake".into(),
                ch: 2,
                plugin: Box::new(Stateful(soundcraft_dsp::create("gain").unwrap())),
            }],
        ))
        .unwrap();
        st.fill(&mut out, 2);
        let mut s44 = (*s).clone();
        s44.sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
        tx.send(Cmd::Session(Arc::new(s44), Vec::new())).unwrap();
        st.fill(&mut out, 2);
        assert_eq!(grx.try_recv().unwrap().len(), 1);
    }

    #[test]
    fn side_creates_each_missing_instance_once() {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let (_gtx, grx) = channel();
        let mut side = Side::new(Arc::new(Session::default()), grx);
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("clap:does.not.exist"));
        s.track_mut(t).unwrap().mixer.inserts[1] = Some(Insert::new("gain"));
        assert!(side.prepare(&s).is_empty(), "the plugin is not installed; built-ins stay with the engine");
        assert_eq!(side.shadow.get(&(t, 0)), Some(&("clap:does.not.exist".to_string(), 1, false)));
        assert!(side.prepare(&s).is_empty());
        assert_eq!(side.shadow.len(), 1, "failures are remembered, not retried every update");
        s.track_mut(t).unwrap().mixer.inserts[0] = None;
        side.prepare(&s);
        assert!(side.shadow.is_empty());
    }

    #[test]
    fn looping_wraps() {
        let s = Arc::new(soundcraft_model::Session::default());
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
            callback_frames: AtomicU32::new(0),
            stream_failed: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            media_error: AtomicU8::new(0),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, channel().0);
        tx.send(Cmd::Play { from: 0, end: None, looped: Some(Range::new(0, 1000)) }).unwrap();
        let mut out = vec![0.0f32; 20_000];
        st.fill(&mut out, 2);
        let p = shared.position.load(Ordering::Relaxed);
        assert!(p <= 1000, "position {p} should stay inside the loop");
        assert!(shared.playing.load(Ordering::Relaxed));
    }

    #[test]
    fn armed_tracks_are_heard_and_metered_after_playback_stops() {
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        s.track_mut(t).unwrap().mixer.input = soundcraft_model::Route::Hardware("In 1".into());
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
            callback_frames: AtomicU32::new(0),
            stream_failed: AtomicBool::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            media_error: AtomicU8::new(0),
        });
        let (tx, rx) = channel();
        let mut st = AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, channel().0);
        let ring = record::InputRing::new(1, 48_000);
        tx.send(Cmd::Input(Arc::clone(&ring))).unwrap();
        tx.send(Cmd::Play { from: 0, end: Some(1000), looped: None }).unwrap();
        let mut out = vec![0.0f32; 4096];
        st.fill(&mut out, 2);
        assert!(!shared.playing.load(Ordering::Relaxed));
        ring.push(&[0.5; 4096]);
        st.fill(&mut out, 2);
        assert!(out.iter().any(|x| x.abs() > 0.1), "the input is heard");
        assert!(shared.meters.lock().unwrap().tracks.get(&t).is_some_and(|m| m.peak[0] > 0.1), "and metered");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)]
mod media_tests {
    use super::*;
    use soundcraft_audio_io::{
        AudioBuffer, BitDepth, EncodeOptions,
        stream_cache::{CacheConfig, StreamCache},
    };
    use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
    use std::{path::PathBuf, sync::atomic::AtomicU64, time::Duration};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn failed_instrument_preparation_stays_failed_on_repeated_play() {
        let mut s = Session::default();
        let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let mut instrument = soundcraft_model::Insert::new("sampler");
        instrument.sample = Some(SourceId(999));
        s.track_mut(track).unwrap().instrument = Some(instrument);
        let (player, _state) = host(Arc::new(s));
        for _ in 0..3 {
            assert!(player.play(0, None, None).is_err());
            assert!(!player.is_playing());
            assert_eq!(player.side().media_problem, Some(CacheError::ReadFailed));
        }
    }
    #[test]
    fn rejected_instrument_edit_cannot_restart_the_previous_session() {
        let original = Arc::new(Session::default());
        let (player, mut state) = host(Arc::clone(&original));
        let mut invalid = (*original).clone();
        let track = invalid.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        let mut instrument = soundcraft_model::Insert::new("sampler");
        instrument.sample = Some(SourceId(999));
        invalid.track_mut(track).unwrap().instrument = Some(instrument);
        assert!(player.update_session(Arc::new(invalid)).is_err());
        for _ in 0..3 {
            assert!(player.play(0, None, None).is_err());
            assert!(!player.is_playing());
        }
        state.drain();
        player.update_session(original).unwrap();
        player.play(0, None, None).unwrap();
        assert!(player.is_playing());
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("sculpt-player-media-{}-{}.wav", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
            let bytes = soundcraft_audio_io::encode(
                &AudioBuffer::new(48_000, 2, 30_000).with_constant(),
                &EncodeOptions { bit_depth: BitDepth::Float32, dither: false, ..EncodeOptions::default() },
            )
            .unwrap();
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }
    // Keep synthetic fixtures entirely in the test module.
    trait Constant {
        fn with_constant(self) -> Self;
    }
    impl Constant for AudioBuffer {
        fn with_constant(mut self) -> Self {
            for ch in &mut self.channels {
                ch.fill(0.25);
            }
            self
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn session(fixture: &Fixture, cache: &Arc<StreamCache>) -> Arc<Session> {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let source = cache.open_source(&fixture.0).unwrap();
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::from_stream(source, 200, |_, _| true).unwrap()));
        let clip = Clip::audio(s.new_clip_id(), "disk", SourceId(900), 0, 0, 30_000);
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(clip);
        Arc::new(s)
    }
    fn host(s: Arc<Session>) -> (Player, AudioState) {
        let (tx, rx) = channel();
        let (gtx, grx) = channel();
        let shared = Arc::new(Shared {
            position: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            speed: AtomicU32::new(1000),
            meters: Mutex::new(MeterSnapshot::default()),
            callback_frames: AtomicU32::new(0),
            stream_failed: AtomicBool::new(false),
            media_error: AtomicU8::new(0),
        });
        let side = Side::new(Arc::clone(&s), grx);
        let mut state = AudioState::new(rx, Arc::clone(&shared), s, 48_000.0, gtx);
        state.mix.set_media(Arc::clone(&side.media));
        let player = Player {
            shared,
            tx,
            side: Mutex::new(side),
            _stream: None,
            device_name: "test".into(),
            device_rate: 48_000,
            device_channels: 2,
            device_buffer: Some(512),
            open_error: None,
            silent: false,
        };
        (player, state)
    }
    #[test]
    fn player_prepares_play_loop_locate_and_reuses_media_for_mixer_edits() {
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let s = session(&fixture, &cache);
        let (player, mut state) = host(Arc::clone(&s));
        player.update_session(Arc::clone(&s)).unwrap();
        let original_media = Arc::clone(&player.side().media);
        player.play(0, None, Some(Range::new(0, 1024))).unwrap();
        let mut out = vec![0.0; 8192];
        state.fill(&mut out, 2);
        assert!(out.iter().all(|&x| (x - 0.25).abs() < 1e-6));
        assert!(state.pos <= 1024);
        assert!(player.take_media_error().is_none());
        let mut edit = (*s).clone();
        edit.tracks[0].mixer.volume_db = -6.0;
        player.update_session(Arc::new(edit)).unwrap();
        assert!(Arc::ptr_eq(&original_media, &player.side().media));
        state.drain();
        player.play(20_000, None, None).unwrap();
        state.fill(&mut out[..1024], 2);
        assert!(state.pos >= 20_000);
        assert!(out[..1024].iter().any(|&x| x > 0.0));
        player.locate(1000);
        state.fill(&mut out[..1024], 2);
        assert!((1000..=2024).contains(&state.pos));
        assert!(player.take_media_error().is_none());
        player.stop();
        state.fill(&mut out[..1024], 2);
        assert!(out[..1024].iter().all(|&x| x == 0.0));
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn callback_media_failure_stops_transport_and_reports_once() {
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let s = session(&fixture, &cache);
        let (player, mut state) = host(s);
        player.play(0, None, None).unwrap();
        cache.shutdown(Duration::from_secs(1)).unwrap();
        let mut out = vec![1.0; 1024];
        state.fill(&mut out, 2);
        assert!(out.iter().all(|&x| x == 0.0));
        assert!(!player.is_playing());
        assert_eq!(state.pos, 0);
        assert_eq!(player.take_media_error(), Some(CacheError::Stopped));
        assert_eq!(player.take_media_error(), None);
        assert!(player.play(0, None, None).is_err());
        assert!(!player.is_playing());
    }
    #[test]
    fn failed_preflight_does_not_start_or_advance_and_retired_media_drop_on_control() {
        let fixture = Fixture::new();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let s = session(&fixture, &cache);
        let (player, mut state) = host(Arc::clone(&s));
        let mut edit = (*s).clone();
        edit.tracks[0].playlist_mut().unwrap().clips[0].start = 100;
        player.update_session(Arc::new(edit)).unwrap();
        assert_eq!(player.side().retired_media.len(), 1);
        player.side().collect_garbage();
        assert_eq!(player.side().retired_media.len(), 1);
        state.drain();
        player.side().collect_garbage();
        assert!(player.side().retired_media.is_empty());
        std::fs::OpenOptions::new().write(true).open(&fixture.0).unwrap().set_len(100).unwrap();
        assert!(player.play(100, None, None).is_err());
        assert!(!player.is_playing());
        assert_eq!(player.position(), 0);
        assert_eq!(player.take_media_error(), Some(CacheError::MediaChanged));
        let mut out = vec![1.0; 1024];
        state.fill(&mut out, 2);
        assert!(out.iter().all(|&x| x == 0.0));
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn stop_discards_pending_clip_samples_before_input_monitoring() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer::new(48_000, 2, 2048).with_constant())));
        let clip = Clip::audio(s.new_clip_id(), "resident", SourceId(900), 0, 0, 2048);
        let track = s.track_mut(t).unwrap();
        track.playlist_mut().unwrap().clips.push(clip);
        track.mixer.record_arm = true;
        track.mixer.input = soundcraft_model::Route::Hardware("Input 1-2".into());
        let (player, mut state) = host(Arc::new(s));
        state.input = Some(record::InputRing::new(2, 48_000));
        player.play(0, None, None).unwrap();
        state.drain();
        state.render_next(BLOCK);
        assert!(state.session.needs_hardware_input());
        let mut first = [0.0; 2];
        state.fill(&mut first, 2);
        assert!(first.iter().any(|&x| x > 0.0));
        assert!(state.pending_len > 1);
        player.stop();
        let mut stopped = [1.0; 32];
        state.fill(&mut stopped, 2);
        assert!(stopped.iter().all(|&x| x == 0.0));
    }
}
