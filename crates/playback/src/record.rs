//! Audio input capture for recording.
//!
//! A [`Recorder`] opens the default input device. While armed, the input callback copies each
//! block into a preallocated lock-free ring (it never locks or allocates), and a writer thread
//! streams the ring into one WAV file per take, rewriting the header every second so a crash leaves
//! a playable file.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use soundcraft_model::TrackId;

/// What [`Recorder::finish`] returns: the takes that hold audio and the first problem met.
pub type Takes = (Vec<(TrackId, PathBuf)>, Option<String>);

const RING_SECONDS: usize = 4;

/// Single-producer single-consumer FIFO of samples (input callback → writer thread). The slots
/// are atomics, so it needs no lock and no `unsafe`.
struct CaptureRing {
    slots: Box<[AtomicU32]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl CaptureRing {
    fn new(capacity: usize) -> CaptureRing {
        let n = capacity.max(2).next_power_of_two();
        CaptureRing { slots: (0..n).map(|_| AtomicU32::new(0)).collect(), mask: n - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    /// Queue up to `n` samples (`sample(i)` is the i-th); returns how many fit.
    fn push(&self, n: usize, sample: impl Fn(usize) -> f32) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let n = n.min(self.slots.len().saturating_sub(head.wrapping_sub(self.tail.load(Ordering::Acquire))));
        for i in 0..n {
            if let Some(slot) = self.slots.get(head.wrapping_add(i) & self.mask) {
                slot.store(sample(i).to_bits(), Ordering::Relaxed);
            }
        }
        self.head.store(head.wrapping_add(n), Ordering::Release);
        n
    }

    fn pop_all(&self, out: &mut Vec<f32>) {
        let (tail, head) = (self.tail.load(Ordering::Relaxed), self.head.load(Ordering::Acquire));
        out.extend(
            (0..head.wrapping_sub(tail))
                .map(|i| self.slots.get(tail.wrapping_add(i) & self.mask).map_or(0.0, |s| f32::from_bits(s.load(Ordering::Relaxed)))),
        );
        self.tail.store(head, Ordering::Release);
    }
}

struct Shared {
    armed: AtomicBool,
    ring: CaptureRing,
    /// Samples lost while the ring was full.
    dropped: AtomicUsize,
}

/// The actual CPAL error callback, also exercised without an audio device by the regression test.
#[cfg(not(target_os = "freebsd"))]
fn input_error_callback(monitor: Arc<InputRing>) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |error| {
        // Possibly the audio thread: the logger must hand the record off rather than write it.
        crate::mark_audio_thread();
        monitor.fail_stream();
        log::warn!("input stream error: {error}");
    }
}

impl Shared {
    /// The input callback's work while armed. Samples that don't fit are owed (`owed`) and queued
    /// as silence before the next block, so the take keeps its timing.
    fn capture(&self, data: &[f32], owed: &mut usize) {
        *owed -= self.ring.push(*owed, |_| 0.0);
        let kept = if *owed == 0 { self.ring.push(data.len(), |i| data.get(i).copied().unwrap_or(0.0)) } else { 0 };
        *owed = owed.saturating_add(data.len() - kept);
        self.dropped.fetch_add(data.len() - kept, Ordering::Relaxed);
    }
}

pub struct Recorder {
    shared: Arc<Shared>,
    /// Live input for monitoring (always fed while the input stream runs).
    pub monitor: Arc<InputRing>,
    #[cfg(not(target_os = "freebsd"))]
    _stream: Option<cpal::Stream>,
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: usize,
    writer: Option<std::thread::JoinHandle<Takes>>,
    failure_reported: bool,
}

impl Recorder {
    /// Open the default input device. Errors when there is none (the caller shows a message).
    #[cfg(not(target_os = "freebsd"))]
    pub fn open() -> Result<Recorder, String> {
        Self::open_with_settings(&crate::sculpt_audio_settings::AudioSettings::read(), 0)
    }

    #[cfg(not(target_os = "freebsd"))]
    pub fn open_with_settings(configured: &crate::sculpt_audio_settings::AudioSettings, session_rate: u32) -> Result<Recorder, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        // SCULPT_CORE22_INPUT_DEVICE: explicit input choice from shared audio settings.
        let device = if configured.input_name.trim().is_empty() {
            host.default_input_device().ok_or("no audio input device")?
        } else {
            #[allow(deprecated)]
            let mut matches = host.input_devices().map_err(|e| e.to_string())?.filter(|d| d.name().is_ok_and(|name| name == configured.input_name));
            let selected = matches.next().ok_or_else(|| format!("Input interface disconnected: {}", configured.input_name))?;
            if matches.next().is_some() {
                return Err("Ambiguous input interface name".into());
            }
            selected
        };
        #[allow(deprecated)]
        let name = device.name().unwrap_or_else(|_| "Input".into());
        let rate = if configured.sample_rate > 0 {
            configured.sample_rate
        } else if session_rate > 0 {
            session_rate
        } else {
            device.default_input_config().map_err(|e| e.to_string())?.sample_rate().0
        };
        let config = crate::sculpt_audio_settings::stream_config(&device, false, rate, configured.buffer_frames)?;
        let channels = usize::from(config.channels).max(1);
        let ring = CaptureRing::new((config.sample_rate.0 as usize).saturating_mul(channels * RING_SECONDS).min(1 << 25));
        let shared = Arc::new(Shared { armed: AtomicBool::new(false), ring, dropped: AtomicUsize::new(0) });
        let sh = Arc::clone(&shared);
        let monitor = InputRing::new(usize::from(config.channels), config.sample_rate.0);
        let mon = Arc::clone(&monitor);
        let mut owed = 0;
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    crate::mark_audio_thread();
                    mon.push(data);
                    if sh.armed.load(Ordering::Acquire) && !mon.stream_failed() {
                        sh.capture(data, &mut owed);
                    } else {
                        owed = 0;
                    }
                },
                input_error_callback(Arc::clone(&monitor)),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Recorder {
            monitor,
            shared,
            _stream: Some(stream),
            device_name: name,
            sample_rate: config.sample_rate.0,
            channels,
            writer: None,
            failure_reported: false,
        })
    }

    #[cfg(target_os = "freebsd")]
    pub fn open() -> Result<Recorder, String> {
        Err("recording is not supported on this platform yet".into())
    }

    #[cfg(target_os = "freebsd")]
    pub fn open_with_settings(_configured: &crate::sculpt_audio_settings::AudioSettings, _session_rate: u32) -> Result<Recorder, String> {
        Err("recording is not supported on this platform yet".into())
    }

    /// Create each take's file and start recording into them.
    pub fn arm(&mut self, takes: Vec<(TrackId, Vec<usize>, PathBuf)>) -> Result<(), String> {
        if self.monitor.stream_failed() {
            return Err("Audio input stream failed. Reopen the input in Playback Engine before recording.".into());
        }
        if self.writer.is_some() {
            return Err("already recording".into());
        }
        let takes =
            takes.into_iter().map(|(id, inputs, path)| TakeWriter::create(id, inputs, path, self.sample_rate)).collect::<Result<Vec<_>, _>>()?;
        let ring = &self.shared.ring;
        ring.tail.store(ring.head.load(Ordering::Acquire), Ordering::Release);
        self.shared.armed.store(true, Ordering::Release);
        let (shared, channels) = (Arc::clone(&self.shared), self.channels);
        match std::thread::Builder::new().name("take writer".into()).spawn(move || write_takes(&shared, takes, channels)) {
            Ok(w) => self.writer = Some(w),
            Err(e) => {
                self.shared.armed.store(false, Ordering::Release);
                return Err(e.to_string());
            }
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Takes {
        self.shared.armed.store(false, Ordering::Release);
        let (takes, problem) = self.writer.take().and_then(|w| w.join().ok()).unwrap_or_default();
        let lost = self.shared.dropped.swap(0, Ordering::Relaxed) / self.channels.max(1);
        let slow =
            (lost > 0).then(|| format!("{} ms of input lost: the disk was too slow", lost.saturating_mul(1000) / self.sample_rate.max(1) as usize));
        let stream_problem =
            self.monitor.stream_failed().then(|| "Audio input stream failed; the take contains only audio captured before the failure.".into());
        let problems: Vec<String> = [problem, slow, stream_problem].into_iter().flatten().collect();
        (takes, (!problems.is_empty()).then(|| problems.join("; ")))
    }

    /// Report a failed input once to the UI. The failure stays latched until the stream is reopened.
    pub fn take_stream_failure(&mut self) -> bool {
        if self.stream_failed() && !self.failure_reported {
            self.failure_reported = true;
            true
        } else {
            false
        }
    }

    /// A reported stream error remains latched until a new Recorder is opened.
    pub fn stream_failed(&self) -> bool {
        self.monitor.stream_failed()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.finish();
    }
}

fn write_takes(shared: &Shared, mut takes: Vec<TakeWriter>, channels: usize) -> Takes {
    let (mut block, mut header_written) = (Vec::new(), Instant::now());
    loop {
        let stopping = !shared.armed.load(Ordering::Acquire);
        // The ring holds interleaved samples and a pop can end mid-frame: write whole frames and
        // keep the remainder for the next pop, so channels never shift.
        shared.ring.pop_all(&mut block);
        let whole = block.len() - block.len() % channels.max(1);
        takes.iter_mut().for_each(|t| t.write(&block[..whole], channels));
        block.drain(..whole);
        if stopping {
            break;
        }
        if header_written.elapsed() >= Duration::from_secs(1) {
            header_written = Instant::now();
            takes.iter_mut().for_each(TakeWriter::write_header);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (mut done, mut problem) = (Vec::new(), None);
    for mut t in takes {
        t.write_header();
        if t.frames == 0 {
            let _ = std::fs::remove_file(&t.path);
        } else {
            done.push((t.id, t.path));
        }
        problem = problem.or(t.error);
    }
    (done, problem)
}

/// One take's file. After an error it stops writing; its header keeps covering what was written.
struct TakeWriter {
    id: TrackId,
    inputs: Vec<usize>,
    path: PathBuf,
    file: File,
    rate: u32,
    frames: u64,
    error: Option<String>,
}

impl TakeWriter {
    fn create(id: TrackId, inputs: Vec<usize>, path: PathBuf, rate: u32) -> Result<TakeWriter, String> {
        let file = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| File::create_new(&path));
        let file = file.map_err(|e| format!("{}: {e}", path.display()))?;
        let mut take = TakeWriter { id, inputs, path, file, rate, frames: 0, error: None };
        take.write_header();
        take.error.take().map_or(Ok(take), Err)
    }

    /// Append this take's channels of `block` (interleaved, `channels` wide).
    fn write(&mut self, block: &[f32], channels: usize) {
        let channels = channels.max(1);
        if self.error.is_some() {
            return;
        }
        let bytes: Vec<u8> = block
            .chunks_exact(channels)
            .flat_map(|f| self.inputs.iter().flat_map(move |&i| f.get(i).copied().unwrap_or(0.0).to_le_bytes()))
            .collect();
        match self.file.write_all(&bytes) {
            Ok(()) => self.frames += (block.len() / channels) as u64,
            Err(e) => self.fail(e),
        }
    }

    fn write_header(&mut self) {
        let header = match soundcraft_audio_io::wav_float_header(self.inputs.len(), self.rate, self.frames) {
            Ok(h) => h,
            Err(e) => return self.fail(e),
        };
        let f = &mut self.file;
        if let Err(e) = f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&header)).and_then(|()| f.seek(SeekFrom::End(0))) {
            self.fail(e);
        }
    }

    fn fail(&mut self, e: impl std::fmt::Display) {
        self.error.get_or_insert_with(|| format!("{}: {e}", self.path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(path: &PathBuf) -> Vec<Vec<f32>> {
        soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), None).unwrap().1.channels
    }

    #[test]
    fn a_full_ring_owes_the_lost_samples_as_silence() {
        let shared = Shared { armed: AtomicBool::new(true), ring: CaptureRing::new(8), dropped: AtomicUsize::new(0) };
        let (mut owed, mut out) = (0, Vec::new());
        shared.capture(&[1.0; 6], &mut owed);
        shared.capture(&[2.0; 4], &mut owed);
        shared.ring.pop_all(&mut out);
        shared.capture(&[3.0; 2], &mut owed);
        shared.ring.pop_all(&mut out);
        assert_eq!(out, [vec![1.0; 6], vec![2.0; 2], vec![0.0; 2], vec![3.0; 2]].concat());
        assert_eq!((owed, shared.dropped.load(Ordering::Relaxed)), (0, 2));
    }

    #[test]
    fn takes_stream_into_their_files() {
        let dir = std::env::temp_dir().join(format!("soundcraft-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let gtr = TakeWriter::create(TrackId(1), vec![1, 2], dir.join("Gtr_01.wav"), 48_000).unwrap();
        let mut vox = TakeWriter::create(TrackId(2), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
        assert!(TakeWriter::create(TrackId(3), vec![0], dir.join("Vox_01.wav"), 48_000).is_err(), "never overwrites a file");
        vox.write(&[0.5; 300], 3);
        vox.write_header();
        vox.write(&[0.25; 150], 3);
        assert_eq!(decode(&dir.join("Vox_01.wav"))[0].len(), 100, "the header covers what it has seen");

        let shared = Arc::new(Shared { armed: AtomicBool::new(true), ring: CaptureRing::new(1 << 16), dropped: AtomicUsize::new(0) });
        let sh = Arc::clone(&shared);
        let writer = std::thread::spawn(move || write_takes(&sh, vec![gtr, vox], 3));
        let input: Vec<f32> = (0..3 * 4_800).map(|i| i as f32 / 20_000.0).collect();
        let mut owed = 0;
        input.chunks(3 * 480).for_each(|block| shared.capture(block, &mut owed));
        shared.armed.store(false, Ordering::Release);
        let (done, problem) = writer.join().unwrap();
        let channel = |c: usize| input.iter().skip(c).step_by(3).copied().collect::<Vec<f32>>();
        assert_eq!((done.len(), problem), (2, None));
        assert_eq!(decode(&done[0].1), vec![channel(1), channel(2)]);
        assert_eq!(decode(&done[1].1), vec![[vec![0.5; 100], vec![0.25; 50], channel(0)].concat()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(not(target_os = "freebsd"))]
    fn failed_input_finalizes_captured_stereo_and_cannot_rearm() {
        let dir = std::env::temp_dir().join(format!("sculpt-failed-input-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let shared = Arc::new(Shared { armed: AtomicBool::new(false), ring: CaptureRing::new(1024), dropped: AtomicUsize::new(0) });
        let mut recorder = Recorder {
            shared,
            monitor: InputRing::new(2, 48_000),
            #[cfg(not(target_os = "freebsd"))]
            _stream: None,
            device_name: "Synthetic input".into(),
            sample_rate: 48_000,
            channels: 2,
            writer: None,
            failure_reported: false,
        };
        assert!(!recorder.take_stream_failure());
        let path = dir.join("take.wav");
        recorder.arm(vec![(TrackId(1), vec![0, 1], path.clone())]).unwrap();
        let input: Vec<f32> = (0..256).map(|i| i as f32 / 512.0).collect();
        recorder.shared.capture(&input, &mut 0);
        recorder.monitor.push(&input);
        let mut on_error = input_error_callback(Arc::clone(&recorder.monitor));
        std::thread::spawn(move || on_error(cpal::StreamError::DeviceNotAvailable)).join().unwrap();
        assert!(recorder.take_stream_failure());
        assert!(!recorder.take_stream_failure(), "one UI notification per stream");
        let (takes, problem) = recorder.finish();
        assert_eq!(takes, vec![(TrackId(1), path.clone())]);
        assert!(problem.unwrap().contains("input stream failed"));
        assert_eq!(decode(&path), vec![input.iter().step_by(2).copied().collect::<Vec<_>>(), input.iter().skip(1).step_by(2).copied().collect()]);
        let mut monitor = vec![vec![1.0; 32]; 2];
        recorder.monitor.pop_into(&mut monitor, 32, 48_000);
        assert!(monitor.iter().flatten().all(|x| *x == 0.0), "never replay stale monitoring audio");
        let next = dir.join("must-not-exist.wav");
        assert!(recorder.arm(vec![(TrackId(1), vec![0, 1], next.clone())]).is_err());
        assert!(!next.exists(), "reject before creating any take files");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Ring state behind the lock: interleaved input frames at the input device's rate, plus the
/// fractional read position (in input frames) of the next output sample.
struct RingState {
    data: std::collections::VecDeque<f32>,
    phase: f64,
}

/// A small interleaved FIFO from the input callback to the output callback, for input
/// monitoring. Both sides only `try_lock`; it holds at most ~0.5 s and drops the oldest frames.
/// The input device may run at a different rate than the session, so the reader converts
/// rates with linear interpolation (cheap, allocation-free, fine for monitoring).
pub struct InputRing {
    stream_failed: AtomicBool,
    state: Mutex<RingState>,
    pub channels: std::sync::atomic::AtomicUsize,
    /// Input device sample rate in Hz (0 when unknown, which reads as "same as the session").
    rate: u32,
    /// Most samples kept: half a second of input, or 24 000 frames when the rate is unknown.
    cap: usize,
}

impl InputRing {
    pub fn new(channels: usize, rate: u32) -> Arc<InputRing> {
        let ch = channels.max(1);
        let frames = if rate == 0 { 24_000 } else { rate as usize / 2 };
        Arc::new(InputRing {
            stream_failed: AtomicBool::new(false),
            state: Mutex::new(RingState { data: std::collections::VecDeque::with_capacity(rate.max(48_000) as usize * ch), phase: 0.0 }),
            channels: std::sync::atomic::AtomicUsize::new(ch),
            rate,
            cap: frames * ch,
        })
    }

    pub fn push(&self, samples: &[f32]) {
        if self.stream_failed() {
            return;
        }
        if let Ok(mut s) = self.state.try_lock() {
            s.data.extend(samples.iter().copied());
            while s.data.len() > self.cap {
                s.data.pop_front();
            }
        }
    }

    #[cfg(not(target_os = "freebsd"))]
    fn fail_stream(&self) {
        self.stream_failed.store(true, Ordering::Release);
    }

    fn stream_failed(&self) -> bool {
        self.stream_failed.load(Ordering::Acquire)
    }

    /// Fill `out` (planar, at `session_rate`) with up to `frames` frames, converting from the
    /// input rate. Missing frames are silence.
    pub fn pop_into(&self, out: &mut [Vec<f32>], frames: usize, session_rate: u32) {
        let ch = self.channels.load(Ordering::Relaxed).max(1);
        for c in out.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        if self.stream_failed() {
            return;
        }
        let Ok(mut s) = self.state.try_lock() else { return };
        // Input frames consumed per output frame.
        let step = if self.rate == 0 || session_rate == 0 { 1.0 } else { f64::from(self.rate) / f64::from(session_rate) };
        for f in 0..frames {
            // Drop input frames we have moved past; keep the one at `phase` and the next one.
            while s.phase >= 1.0 && s.data.len() >= 2 * ch {
                s.data.drain(..ch);
                s.phase -= 1.0;
            }
            if s.phase >= 1.0 || s.data.len() < 2 * ch {
                break;
            }
            let fr = s.phase as f32;
            for c in 0..ch {
                let a = s.data.get(c).copied().unwrap_or(0.0);
                let b = s.data.get(ch + c).copied().unwrap_or(a);
                if let Some(x) = out.get_mut(c).and_then(|o| o.get_mut(f)) {
                    *x = a + (b - a) * fr;
                }
            }
            s.phase += step;
        }
    }
}

#[cfg(test)]
mod ring_tests {
    use super::InputRing;

    /// A 1 kHz sine recorded at 16 kHz must come out as a 1 kHz sine at 48 kHz, across calls.
    #[test]
    fn monitor_converts_input_rate_to_session_rate() {
        let ring = InputRing::new(1, 16_000);
        let input: Vec<f32> = (0..4_000).map(|i| (2.0 * std::f64::consts::PI * 1_000.0 * f64::from(i) / 16_000.0).sin() as f32).collect();
        ring.push(&input);
        let mut out = vec![vec![0.0f32; 48]];
        let mut k = 0usize;
        for _ in 0..10 {
            ring.pop_into(&mut out, 48, 48_000);
            for v in &out[0] {
                let want = (2.0 * std::f64::consts::PI * 1_000.0 * k as f64 / 48_000.0).sin() as f32;
                assert!((v - want).abs() < 0.05, "sample {k}: got {v}, want {want}");
                k += 1;
            }
        }
    }

    /// Input faster than the session (96 kHz into 48 kHz): every other frame is read, in order.
    #[test]
    fn monitor_decimates_when_input_is_faster() {
        let ring = InputRing::new(1, 96_000);
        let input: Vec<f32> = (0..1_000).map(|i| i as f32).collect();
        ring.push(&input);
        let mut out = vec![vec![0.0f32; 100]];
        ring.pop_into(&mut out, 100, 48_000);
        for (k, v) in out[0].iter().enumerate() {
            assert_eq!(*v, (2 * k) as f32, "output {k}");
        }
    }

    /// Same rate: samples pass through unchanged.
    #[test]
    fn monitor_passes_through_at_equal_rates() {
        let ring = InputRing::new(2, 48_000);
        let input: Vec<f32> = (0..200).map(|i| i as f32).collect();
        ring.push(&input);
        let mut out = vec![vec![0.0f32; 50], vec![0.0f32; 50]];
        ring.pop_into(&mut out, 50, 48_000);
        assert_eq!(out[0][0], 0.0);
        assert_eq!(out[1][0], 1.0);
        assert_eq!(out[0][49], 98.0);
    }

    /// Nothing buffered: silence, not stale data.
    #[test]
    fn monitor_underrun_is_silence() {
        let ring = InputRing::new(1, 16_000);
        let mut out = vec![vec![1.0f32; 16]];
        ring.pop_into(&mut out, 16, 48_000);
        assert!(out[0].iter().all(|v| *v == 0.0));
    }
}
