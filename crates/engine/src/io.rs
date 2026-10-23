//! Import, export, save/load, rendering helpers (bounce, consolidate, commit) and analysis.

use crate::{Engine, EngineError, Result};
use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, EncodeOptions, FileFormat};
use soundcraft_model::{BusId, ChannelFormat, Clip, ClipContent, Session, Source, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Largest import we accept (samples per channel × channels), ~4 GiB of f32.
const MAX_IMPORT_SAMPLES: usize = 1 << 30;

/// A relative `Audio Files/` path no other source in `s` already uses.
fn unique_audio_rel(s: &Session, stem: &str) -> String {
    let stem = sanitize_name(stem);
    let mut path = format!("Audio Files/{stem}.wav");
    let mut n = 2u32;
    while s.sources.iter().any(|src| src.path == path) {
        path = format!("Audio Files/{stem}-{n}.wav");
        n = n.saturating_add(1);
        if n > 10_000 {
            break;
        }
    }
    path
}

/// Register decoded audio as a new source. Resamples to the session rate if needed.
pub fn add_source(s: &mut Session, name: &str, mut buf: AudioBuffer, path: Option<&str>, format: FileFormat) -> SourceId {
    let sr = s.sample_rate.hz();
    if buf.sample_rate != sr && buf.sample_rate > 0 {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, sr);
        buf.sample_rate = sr;
    }
    let id = SourceId(s.alloc());
    let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).unwrap_or(name).to_string();
    let stored = match path {
        Some(p) => p.to_string(),
        None => unique_audio_rel(s, &stem),
    };
    s.sources.push(Source {
        id,
        name: stem.clone(),
        path: stored,
        channels: u16::try_from(buf.num_channels()).unwrap_or(u16::MAX),
        frames: buf.frames() as u64,
        sample_rate: sr,
        format,
        time_reference: 0,
        unsaved: true,
    });
    s.pool.insert(id, Arc::new(SourceAudio::new(buf)));
    id
}

/// Decode and place an audio file. With no target track, a new track of matching width is made.
pub fn import_audio_bytes(e: &mut Engine, name: &str, bytes: &[u8], path: Option<&str>, target: Option<TrackId>, at: Samples) -> Result<Value> {
    let ext = Path::new(name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let (info, buf) = soundcraft_audio_io::decode(bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    if buf.frames().saturating_mul(buf.num_channels().max(1)) > MAX_IMPORT_SAMPLES {
        return Err(EngineError::Io(format!("{name}: file is too large to import")));
    }
    if buf.frames() == 0 {
        return Err(EngineError::Io(format!("{name}: no audio")));
    }
    // SCULPT_CORE23_IMPORT_RATE_NOTICE: sources are resampled by add_source.
    if buf.sample_rate > 0 && buf.sample_rate != e.session().sample_rate.hz() {
        e.message(format!("{}: imported at {} Hz, resampled to session {} Hz", name, buf.sample_rate, e.session().sample_rate.hz()));
    }
    let channels = buf.num_channels();
    let original_rate = buf.sample_rate; // SCULPT_CORE24_IMPORT_ORIGINAL_RATE
    let s = e.session_mut();
    let src = add_source(s, name, buf, path, info.format);
    if let (Some(bwf), Some(sx)) = (info.bwf.as_ref(), s.sources.last_mut()) {
        sx.time_reference = bwf.time_reference;
    }
    let frames = s.source(src).map_or(0, |x| i64::try_from(x.frames).unwrap_or(0));
    let track = match target {
        Some(t) => t,
        None => {
            let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).unwrap_or("Audio").to_string();
            s.add_track(TrackKind::Audio, ChannelFormat::for_channels(channels), Some(&stem))
        }
    };
    let cid = s.new_clip_id();
    let stem = s.source(src).map(|x| x.name.clone()).unwrap_or_default();
    crate::edit::place_clip(s, track, Clip::audio(cid, stem, src, 0, at, frames));
    Ok(json!({"track": track, "clip": cid, "source": src, "frames": frames, "channels": channels,
              "source_sample_rate":original_rate,"session_sample_rate":s.sample_rate.hz()})) // SCULPT_CORE24_IMPORT_RESULT_RATE
}

/// Import a movie: a Video track clip spanning the picture (the movie is referenced by `path`,
/// never copied) plus its audio on a new audio track when it has a usable audio stream. Fails
/// only when the file has neither a readable picture nor audio.
pub fn import_video_bytes(e: &mut Engine, path: &str, bytes: Vec<u8>, at: Samples) -> Result<Value> {
    let stem = Path::new(path).file_stem().and_then(|n| n.to_str()).unwrap_or("Video").to_string();
    let ext = Path::new(path).extension().and_then(|x| x.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    let bytes: Arc<[u8]> = bytes.into();
    let movie = soundcraft_video::Movie::open(Arc::clone(&bytes)).map(|m| m.info().clone());
    let audio_name = format!("{stem} audio.{}", if ext == "mov" { "mp4" } else { ext.as_str() });
    let audio = import_audio_bytes(e, &audio_name, &bytes, Some(path), None, at);
    let info = match (movie, &audio) {
        (Ok(info), _) => info,
        (Err(verr), Ok(a)) => {
            e.message(format!("{stem}: imported the audio; no picture ({verr})"));
            return Ok(json!({"audio": a, "video": Value::Null, "video_error": verr.to_string()}));
        }
        (Err(verr), Err(aerr)) => return Err(EngineError::Io(format!("{path}: {verr}; no usable audio stream ({aerr})"))),
    };
    let abs = std::fs::canonicalize(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned());
    let s = e.session_mut();
    let sr = s.sample_rate.as_f64();
    let len = (info.duration * sr).round();
    let len = if len.is_finite() { (len as Samples).clamp(1, sr as Samples * 3600 * 48) } else { 1 };
    let vid = SourceId(s.alloc());
    s.videos.push(soundcraft_model::VideoSource {
        id: vid,
        name: Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("movie").to_string(),
        path: abs,
        width: info.width,
        height: info.height,
        frame_rate: info.frame_rate,
        duration: info.duration,
        codec: info.codec.clone(),
    });
    // Reuse the first Video track when the new clip does not overlap its clips; else add one at the top.
    let range = Range::new(at, at.saturating_add(len));
    let track = match s.tracks.iter().find(|t| t.kind == TrackKind::Video) {
        Some(t) if !t.clips().iter().any(|c| c.range().overlaps(&range)) => t.id,
        _ => {
            let id = s.add_track(TrackKind::Video, ChannelFormat::Mono, Some(&stem));
            if let Some(i) = s.track_index(id) {
                let t = s.tracks.remove(i);
                s.tracks.insert(0, t);
            }
            id
        }
    };
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, track, Clip::video(cid, stem.clone(), vid, 0, at, len));
    if !info.decodable {
        e.message(format!("{stem}: {} pictures cannot be shown yet; the Video track shows the clip only", info.codec_detail));
    }
    if let Err(err) = &audio {
        e.message(format!("{stem}: no usable audio stream ({err})"));
    }
    Ok(json!({
        "video": {"track": track, "clip": cid, "source": vid, "length": len, "info": info},
        "audio": audio.as_ref().ok(),
        "audio_error": audio.as_ref().err().map(ToString::to_string),
    }))
}

/// Resolve each referenced movie: keep it when its path exists, else look for a file of the same
/// name next to the session (moved projects) and relink. Returns the movies still missing.
pub fn relink_videos(s: &mut Session, session_dir: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    for v in &mut s.videos {
        let p = if Path::new(&v.path).is_absolute() { PathBuf::from(&v.path) } else { session_dir.join(&v.path) };
        if p.is_file() {
            continue;
        }
        let name = Path::new(&v.path).file_name().map(PathBuf::from).unwrap_or_default();
        let near =
            [session_dir.join(&name), session_dir.join("Video Files").join(&name)].into_iter().find(|c| !name.as_os_str().is_empty() && c.is_file());
        match near {
            Some(c) => v.path = c.to_string_lossy().into_owned(),
            None => missing.push(format!("{} (video)", p.display())),
        }
    }
    missing
}

/// Import a Standard MIDI File as MIDI tracks (one per SMF track).
pub fn import_midi_bytes(e: &mut Engine, bytes: &[u8], at: Samples, tempo_map: bool) -> Result<Vec<TrackId>> {
    let smf = soundcraft_midi::read_smf(bytes).map_err(|err| EngineError::Io(err.to_string()))?;
    let s = e.session_mut();
    if tempo_map {
        for (tick, bpm) in &smf.tempos {
            let _ = s.tempo.set_tempo(*tick, *bpm);
        }
        for (tick, n, d) in &smf.meters {
            let _ = s.tempo.set_meter(*tick, u32::from(*n), u32::from(*d));
        }
    }
    let base_tick = s.tempo.samples_to_ticks(at, s.sample_rate);
    let mut out = Vec::new();
    for (i, tr) in smf.tracks.iter().enumerate() {
        if tr.sequence.notes.is_empty() {
            continue;
        }
        let name = if tr.name.trim().is_empty() { format!("MIDI {}", i + 1) } else { tr.name.clone() };
        let id = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&name));
        let drums = tr.sequence.notes.iter().all(|n| n.channel == 9);
        if let Some(t) = s.track_mut(id) {
            t.instrument = Some(soundcraft_model::Insert::new(if drums { "drum_synth" } else { "subtractive_synth" }));
        }
        let end_tick = base_tick + tr.sequence.end_tick().max(1);
        let len = s.tempo.tick_to_samples(end_tick, s.sample_rate) - at;
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, id, Clip::midi(cid, name, at, len.max(1), tr.sequence.clone()));
        out.push(id);
    }
    Ok(out)
}

/// Export MIDI clips as a format-1 SMF. `only` limits the file to those tracks.
pub fn export_midi(e: &Engine, only: Option<&[TrackId]>) -> Result<Vec<u8>> {
    let s = e.session();
    let mut smf = soundcraft_midi::Smf {
        format: 1,
        tracks: Vec::new(),
        tempos: s.tempo.tempos().iter().map(|t| (t.tick, t.bpm)).collect(),
        meters: s.tempo.meters().iter().map(|m| (m.tick, u8::try_from(m.numerator).unwrap_or(4), u8::try_from(m.denominator).unwrap_or(4))).collect(),
        markers: s.markers.iter().map(|m| (s.tempo.samples_to_ticks(m.start, s.sample_rate), m.name.clone())).collect(),
        key_sigs: Vec::new(),
    };
    let chosen: Vec<_> = s.tracks.iter().filter(|t| t.kind.is_midi() && only.is_none_or(|ids| ids.contains(&t.id))).collect();
    if only.is_some() && chosen.is_empty() {
        return Err(EngineError::BadParams("file.export_midi".into(), "none of the requested tracks hold MIDI".into()));
    }
    for t in chosen {
        let mut seq = soundcraft_midi::Sequence::default();
        for c in t.clips() {
            if let ClipContent::Midi { sequence } = &c.content {
                let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
                for n in &sequence.notes {
                    let mut n = *n;
                    n.start = n.start.saturating_add(base);
                    seq.notes.push(n);
                }
                for ev in &sequence.ctrls {
                    let mut ev = *ev;
                    ev.tick = ev.tick.saturating_add(base);
                    seq.ctrls.push(ev);
                }
            }
        }
        seq.sort();
        smf.tracks.push(soundcraft_midi::SmfTrack { name: t.name.clone(), sequence: seq });
    }
    soundcraft_midi::write_smf(&smf).map_err(|err| EngineError::Io(err.to_string()))
}

/// Render the main mix. Returns the encoded file and (peak dBFS, integrated LUFS).
pub fn bounce_bytes(e: &Engine, r: Range, opts: &EncodeOptions, normalize: bool) -> Result<(Vec<u8>, (f32, f32, f32))> {
    bounce_bytes_with(e, r, opts, normalize, false)
}

/// Render the main mix in its own format (every main channel, SMPTE/WAV order, with the WAV
/// speaker mask of the main format), or an ITU stereo fold-down when `fold_stereo`.
pub fn bounce_bytes_with(e: &Engine, r: Range, opts: &EncodeOptions, normalize: bool, fold_stereo: bool) -> Result<(Vec<u8>, (f32, f32, f32))> {
    bounce_source(e, r, opts, normalize, fold_stereo, None)
}

/// [`bounce_bytes_with`] for the main mix, or for `bus` when one is named.
pub fn bounce_source(
    e: &Engine,
    r: Range,
    opts: &EncodeOptions,
    normalize: bool,
    fold_stereo: bool,
    bus: Option<BusId>,
) -> Result<(Vec<u8>, (f32, f32, f32))> {
    let s = e.session();
    if r.len() > s.sample_rate.samples(4.0 * 3600.0) {
        return Err(EngineError::BadParams("file.bounce_mix".into(), "bounces are limited to 4 hours".into()));
    }
    let (mut ch, fmt) = render_bounce(s, r, fold_stereo, bus)?;
    if normalize {
        soundcraft_dsp::offline::normalize(&mut ch, -0.1, false);
    }
    let peak = soundcraft_dsp::offline::peak_db(&ch);
    let mut lm = soundcraft_dsp::meter::LoudnessMeter::new(s.sample_rate.as_f64() as f32, ch.len());
    let frames = ch.first().map_or(0, Vec::len);
    lm.process(&ch, frames);
    let lufs = lm.integrated_lufs();
    let true_peak = lm.true_peak_db();
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: ch };
    let bytes = soundcraft_audio_io::encode_with_channel_mask(&buf, opts, fmt.channel_mask()).map_err(|err| EngineError::Io(err.to_string()))?;
    Ok((bytes, (peak, lufs, true_peak)))
}

/// The main mix over `r` and its format (folded to stereo on request).
fn render_main(s: &Session, r: Range, fold_stereo: bool) -> Result<(Vec<Vec<f32>>, ChannelFormat)> {
    render_bounce(s, r, fold_stereo, None)
}

/// The main mix, or one bus, over `r`.
fn render_bounce(s: &Session, r: Range, fold_stereo: bool, bus: Option<BusId>) -> Result<(Vec<Vec<f32>>, ChannelFormat)> {
    let (ch, fmt) = if let Some(id) = bus {
        let fmt = s.bus(id).map(|b| b.format).unwrap_or(ChannelFormat::Stereo);
        (soundcraft_mix::render_bus_checked(s, id, r, 1024).map_err(EngineError::Io)?, fmt)
    } else {
        let ch = soundcraft_mix::render_range_checked(s, r, 1024).map_err(EngineError::Io)?;
        let fmt = if ch.len() <= 2 { ChannelFormat::Stereo } else { s.main_format() };
        (ch, fmt)
    };
    Ok(if fold_stereo && ch.len() > 2 { (soundcraft_mix::fold_down_stereo(fmt, &ch), ChannelFormat::Stereo) } else { (ch, fmt) })
}

/// Bounce one file per audio/instrument track (each soloed, so its sends and auxes are included)
/// into `dir`. Returns the written paths.
pub fn bounce_stems(e: &Engine, dir: &str, r: Range, opts: &EncodeOptions) -> Result<Vec<String>> {
    bounce_stems_with(e, dir, r, opts, false)
}

/// [`bounce_stems`] in the main format, or folded down to stereo when `fold_stereo`.
pub fn bounce_stems_with(e: &Engine, dir: &str, r: Range, opts: &EncodeOptions, fold_stereo: bool) -> Result<Vec<String>> {
    let s = e.session();
    std::fs::create_dir_all(dir).map_err(|err| EngineError::Io(format!("{dir}: {err}")))?;
    let ext = match opts.format {
        FileFormat::Aiff => "aif",
        FileFormat::Flac => "flac",
        _ => "wav",
    };
    let mut out = Vec::new();
    let stems: Vec<(TrackId, String)> = s
        .tracks
        .iter()
        .filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi) && !t.inactive && !t.mixer.mute)
        .map(|t| (t.id, t.name.clone()))
        .collect();
    for (id, name) in stems {
        let mut solo = s.clone();
        for t in &mut solo.tracks {
            t.mixer.solo = t.id == id;
            if t.id == id {
                // SCULPT_CORE24_STEM_MONITOR: a hardware-only route would render silence
                // in an offline Main bounce. Preserve the original session unchanged.
                t.mixer.output = soundcraft_model::Route::Main;
            }
        }
        let (ch, fmt) = render_main(&solo, r, fold_stereo)?;
        let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: ch };
        let bytes = soundcraft_audio_io::encode_with_channel_mask(&buf, opts, fmt.channel_mask()).map_err(|err| EngineError::Io(err.to_string()))?;
        let path = Path::new(dir).join(format!("{} - {}.{ext}", sanitize_name(&s.name), sanitize_name(&name)));
        std::fs::write(&path, bytes).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))?;
        out.push(path.to_string_lossy().into_owned());
    }
    Ok(out)
}

/// Limit a range used for rendering to the track's material (plus a second), so a huge
/// selection never renders hours of silence.
pub fn bounded_to_track(s: &Session, t: TrackId, r: Range) -> Range {
    let Some(tr) = s.track(t) else { return Range::point(r.start) };
    let end = tr.clips().iter().map(Clip::end).max().unwrap_or(0).saturating_add(s.sample_rate.samples(1.0));
    let start = r.start.max(0).min(end);
    Range::new(start, r.end.min(end).max(start))
}

/// Transient positions (absolute) in a track's clips within `r`.
pub fn transients_in(s: &Session, t: TrackId, r: Range, sens: f32) -> Result<Vec<Samples>> {
    let r = if r.is_empty() { Range::new(0, s.content_end()) } else { r };
    let r = bounded_to_track(s, t, r);
    let audio = soundcraft_mix::render_clips_checked(s, t, r).map_err(EngineError::Io)?;
    Ok(soundcraft_dsp::offline::detect_transients(&audio, s.sample_rate.as_f64() as f32, sens).into_iter().map(|i| r.start + i as i64).collect())
}

/// Strip Silence: keep only the non-silent ranges of clips inside `r`.
pub fn strip_silence(s: &mut Session, t: TrackId, r: Range, thr: f32, min_len: Samples, pre: Samples, post: Samples) -> Result<usize> {
    let r = bounded_to_track(s, t, r);
    let audio = soundcraft_mix::render_clips_checked(s, t, r).map_err(EngineError::Io)?;
    let keep = soundcraft_dsp::offline::non_silent_ranges(
        &audio,
        thr,
        usize::try_from(min_len).unwrap_or(0),
        usize::try_from(pre).unwrap_or(0),
        usize::try_from(post).unwrap_or(0),
    );
    let mut cut_points = vec![r.start];
    for (a, b) in &keep {
        cut_points.push(r.start + *a as i64);
        cut_points.push(r.start + *b as i64);
    }
    cut_points.push(r.end);
    for p in &cut_points {
        crate::edit::separate_at(s, t, *p);
    }
    // Remove pieces that fall in the gaps.
    let mut gaps = Vec::new();
    let mut prev = r.start;
    for (a, b) in &keep {
        let a = r.start + *a as i64;
        if a > prev {
            gaps.push(Range::new(prev, a));
        }
        prev = r.start + *b as i64;
    }
    if prev < r.end {
        gaps.push(Range::new(prev, r.end));
    }
    if let Some(pl) = s.track_mut(t).and_then(|x| x.playlist_mut()) {
        pl.clips.retain(|c| !gaps.iter().any(|g| c.start >= g.start && c.end() <= g.end));
    }
    Ok(keep.len())
}

/// Consolidate the selection on a track into one new clip backed by a rendered source.
pub fn consolidate(e: &mut Engine, t: TrackId, r: Range) -> Result<bool> {
    let s = e.session();
    if s.track(t).is_none_or(|tr| !tr.kind.has_playlist() || tr.kind.is_midi()) {
        return Ok(false);
    }
    let r = bounded_to_track(s, t, r);
    if r.is_empty() {
        return Ok(false);
    }
    let audio = soundcraft_mix::render_clips_checked(s, t, r).map_err(EngineError::Io)?;
    let name = format!("{}_consolidated", s.track(t).map_or("Audio", |x| x.name.as_str()));
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &name, buf, None, FileFormat::Wav);
    crate::edit::clear_range(s, t, r, false);
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::audio(cid, name, src, 0, r.start, r.len()));
    Ok(true)
}

/// Render clip gain into a new source and reset the clip's gain.
pub fn render_clip_gain(e: &mut Engine, id: soundcraft_model::ClipId) -> Result<bool> {
    let s = e.session();
    let Some((t, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { return Ok(false) };
    if !c.is_audio() {
        return Ok(false);
    }
    let mut c2 = c.clone();
    c2.fade_in = Default::default();
    c2.fade_out = Default::default();
    let mut tmp = s.clone();
    if let Some(pl) = tmp.track_mut(t).and_then(|x| x.playlist_mut()) {
        pl.clips = vec![c2];
    }
    let audio = soundcraft_mix::render_clips_checked(&tmp, t, c.range()).map_err(EngineError::Io)?;
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &format!("{}_gain", c.name), buf, None, FileFormat::Wav);
    if let Some(cl) = s.find_clip_mut(id) {
        cl.content = ClipContent::Audio { source: src, offset: 0 };
        cl.gain_db = 0.0;
        cl.gain_env.clear();
        // The new source is the clip as it already sounds, stretch included.
        // Keeping the old ratio would play that audio through the stretch again.
        cl.stretch = 1.0;
    }
    Ok(true)
}

/// Commit a track's inserts into a new audio track (Track › Commit).
pub fn commit_track(e: &mut Engine, t: TrackId) -> Result<Option<TrackId>> {
    let s = e.session();
    let Some(tr) = s.track(t) else { return Ok(None) };
    if !tr.kind.is_audio_path() || tr.kind == TrackKind::Master {
        return Ok(None);
    }
    let end = tr.clips().iter().map(Clip::end).max().unwrap_or(0);
    if end <= 0 {
        return Ok(None);
    }
    let range = Range::new(0, end + s.sample_rate.samples(2.0));
    let audio = soundcraft_mix::render_track_pre_fader_checked(s, t, range).map_err(EngineError::Io)?;
    let name = format!("{}.cm", tr.name);
    let fmt = ChannelFormat::for_channels(audio.len());
    let mixer = tr.mixer.clone();
    let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    let s = e.session_mut();
    let src = add_source(s, &name, buf, None, FileFormat::Wav);
    let nt = s.add_track(TrackKind::Audio, fmt, Some(&name));
    if let (Some(idx), Some(new_idx)) = (s.track_index(t), s.track_index(nt)) {
        let tr = s.tracks.remove(new_idx);
        s.tracks.insert((idx + 1).min(s.tracks.len()), tr);
    }
    let cid = s.new_clip_id();
    if let Some(ntr) = s.track_mut(nt) {
        ntr.mixer.volume_db = mixer.volume_db;
        ntr.mixer.output = mixer.output.clone();
        ntr.mixer.sends = mixer.sends.clone();
        if let Some(pl) = ntr.playlist_mut() {
            pl.clips.push(Clip::audio(cid, name, src, 0, 0, range.len()));
        }
    }
    if let Some(old) = s.track_mut(t) {
        old.inactive = true;
        old.hidden = true;
    }
    Ok(Some(nt))
}

pub fn export_clips(e: &Engine, ids: &[soundcraft_model::ClipId], dir: &str) -> Result<usize> {
    let s = e.session();
    std::fs::create_dir_all(dir).map_err(|err| EngineError::Io(format!("{dir}: {err}")))?;
    let mut n = 0;
    for id in ids {
        let Some((t, c)) = s.find_clip(*id) else { continue };
        let audio = soundcraft_mix::render_clips_checked(s, t, c.range()).map_err(EngineError::Io)?;
        let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
        let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).map_err(|err| EngineError::Io(err.to_string()))?;
        let stem = sanitize_name(&c.name);
        let mut path = Path::new(dir).join(format!("{stem}.wav"));
        let mut k = 2u32;
        while path.exists() {
            path = Path::new(dir).join(format!("{stem}-{k}.wav"));
            k = k.saturating_add(1);
            if k > 10_000 {
                return Err(EngineError::Io(format!("{dir}: too many files named {stem}")));
            }
        }
        std::fs::write(&path, bytes).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))?;
        n += 1;
    }
    Ok(n)
}

pub fn sanitize_name(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') { c } else { '_' }).collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "untitled".into() } else { s.chars().take(100).collect() }
}

/// Save the session to `path` (a `.scraft` file). Audio that only exists in memory is written
/// as 32-bit float WAV into `Audio Files/` next to it. Returns how many audio files were written.
///
/// In the browser there are no folders: the session and all its audio go out as one bundle file
/// (`<name>.scraftz`, see [`session_bundle`]) through the host's delivery hook ([`set_deliver`]).
pub fn save_session(e: &mut Engine, path: &str) -> Result<usize> {
    if cfg!(target_arch = "wasm32") {
        return save_session_bundle(e, path);
    }
    let (fresh, written, p) = write_session(e.session(), path, false)?;
    // Keep the in-memory document in sync (paths, unsaved flags) without an undo step.
    *e.session_mut() = fresh;
    e.path = Some(p.to_string_lossy().into_owned());
    e.mark_clean();
    Ok(written)
}

/// Write a copy of the session (and any audio it needs) without touching the open document —
/// used for autosave / crash recovery. Audio already written for a source is reused.
pub fn save_copy(e: &Engine, path: &str) -> Result<usize> {
    write_session(e.session(), path, true).map(|(_, n, _)| n)
}

/// File › Save Session Copy In: a normal session folder that does not rename, retarget or
/// clean the session that stays open.
pub fn save_session_copy(e: &Engine, path: &str) -> Result<usize> {
    write_session(e.session(), path, false).map(|(_, n, _)| n)
}

/// Write `session` to `path`; returns the session with updated media paths, the number of audio
/// files written and the final path.
fn write_session(session: &Session, path: &str, overwrite: bool) -> Result<(Session, usize, PathBuf)> {
    let p = session_file(path);
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let audio_dir = dir.join("Audio Files");
    // SCULPT_CORE17_AUDIO_FOLDER: project layout exists from first save.
    std::fs::create_dir_all(&audio_dir).map_err(|err| EngineError::Io(format!("{}: {err}", audio_dir.display())))?;
    let mut s = session.clone();
    let from = s.folder.clone();
    let same_dir = from.as_deref().is_some_and(|f| same_path(f, &dir));
    let mut written = 0;
    let shared_paths: Vec<bool> =
        s.sources.iter().enumerate().map(|(i, src)| s.sources.iter().enumerate().any(|(j, other)| j != i && other.path == src.path)).collect();
    for (i, src) in s.sources.iter_mut().enumerate() {
        if !same_dir
            && s.pool.get(src.id).is_none()
            && Path::new(&src.path).is_relative()
            && let Some(f) = &from
        {
            src.path = f.join(&src.path).to_string_lossy().into_owned();
        }
        let exists_rel = same_dir && !Path::new(&src.path).is_absolute() && dir.join(&src.path).exists();
        let shared = shared_paths.get(i).copied().unwrap_or(false);
        if exists_rel && !src.unsaved && !shared {
            continue;
        }
        let in_folder = Path::new(&src.path).strip_prefix(&audio_dir).ok().and_then(Path::to_str).map(str::to_string);
        if let Some(f) = in_folder.filter(|_| !src.unsaved && Path::new(&src.path).is_file()) {
            src.path = format!("Audio Files/{f}");
            continue;
        }
        let Some(audio) = s.pool.get(src.id) else { continue };
        std::fs::create_dir_all(&audio_dir).map_err(|err| EngineError::Io(format!("{}: {err}", audio_dir.display())))?;
        let mut stem = sanitize_name(&src.name);
        if overwrite {
            // One file per source id so autosaves never collide or pile up.
            stem = format!("{stem}_{}", src.id.0);
            if audio_dir.join(format!("{stem}.wav")).exists() {
                src.path = format!("Audio Files/{stem}.wav");
                src.unsaved = false;
                continue;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        let (name, new) = store_source_wav(&audio_dir, &stem, audio)?;
        #[cfg(target_arch = "wasm32")]
        let (name, new) = {
            let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: soundcraft_audio_io::BitDepth::Float32, dither: false, bwf: None };
            let bytes = soundcraft_audio_io::encode(&audio.buffer, &opts).map_err(|err| EngineError::Io(err.to_string()))?;
            store_wav(&audio_dir, &stem, &bytes)?
        };
        src.path = format!("Audio Files/{name}");
        src.unsaved = false;
        src.format = FileFormat::Wav;
        written += usize::from(new);
    }
    if written > 0 {
        sync_dir(&audio_dir);
    }
    s.folder = Some(dir.clone());
    if !overwrite && let Some(stem) = p.file_stem().and_then(|x| x.to_str()) {
        s.name = stem.to_string();
    }
    let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|err| EngineError::Io(format!("{}: {err}", dir.display())))?;
    // Atomic-ish write: temp file then rename.
    let tmp = p.with_extension("scraft.tmp");
    if !overwrite
        && p.try_exists().map_err(|error| EngineError::Io(format!("{}: {error}", p.display())))?
        && !file_matches_bytes(&p, text.as_bytes()).map_err(|error| EngineError::Io(format!("{}: {error}", p.display())))?
    {
        back_up(&p)?;
    }
    replace_file(&tmp, &p, text.as_bytes())?;
    Ok((s, written, p))
}

/// Write `bytes` to `tmp`, flush it to disk and rename it over `file`, so `file` holds either the
/// old or the new contents, never a mix.
pub fn replace_file(tmp: &Path, file: &Path, bytes: &[u8]) -> Result<()> {
    if let Err(err) = write_synced(tmp, bytes).and_then(|()| std::fs::rename(tmp, file)) {
        let _ = std::fs::remove_file(tmp);
        return Err(EngineError::Io(format!("{}: {err}", file.display())));
    }
    sync_dir(file.parent().unwrap_or(Path::new("")));
    Ok(())
}

fn sync_dir(dir: &Path) {
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Copy the session file about to be replaced to `Backups/<name> backup <n>.<ext>`, keeping the
/// newest ten.
/// Compare without retaining a second whole project JSON in RAM.
fn file_matches_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() != bytes.len() as u64 {
        return Ok(false);
    }
    let mut buffer = [0u8; 8192];
    for expected in bytes.chunks(buffer.len()) {
        let actual = buffer.get_mut(..expected.len()).ok_or_else(|| std::io::Error::other("project comparison buffer bounds"))?;
        file.read_exact(actual)?;
        if actual != expected {
            return Ok(false);
        }
    }
    let mut extra = [0u8; 1];
    Ok(file.read(&mut extra)? == 0)
}

/// Atomically reserve a free name, never following or replacing an existing destination.
/// Source copying and sync finish before the caller can replace the current session file.
fn copy_exclusive_backup(file: &Path, dir: &Path, prefix: &str, suffix: &str, mut next: u64) -> Result<(u64, PathBuf)> {
    let mut source = std::fs::File::open(file).map_err(|error| EngineError::Io(format!("backup {}: {error}", file.display())))?;
    let before = source.metadata().map_err(|error| EngineError::Io(error.to_string()))?;
    for _ in 0..1024 {
        let destination = dir.join(format!("{prefix}{next}{suffix}"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&destination) {
            Ok(mut target) => {
                let result = (|| -> std::io::Result<()> {
                    let copied = std::io::copy(&mut source, &mut target)?;
                    let after = source.metadata()?;
                    if copied != before.len() || after.len() != before.len() || before.modified().ok() != after.modified().ok() {
                        return Err(std::io::Error::other("project changed during backup"));
                    }
                    target.set_permissions(before.permissions())?;
                    target.sync_all()
                })();
                drop(target);
                if let Err(error) = result {
                    if let Err(cleanup) = std::fs::remove_file(&destination) {
                        log::warn!("could not remove incomplete backup {}: {cleanup}", destination.display());
                    }
                    return Err(EngineError::Io(format!("backup {}: {error}", destination.display())));
                }
                sync_dir(dir);
                return Ok((next, destination));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                next = next.checked_add(1).ok_or_else(|| EngineError::Io("backup counter exhausted; existing backups were preserved".into()))?;
            }
            Err(error) => return Err(EngineError::Io(format!("backup {}: {error}", destination.display()))),
        }
    }
    Err(EngineError::Io("too many concurrent backup name collisions; existing backups were preserved".into()))
}

fn back_up(file: &Path) -> Result<PathBuf> {
    let dir = file.parent().ok_or_else(|| EngineError::Io("backup needs a project directory".into()))?;
    let stem = file.file_stem().and_then(|name| name.to_str()).ok_or_else(|| EngineError::Io("invalid backup project name".into()))?;
    let backups = dir.join("Backups");
    std::fs::create_dir_all(&backups).map_err(|error| EngineError::Io(format!("{}: {error}", backups.display())))?;
    let prefix = format!("{stem} backup ");
    let suffix = file.extension().and_then(|extension| extension.to_str()).map_or_else(String::new, |extension| format!(".{extension}"));
    let mut numbers = Vec::<u64>::new();
    for entry in std::fs::read_dir(&backups).map_err(|error| EngineError::Io(error.to_string()))? {
        let entry = entry.map_err(|error| EngineError::Io(error.to_string()))?;
        let name = entry.file_name();
        let Some(number) = name.to_str().and_then(|name| name.strip_prefix(&prefix)).and_then(|name| name.strip_suffix(&suffix)) else { continue };
        let Ok(value) = number.parse::<u64>() else { continue };
        if number != value.to_string() || !entry.file_type().map_err(|error| EngineError::Io(error.to_string()))?.is_file() {
            continue;
        }
        if numbers.len() >= 10_000 {
            return Err(EngineError::Io("too many numbered project backups".into()));
        }
        numbers.push(value);
    }
    let next = match numbers.iter().max() {
        Some(number) => number.checked_add(1).ok_or_else(|| EngineError::Io("backup counter exhausted; existing backups were preserved".into()))?,
        None => 1,
    };
    let (number, destination) = copy_exclusive_backup(file, &backups, &prefix, &suffix, next)?;
    numbers.push(number);
    numbers.sort_unstable();
    numbers.dedup();
    for number in numbers.iter().take(numbers.len().saturating_sub(10)) {
        let old = backups.join(format!("{prefix}{number}{suffix}"));
        if let Err(error) = std::fs::remove_file(&old) {
            log::warn!("could not prune backup {}: {error}", old.display());
        }
    }
    Ok(destination)
}

/// The file a session saved to `path` goes to: `path`, with `.scraft` added when it has no
/// extension and isn't an existing file.
pub fn session_file(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    // SCULPT_CORE16_SESSION_LOOKUP: first try the native extension, then legacy.
    if p.extension().is_none() && !p.is_file() {
        let native = p.with_extension(soundcraft_model::SESSION_EXTENSION);
        if native.exists() {
            return native;
        }
        let legacy = p.with_extension("scraft");
        if legacy.exists() {
            return legacy;
        }
        return native;
    }
    p
}

/// SCULPT_CORE17_PROJECT_FOLDER: a new native project always gets its own folder.
/// Existing sessions keep their exact paths, including legacy .scraft files.
pub fn sculpt_project_file(path: &str) -> PathBuf {
    let file = session_file(path);
    if file.exists() || file.extension().and_then(|x| x.to_str()) != Some(soundcraft_model::SESSION_EXTENSION) {
        return file;
    }
    let Some(stem) = file.file_stem().and_then(|x| x.to_str()) else { return file };
    if file.parent().and_then(Path::file_name).and_then(|x| x.to_str()) == Some(stem) {
        return file;
    }
    let Some(filename) = file.file_name() else { return file };
    file.with_file_name(stem).join(filename)
}

/// True when `command` would write a session over the existing `file`. Save, Save As and Save
/// Copy In may write the open session's own file; other commands never ask.
pub fn replaces(e: &Engine, command: &str, file: &Path) -> bool {
    match command {
        "session.save" | "session.save_as" | "session.save_copy" => {
            file.exists() && !e.path.as_deref().is_some_and(|own| same_path(Path::new(own), file))
        }
        "session.save_template" | "file.export_selected_tracks_as_session" => file.exists(),
        _ => false,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let full = |p: &Path| if p.as_os_str().is_empty() { Path::new(".").canonicalize() } else { p.canonicalize() };
    a == b || matches!((full(a), full(b)), (Ok(x), Ok(y)) if x == y)
}

/// Store `bytes` as the first of `<stem>.wav`, `<stem>_01.wav`, … in `dir` that is free or already
/// holds exactly these bytes, so saving never replaces audio; returns the name and whether it was
/// written.
/// Compare generated WAV chunks with an existing file without retaining either whole file.
#[cfg(not(target_arch = "wasm32"))]
struct AudioFileComparison {
    file: std::fs::File,
    matches: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::io::Write for AudioFileComparison {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        use std::io::Read;
        if self.matches {
            let mut scratch = [0u8; 8192];
            for expected in bytes.chunks(scratch.len()) {
                let actual = scratch.get_mut(..expected.len()).ok_or_else(|| std::io::Error::other("audio comparison bounds"))?;
                match self.file.read_exact(actual) {
                    Ok(()) => self.matches &= actual == expected,
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => self.matches = false,
                    Err(e) => return Err(e),
                }
                if !self.matches {
                    break;
                }
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn store_source_wav(dir: &Path, stem: &str, audio: &SourceAudio) -> Result<(String, bool)> {
    use std::io::Read;
    for k in 0..10_000u32 {
        let name = if k == 0 { format!("{stem}.wav") } else { format!("{stem}_{k:02}.wav") };
        let path = dir.join(&name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let result = audio
                    .write_wave(&mut file, |_, _| true)
                    .map(|_| ())
                    .map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))
                    .and_then(|_| file.sync_all().map_err(|e| EngineError::Io(format!("{}: {e}", path.display()))));
                if let Err(error) = result {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(error);
                }
                return Ok((name, true));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Directories and links are never reused as managed source files.
                if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
                    continue;
                }
                let file = std::fs::File::open(&path).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
                let mut comparison = audio
                    .write_wave(AudioFileComparison { file, matches: true }, |_, _| true)
                    .map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
                let mut extra = [0u8; 1];
                if comparison.matches && comparison.file.read(&mut extra).map_err(|e| EngineError::Io(e.to_string()))? == 0 {
                    return Ok((name, false));
                }
            }
            Err(e) => return Err(EngineError::Io(format!("{}: {e}", path.display()))),
        }
    }
    Err(EngineError::Io(format!("{}: no free file name for {stem}", dir.display())))
}

#[cfg(target_arch = "wasm32")]
fn store_wav(dir: &Path, stem: &str, bytes: &[u8]) -> Result<(String, bool)> {
    use std::io::Write;
    for k in 0..10_000u32 {
        let name = if k == 0 { format!("{stem}.wav") } else { format!("{stem}_{k:02}.wav") };
        let path = dir.join(&name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(err) = f.write_all(bytes).and_then(|()| f.sync_all()) {
                    let _ = std::fs::remove_file(&path);
                    return Err(EngineError::Io(format!("{}: {err}", path.display())));
                }
                return Ok((name, true));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) && std::fs::read(&path).is_ok_and(|b| b == bytes) {
                    return Ok((name, false));
                }
            }
            Err(err) => return Err(EngineError::Io(format!("{}: {err}", path.display()))),
        }
    }
    Err(EngineError::Io(format!("{}: no free file name for {stem}", dir.display())))
}

/// Open a `.scraft` session and decode its audio files (missing files are reported, not fatal).
pub fn open_session(e: &mut Engine, path: &str) -> Result<Vec<String>> {
    // SCULPT_CORE_SESSION_OPEN_PATH_V1: same path resolver used by Save As.
    let path = session_file(path);
    let text = std::fs::read_to_string(&path).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))?;
    let mut s = Session::from_json(&text).map_err(|err| EngineError::Io(err.to_string()))?;
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut missing = Vec::new();
    for src in s.sources.clone() {
        let p = if Path::new(&src.path).is_absolute() { PathBuf::from(&src.path) } else { dir.join(&src.path) };
        match std::fs::read(&p) {
            Ok(bytes) => {
                if let Err(err) = load_source_audio(&mut s, src.id, &bytes, p.extension().and_then(|x| x.to_str())) {
                    missing.push(format!("{}: {err}", p.display()));
                }
            }
            Err(_) => missing.push(p.display().to_string()),
        }
    }
    missing.extend(relink_videos(&mut s, &dir));
    s.folder = Some(dir);
    e.replace_session(s);
    e.path = Some(path.to_string_lossy().into_owned());
    for m in &missing {
        e.message(format!("missing media: {m}"));
    }
    Ok(missing)
}

/// Decode one source's audio file into the session pool, at the session rate.
fn load_source_audio(s: &mut Session, id: SourceId, bytes: &[u8], ext: Option<&str>) -> std::result::Result<(), String> {
    let (_, mut buf) = soundcraft_audio_io::decode(bytes, ext).map_err(|err| err.to_string())?;
    if buf.sample_rate != s.sample_rate.hz() && buf.sample_rate > 0 {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, s.sample_rate.hz());
        buf.sample_rate = s.sample_rate.hz();
    }
    s.pool.insert(id, Arc::new(SourceAudio::new(buf)));
    Ok(())
}

// ─── Hosts without a filesystem (the browser) ─────────────────────────────────────────────────

/// Extension of a session bundle: one zip file holding the `.scraft` session and its
/// `Audio Files/`, the same layout as a saved session folder. The browser has no folders, so it
/// saves and opens sessions as bundles.
pub const BUNDLE_EXTENSION: &str = "scraftz";

/// Most entries and bytes a bundle may unpack to (bundles are hostile input).
const MAX_BUNDLE_ENTRIES: usize = 10_000;
const MAX_BUNDLE_BYTES: u64 = 1 << 30;

/// Hands a finished file (name, bytes) to the person on a host without a filesystem: the web app
/// installs one that starts a download. Native builds write files and never install it.
pub type DeliverHook = fn(&str, &[u8]) -> std::result::Result<(), String>;

static DELIVER: std::sync::OnceLock<DeliverHook> = std::sync::OnceLock::new();

/// Install the delivery hook (once, at start-up of the web app).
pub fn set_deliver(h: DeliverHook) {
    let _ = DELIVER.set(h);
}

/// Write one output file: to `path` natively; in the browser, delivered under its file name.
/// Without this, every export on the web failed with «operation not supported on this platform».
pub fn write_file(path: &str, bytes: &[u8]) -> Result<()> {
    if cfg!(target_arch = "wasm32") {
        let deliver = DELIVER.get().ok_or_else(|| EngineError::Io("saving files is not available in this build".into()))?;
        let name = Path::new(path).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or("Untitled");
        return deliver(name, bytes).map_err(EngineError::Io);
    }
    std::fs::write(path, bytes).map_err(|err| EngineError::Io(format!("{path}: {err}")))
}

/// The session as a bundle: `<name>.scraft` plus every source's audio as 32-bit float WAV under
/// `Audio Files/` (stored, not compressed: audio barely compresses). Returns the bytes and the
/// session with its source paths pointing into the bundle.
pub fn session_bundle(session: &Session) -> Result<(Vec<u8>, Session)> {
    let (cursor, session) = write_session_bundle(session, std::io::Cursor::new(Vec::new()))?;
    Ok((cursor.into_inner(), session))
}

/// Stream a bundle to a seekable destination. Native callers can use a file rather than a
/// whole archive allocation. Publish the destination only after success; errors may leave a
/// partial archive. The byte-returning browser adapter remains `session_bundle`.
pub fn write_session_bundle<W: std::io::Write + std::io::Seek>(session: &Session, destination: W) -> Result<(W, Session)> {
    use std::io::Write as _;
    let zerr = |err: zip::result::ZipError| EngineError::Io(format!("bundle: {err}"));
    let ioerr = |err: std::io::Error| EngineError::Io(format!("bundle: {err}"));
    let mut s = session.clone();
    // The current reader has explicit entry/expanded-byte limits. Do not produce an archive
    // this application cannot reopen. Folder-based native projects have no bundle limit.
    if s.sources.len().saturating_add(1) > MAX_BUNDLE_ENTRIES {
        return Err(EngineError::Io("bundle: too many files".into()));
    }
    let mut expanded = 0u64;
    for source in &s.sources {
        if let Some(audio) = s.pool.get(source.id) {
            let frames = u64::try_from(audio.frames()).map_err(|_| EngineError::Io("bundle: source duration overflow".into()))?;
            let header = soundcraft_audio_io::wav_float_header(audio.channels(), audio.sample_rate(), frames)
                .map_err(|e| EngineError::Io(format!("bundle: {e}")))?;
            expanded = bundle_size_with_audio(expanded, frames, audio.channels(), header.len())?;
        }
    }
    let mut w = zip::ZipWriter::new(destination);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    let mut taken = std::collections::HashSet::new();
    for src in &mut s.sources {
        let Some(audio) = s.pool.get(src.id) else { continue };
        let base = sanitize_name(&src.name);
        let mut target = format!("Audio Files/{base}.wav");
        let mut k = 1;
        while taken.contains(&target) && k <= 9_999 {
            target = format!("Audio Files/{base}_{k:02}.wav");
            k += 1;
        }
        w.start_file(target.as_str(), opts).map_err(zerr)?;
        #[cfg(not(target_arch = "wasm32"))]
        audio.write_wave(&mut w, |_, _| true).map_err(|err| EngineError::Io(format!("bundle source {}: {err}", src.name)))?;
        #[cfg(target_arch = "wasm32")]
        {
            let opts_wav = EncodeOptions { format: FileFormat::Wav, bit_depth: soundcraft_audio_io::BitDepth::Float32, dither: false, bwf: None };
            let bytes = soundcraft_audio_io::encode(&audio.buffer, &opts_wav).map_err(|err| EngineError::Io(err.to_string()))?;
            w.write_all(&bytes).map_err(ioerr)?;
        }
        taken.insert(target.clone());
        src.path = target;
        src.unsaved = false;
        src.format = FileFormat::Wav;
    }
    let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
    if expanded.checked_add(text.len() as u64).is_none_or(|n| n > MAX_BUNDLE_BYTES) {
        return Err(EngineError::Io("bundle: too large".into()));
    }
    w.start_file(format!("{}.{}", sanitize_name(&s.name), soundcraft_model::SESSION_EXTENSION), opts).map_err(zerr)?;
    w.write_all(text.as_bytes()).map_err(ioerr)?;
    let destination = w.finish().map_err(zerr)?;
    Ok((destination, s))
}

fn bundle_size_with_audio(total: u64, frames: u64, channels: usize, header: usize) -> Result<u64> {
    frames
        .checked_mul(channels as u64)
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| n.checked_add(header as u64))
        .and_then(|n| n.checked_add(total))
        .filter(|n| *n <= MAX_BUNDLE_BYTES)
        .ok_or_else(|| EngineError::Io("bundle: too large".into()))
}

/// Save as a bundle named after `path`'s file name, delivered through the host ([`write_file`]).
fn save_session_bundle(e: &mut Engine, path: &str) -> Result<usize> {
    let stem = Path::new(path).file_stem().and_then(|x| x.to_str()).filter(|x| !x.is_empty()).map(str::to_string);
    let mut s = e.session().clone();
    if let Some(stem) = stem {
        s.name = stem;
    }
    let (bytes, fresh) = session_bundle(&s)?;
    let file = format!("{}.{BUNDLE_EXTENSION}", sanitize_name(&fresh.name));
    write_file(&file, &bytes)?;
    let written = fresh.sources.iter().filter(|x| fresh.pool.get(x.id).is_some()).count();
    *e.session_mut() = fresh;
    e.path = Some(file);
    e.mark_clean();
    Ok(written)
}

/// Open a session from bytes (the browser has no paths): a bundle ([`session_bundle`]) or a bare
/// `.scraft`, whose audio is then reported missing. Returns the missing media, like
/// [`open_session`].
pub fn open_session_bytes(e: &mut Engine, name: &str, bytes: &[u8]) -> Result<Vec<String>> {
    let (text, mut files) = if bytes.starts_with(b"PK\x03\x04") {
        read_bundle(bytes)?
    } else {
        (
            String::from_utf8(bytes.to_vec()).map_err(|_| EngineError::Io(format!("{name}: not a SoundCraft session")))?,
            std::collections::HashMap::new(),
        )
    };
    let mut s = Session::from_json(&text).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    let mut missing = Vec::new();
    for src in s.sources.clone() {
        match files.remove(&src.path) {
            Some(audio) => {
                let ext = Path::new(&src.path).extension().and_then(|x| x.to_str()).map(str::to_string);
                if let Err(err) = load_source_audio(&mut s, src.id, &audio, ext.as_deref()) {
                    missing.push(format!("{}: {err}", src.path));
                }
            }
            None => missing.push(src.path.clone()),
        }
    }
    // Movies are referenced by path, never copied into a session: none can be found here.
    missing.extend(s.videos.iter().map(|v| v.path.clone()));
    let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).filter(|x| !x.is_empty()).unwrap_or("Untitled").to_string();
    e.replace_session(s);
    e.path = Some(format!("{}.{BUNDLE_EXTENSION}", sanitize_name(&stem)));
    for m in &missing {
        e.message(format!("missing media: {m}"));
    }
    Ok(missing)
}

/// Unpack a bundle: the one `.scraft` at its top level and the files beside it, by relative path.
fn read_bundle(bytes: &[u8]) -> Result<(String, std::collections::HashMap<String, Vec<u8>>)> {
    use std::io::Read as _;
    let zerr = |err: zip::result::ZipError| EngineError::Io(format!("bundle: {err}"));
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(zerr)?;
    if z.len() > MAX_BUNDLE_ENTRIES {
        return Err(EngineError::Io("bundle: too many files".into()));
    }
    let (mut text, mut files, mut total) = (None, std::collections::HashMap::new(), 0u64);
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(zerr)?;
        if f.is_dir() {
            continue;
        }
        let rel = f.name().map_err(zerr)?.to_string();
        let size = f.size();
        total = total.saturating_add(size);
        if total > MAX_BUNDLE_BYTES {
            return Err(EngineError::Io("bundle: too large".into()));
        }
        let mut data = Vec::new();
        // The declared size is hostile too: never read past it.
        f.by_ref().take(size).read_to_end(&mut data).map_err(|err| EngineError::Io(format!("bundle: {rel}: {err}")))?;
        let is_session = !rel.contains('/') && Path::new(&rel).extension().and_then(|x| x.to_str()) == Some(soundcraft_model::SESSION_EXTENSION);
        if is_session && text.is_none() {
            text = Some(String::from_utf8(data).map_err(|_| EngineError::Io(format!("bundle: {rel} is not text")))?);
        } else {
            files.insert(rel, data);
        }
    }
    let text = text.ok_or_else(|| EngineError::Io("bundle: no .scraft session inside".into()))?;
    Ok((text, files))
}

/// The record-enabled audio tracks, each with the input channels it records (those its input
/// route names, "In 3" → 2, wrapped to the device's `input_channels`) and a new take file
/// `<track>_<NN>.wav` in the `Audio Files` of the session's folder (`unsaved_dir` until it is saved).
pub fn plan_takes(e: &Engine, unsaved_dir: &Path, input_channels: usize) -> Vec<(TrackId, Vec<usize>, PathBuf)> {
    let dir = e.path.as_deref().and_then(|p| Path::new(p).parent()).unwrap_or(unsaved_dir);
    let s = e.session();
    let take_no = s.sources.len() + 1;
    s.tracks
        .iter()
        .filter(|t| t.mixer.record_arm && t.kind == TrackKind::Audio && !t.inactive)
        .filter_map(|t| {
            let inputs: Option<Vec<usize>> = (0..t.channels()).map(|k| s.input_channel(&t.mixer.input, k, input_channels)).collect();
            let inputs = inputs?;
            let file = |n: usize| dir.join("Audio Files").join(format!("{}_{n:02}.wav", sanitize_name(&t.name)));
            let path = (take_no..take_no + 1_000).map(file).find(|p| !p.exists()).unwrap_or_else(|| file(take_no));
            Some((t.id, inputs, path))
        })
        .collect()
}

/// Turn recorded take files into clips at `start`, as one undo step ("Record"); each file is the
/// source of its clips. With `pass` (loop recording), a take becomes one clip per pass (at most
/// 1000), each pass after the first on a new playlist.
pub fn add_recording(e: &mut Engine, start: Samples, takes: Vec<(TrackId, PathBuf)>, pass: Option<Samples>) -> Vec<soundcraft_model::ClipId> {
    let before = e.session_arc();
    let mut out = Vec::new();
    for (tid, path) in takes {
        let read = std::fs::read(&path).map_err(|x| x.to_string()).and_then(|b| soundcraft_audio_io::decode(&b, None).map_err(|x| x.to_string()));
        let Ok((_, buf)) = read.inspect_err(|err| e.message(format!("{}: {err}", path.display()))) else { continue };
        let s = e.session_mut();
        if s.track(tid).is_none() {
            continue;
        }
        let resampled = buf.sample_rate != s.sample_rate.hz();
        let name = path.file_stem().and_then(|x| x.to_str()).unwrap_or("Take");
        let src = add_source(s, name, buf, Some(&path.to_string_lossy()), FileFormat::Wav);
        let Some(x) = s.sources.last_mut() else { continue };
        x.unsaved = resampled;
        let len = i64::try_from(x.frames).unwrap_or(0);
        let pass = pass.unwrap_or(len).max(1);
        for offset in (0..len).step_by(usize::try_from(pass).unwrap_or(usize::MAX)).take(1_000) {
            let n = pass.min(len - offset);
            if offset > 0 && n < pass / 8 {
                break;
            }
            if offset > 0
                && let Some(t) = s.track_mut(tid)
            {
                t.playlists.push(soundcraft_model::Playlist::new(format!("{}.{:02}", t.name, t.playlists.len() + 1)));
                t.active_playlist = t.playlists.len() - 1;
            }
            let cid = s.new_clip_id();
            crate::edit::place_clip(s, tid, Clip::audio(cid, name, src, offset, start, n));
            out.push(cid);
        }
    }
    if !out.is_empty() {
        e.push_undo("Record", before);
    }
    out
}

#[cfg(test)]
mod record_tests {
    use super::*;
    use serde_json::json;

    fn armed(e: &mut Engine, name: &str, format: &str, input: &str) -> TrackId {
        let id = e.execute("track.new", &json!({"name": name, "format": format})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [id], "input": input})).unwrap();
        e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
        TrackId(id)
    }

    fn write_take(path: &Path, frames: usize) {
        let mut bytes = soundcraft_audio_io::wav_float_header(1, 48_000, frames as u64).unwrap();
        bytes.extend((0..frames).flat_map(|i| (i as f32 / frames as f32 - 0.5).to_le_bytes()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn armed_tracks_get_new_take_files_used_in_place() {
        let dir = std::env::temp_dir().join(format!("soundcraft-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 2");
        let gtr = armed(&mut e, "Gtr", "stereo", "In 3-4");
        write_take(&dir.join("Audio Files/Vox_01.wav"), 10);
        let plan = plan_takes(&e, &dir, 4);
        assert_eq!(plan, vec![(vox, vec![1], dir.join("Audio Files/Vox_02.wav")), (gtr, vec![2, 3], dir.join("Audio Files/Gtr_01.wav"))]);
        assert_eq!(plan_takes(&e, &dir, 2).len(), 1, "unavailable inputs must not wrap back to channel one");

        let session = dir.join("Song.scraft").to_string_lossy().into_owned();
        e.execute("session.save_as", &json!({"path": session})).unwrap();
        write_take(&plan[0].2, 4_800);
        let clips = add_recording(&mut e, 96_000, vec![(vox, plan[0].2.clone())], None);
        let clip = e.session().find_clip(clips[0]).unwrap().1.clone();
        assert_eq!((clip.start, clip.length), (96_000, 4_800));
        assert_eq!(save_session(&mut e, &session).unwrap(), 0);
        let src = e.session().sources.last().unwrap().clone();
        assert_eq!((src.name.as_str(), src.path.as_str(), src.unsaved, src.frames), ("Vox_02", "Audio Files/Vox_02.wav", false, 4_800));
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().find_clip(clips[0]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_loop_take_becomes_one_clip_per_pass_and_autosaves_in_place() {
        let dir = std::env::temp_dir().join(format!("soundcraft-loop-take-{}", std::process::id()));
        let path = dir.join("Audio Files/Vox_01.wav");
        write_take(&path, 1_050);
        let mut e = Engine::default();
        let vox = armed(&mut e, "Vox", "mono", "In 1");
        assert_eq!(add_recording(&mut e, 1_000, vec![(vox, path.clone())], Some(400)).len(), 3);
        assert_eq!(save_copy(&e, &dir.join("Untitled.scraft").to_string_lossy()).unwrap(), 0);
        let t = e.session().track(vox).unwrap();
        let passes: Vec<_> = t.playlists.iter().flat_map(|p| p.clips.iter().map(|c| (c.source_offset(), c.start, c.length))).collect();
        assert_eq!(passes, vec![(0, 1_000, 400), (400, 1_000, 400), (800, 1_000, 250)]);
        assert_eq!((t.active_playlist, t.playlists[2].name.as_str()), (2, "Vox.03"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(vox).unwrap().playlists.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;

    #[test]
    fn autosave_copy_leaves_document_untouched_and_reuses_audio() {
        let mut e = crate::demo::demo_engine();
        e.execute("mix.volume", &serde_json::json!({"track": "Kick", "db": -7.0})).unwrap();
        let dir = std::env::temp_dir().join(format!("soundcraft-autosave-{}", std::process::id()));
        let path = dir.join("Midnight Groove.scraft");
        let p = path.to_string_lossy().into_owned();
        let n1 = save_copy(&e, &p).unwrap();
        let n2 = save_copy(&e, &p).unwrap();
        assert!(n1 >= 5);
        assert_eq!(n2, 0, "second autosave must not rewrite audio");
        assert!(e.is_dirty() && e.path.is_none());
        let mut r = Engine::default();
        open_session(&mut r, &p).unwrap();
        assert_eq!(r.session().track_by_name("Kick").unwrap().mixer.volume_db, -7.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn with_take(level: f32) -> Engine {
        let mut e = Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let src = add_source(s, "Audio 1_02", AudioBuffer { sample_rate: 48_000, channels: vec![vec![level; 4_800]] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::audio(id, "Audio 1_02", src, 0, 0, 4_800));
        e
    }

    fn reopen(path: &str) -> Engine {
        let mut r = Engine::default();
        assert_eq!(open_session(&mut r, path).unwrap(), Vec::<String>::new());
        r
    }

    #[test]
    fn bridge_stems_preserve_float_headroom_without_document_changes() {
        let mut engine = with_take(0.75);
        engine.session_mut().tracks[0].mixer.volume_db = 6.020_6;
        let before = engine.session().to_json().unwrap();
        let dirty = engine.is_dirty();
        let dir = std::env::temp_dir().join(format!("sculpt-bridge-headroom-{}", std::process::id()));
        let result = engine.execute("bridge.render_stems", &json!({"dir": dir})).unwrap();
        let files = result["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        let audio = soundcraft_audio_io::decode(&std::fs::read(files[0]["path"].as_str().unwrap()).unwrap(), None).unwrap().1;
        assert_eq!(audio.num_channels(), 1);
        assert_eq!(audio.frames(), 4800);
        // Preserve the existing -3 dB pan law and bridge mono compensation.
        let expected = 0.75 * 10.0_f32.powf(6.020_6 / 20.0) * 10.0_f32.powf(-3.0 / 20.0) * std::f32::consts::SQRT_2;
        assert!(expected > 1.5);
        assert!((audio.channels[0][2000] - expected).abs() < 0.000_01, "actual {}", audio.channels[0][2000]);
        assert_eq!(engine.session().to_json().unwrap(), before);
        assert_eq!(engine.is_dirty(), dirty);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn float_headroom_survives_project_save_copy_and_bundle() {
        let dir = std::env::temp_dir().join(format!("sculpt-float-headroom-{}", std::process::id()));
        let mut engine = with_take(1.5);
        let path = dir.join("Main.scdaw").to_string_lossy().into_owned();
        save_session(&mut engine, &path).unwrap();
        let restored = reopen(&path);
        let source = engine.session().sources[0].id;
        assert_eq!(restored.session().pool.get(source).unwrap().buffer.channels[0], vec![1.5; 4800]);
        let copy = dir.join("Copy/Copy.scdaw").to_string_lossy().into_owned();
        save_copy(&engine, &copy).unwrap();
        assert_eq!(reopen(&copy).session().pool.get(source).unwrap().buffer.channels[0], vec![1.5; 4800]);
        let before = engine.session().to_json().unwrap();
        let (bytes, _) = session_bundle(engine.session()).unwrap();
        let mut bundled = Engine::default();
        assert!(open_session_bytes(&mut bundled, "Headroom.scraftz", &bytes).unwrap().is_empty());
        assert_eq!(bundled.session().pool.get(source).unwrap().buffer.channels[0], vec![1.5; 4800]);
        assert_eq!(engine.session().to_json().unwrap(), before);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saving_never_replaces_audio_of_another_session() {
        let dir = std::env::temp_dir().join(format!("soundcraft-no-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = |p: &str| dir.join(p).to_string_lossy().into_owned();
        save_session(&mut with_take(0.5), &path("A/A.scraft")).unwrap();
        save_session(&mut with_take(-0.25), &path("A/A v2.scraft")).unwrap();
        save_session(&mut with_take(0.75), &path("B/B.scraft")).unwrap();
        save_session(&mut reopen(&path("B/B.scraft")), &path("A/B.scraft")).unwrap();
        let levels = ["A/A.scraft", "A/A v2.scraft", "B/B.scraft", "A/B.scraft"].map(|p| {
            let r = reopen(&path(p));
            r.session().pool.get(r.session().sources[0].id).unwrap().buffer.channels[0][0]
        });
        assert_eq!(levels, [0.5, -0.25, 0.75, 0.75]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn consolidating_twice_keeps_both_renders() {
        let dir = std::env::temp_dir().join(format!("soundcraft-consolidate-twice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().id;
        let end = e.session().content_end();
        let render = |e: &Engine| soundcraft_mix::render_clips(e.session(), kick, Range::new(0, end));
        e.execute("edit.consolidate", &json!({"tracks": [kick.0], "start": 500_000, "end": 560_000})).unwrap();
        save_session(&mut e, &path).unwrap();
        e.execute("edit.consolidate", &json!({"tracks": [kick.0], "start": 1_400_000, "end": 1_500_000})).unwrap();
        save_session(&mut e, &path).unwrap();
        assert!(render(&reopen(&path)) == render(&e));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_after_undo_reuses_the_saved_audio() {
        let dir = std::env::temp_dir().join(format!("soundcraft-undo-resave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        let mut e = with_take(0.5);
        e.execute("track.new", &json!({})).unwrap();
        assert_eq!(save_session(&mut e, &path).unwrap(), 1);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(save_session(&mut e, &path).unwrap(), 0);
        assert_eq!(std::fs::read_dir(dir.join("Audio Files")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_audio_is_trusted_only_in_the_folder_it_was_saved_in() {
        let dir = std::env::temp_dir().join(format!("soundcraft-own-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = |p: &str| dir.join(p).to_string_lossy().into_owned();
        let level = |r: &Engine| r.session().pool.get(r.session().sources[0].id).unwrap().buffer.channels[0][0];
        save_session(&mut with_take(-0.25), &path("B/Other.scraft")).unwrap();
        let mut e = with_take(0.5);
        save_session(&mut e, &path("A/A.scraft")).unwrap();
        e.execute("track.new", &json!({})).unwrap();
        save_session(&mut e, &path("B/B.scraft")).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("session.save", &json!({})).unwrap();
        assert_eq!(level(&reopen(&path("B/B.scraft"))), 0.5);
        e.execute("session.save_copy", &json!({"path": path("A/Copy.scraft")})).unwrap();
        e.execute("session.save", &json!({})).unwrap();
        assert_eq!(level(&reopen(&path("B/B.scraft"))), 0.5);
        std::fs::remove_file(dir.join("A/Audio Files/Audio 1_02.wav")).unwrap();
        let mut missing = Engine::default();
        open_session(&mut missing, &path("A/A.scraft")).unwrap();
        save_session(&mut missing, &path("B/Moved.scraft")).unwrap();
        let mut r = Engine::default();
        assert_eq!(open_session(&mut r, &path("B/Moved.scraft")).unwrap(), vec![dir.join("A/Audio Files/Audio 1_02.wav").display().to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_over_another_session_needs_replace() {
        let dir = std::env::temp_dir().join(format!("soundcraft-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song.scraft").to_string_lossy().into_owned();
        let mut first = with_take(0.5);
        first.execute("session.save_as", &json!({"path": path})).unwrap();
        first.execute("session.save", &json!({})).unwrap();
        first.execute("session.save_as", &json!({"path": dir.join("Song")})).unwrap();
        assert!(first.execute("session.save_template", &json!({"path": path})).is_err());
        let mut second = with_take(-0.25);
        let track = second.session().tracks[0].id.0;
        for id in ["session.save", "session.save_as", "session.save_copy", "session.save_template", "file.export_selected_tracks_as_session"] {
            let err = second.execute(id, &json!({"path": path, "tracks": [track]})).unwrap_err();
            assert!(err.to_string().contains("`replace: true`"), "{id}: {err}");
        }
        assert_eq!(std::fs::read_dir(dir.join("Audio Files")).unwrap().count(), 1);
        Engine::default().execute("session.save_as", &json!({"path": dir.join("New/New.scraft")})).unwrap();
        std::fs::copy(&path, dir.join("Song")).unwrap();
        let mut bare = Engine::default();
        open_session(&mut bare, &dir.join("Song").to_string_lossy()).unwrap();
        bare.execute("session.save", &json!({})).unwrap();
        second.execute("session.save_as", &json!({"path": path, "replace": true})).unwrap();
        let r = reopen(&path);
        assert_eq!(r.session().pool.get(r.session().sources[0].id).unwrap().buffer.channels[0][0], -0.25);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_keeps_the_last_ten_versions_as_backups() {
        let dir = std::env::temp_dir().join(format!("soundcraft-backups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Song.scraft");
        let mut e = Engine::default();
        let mut versions = Vec::new();
        for i in 0..12 {
            e.execute("track.new", &json!({"name": format!("T{i}")})).unwrap();
            save_session(&mut e, &path.to_string_lossy()).unwrap();
            save_session(&mut e, &path.to_string_lossy()).unwrap();
            versions.push(std::fs::read(&path).unwrap());
        }
        let backup = |n: usize| std::fs::read(dir.join(format!("Backups/Song backup {n}.scraft"))).ok();
        assert_eq!(std::fs::read_dir(dir.join("Backups")).unwrap().count(), 10);
        assert_eq!((backup(1), backup(2), backup(11)), (None, Some(versions[1].clone()), Some(versions[10].clone())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_gives_sources_that_share_a_path_their_own_files() {
        let mut s = Session::default();
        let loud = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 64]] };
        let quiet = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 64]] };
        add_source(&mut s, "tone", loud, Some("Audio Files/tone.wav"), FileFormat::Wav);
        add_source(&mut s, "tone", quiet, Some("Audio Files/tone.wav"), FileFormat::Wav);
        let mut e = Engine::new(s);
        let dir = std::env::temp_dir().join(format!("soundcraft-shared-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Shared.scraft");
        save_session(&mut e, &path.to_string_lossy()).unwrap();
        let paths: Vec<_> = e.session().sources.iter().map(|src| src.path.clone()).collect();
        assert_eq!(paths.len(), 2);
        assert_ne!(paths[0], paths[1], "{paths:?}");
        let a = std::fs::read(dir.join(&paths[0])).unwrap();
        let b = std::fs::read(dir.join(&paths[1])).unwrap();
        assert_ne!(a, b);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod stem_tests {
    #[test]
    fn stems_one_file_per_track() {
        let mut e = crate::demo::demo_engine();
        let dir = std::env::temp_dir().join(format!("soundcraft-stems-{}", std::process::id()));
        let r = e
            .execute("file.bounce_stems", &serde_json::json!({"dir": dir.to_string_lossy(), "start": {"seconds": 10.0}, "end": {"seconds": 11.0}}))
            .unwrap();
        let n = r["files"].as_array().map_or(0, Vec::len);
        assert_eq!(n, 7, "{r}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod bundle_tests {
    use super::*;

    #[test]
    fn bundle_size_limits_include_headers_and_reject_overflow() {
        assert_eq!(bundle_size_with_audio(4, 10, 7, 100).unwrap(), 384);
        assert!(bundle_size_with_audio(0, u64::MAX, 64, 100).is_err());
        assert!(bundle_size_with_audio(MAX_BUNDLE_BYTES, 0, 1, 100).is_err());
        assert!(bundle_size_with_audio(0, MAX_BUNDLE_BYTES / 4, 1, 100).is_err());
        assert_eq!(bundle_size_with_audio(0, (MAX_BUNDLE_BYTES - 100) / 4, 1, 100).unwrap(), MAX_BUNDLE_BYTES);
    }

    #[test]
    fn bundle_round_trips_session_and_audio() {
        let mut e = crate::demo::demo_engine();
        e.execute("mix.volume", &serde_json::json!({"track": "Kick", "db": -7.0})).unwrap();
        let (bytes, fresh) = session_bundle(e.session()).unwrap();
        assert!(bytes.starts_with(b"PK\x03\x04"));
        assert!(fresh.sources.iter().all(|s| s.path.starts_with("Audio Files/") && !s.unsaved));
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Midnight Groove.scraftz", &bytes).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(r.session().track_by_name("Kick").unwrap().mixer.volume_db, -7.0);
        assert_eq!(r.session().tracks.len(), e.session().tracks.len());
        for src in &e.session().sources {
            let (a, b) = (e.session().pool.get(src.id).unwrap(), r.session().pool.get(src.id).unwrap());
            assert_eq!(a.buffer.channels, b.buffer.channels, "{}: float WAV keeps every sample", src.name);
        }
        assert!(!r.is_dirty());
        assert_eq!(r.path.as_deref(), Some("Midnight Groove.scraftz"));
    }

    #[test]
    fn same_named_sources_get_distinct_files() {
        let mut e = crate::demo::demo_engine();
        let name = e.session().sources[0].name.clone();
        let s = e.session_mut();
        let n = s.sources.len();
        s.sources[1].name = name;
        let (bytes, fresh) = session_bundle(e.session()).unwrap();
        let paths: std::collections::HashSet<_> = fresh.sources.iter().map(|s| s.path.clone()).collect();
        assert_eq!(paths.len(), n);
        let mut r = Engine::default();
        assert!(open_session_bytes(&mut r, "x.scraftz", &bytes).unwrap().is_empty());
    }

    #[test]
    fn bare_session_opens_with_its_audio_reported_missing() {
        let e = crate::demo::demo_engine();
        let (_, fresh) = session_bundle(e.session()).unwrap();
        let text = fresh.to_json().unwrap();
        let mut r = Engine::default();
        let missing = open_session_bytes(&mut r, "Song.scraft", text.as_bytes()).unwrap();
        assert_eq!(missing.len(), fresh.sources.len());
        assert_eq!(r.session().tracks.len(), fresh.tracks.len());
    }

    #[test]
    fn hostile_bundles_are_errors_not_panics() {
        let mut r = Engine::default();
        for bad in [&b"PK\x03\x04 not really a zip"[..], b"", b"{ not json", b"\xff\xfe\x00"] {
            assert!(open_session_bytes(&mut r, "bad.scraftz", bad).is_err());
        }
        // A valid zip without a session inside.
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("Audio Files/a.wav", zip::write::SimpleFileOptions::default()).unwrap();
        let only_audio = w.finish().unwrap().into_inner();
        assert!(open_session_bytes(&mut r, "a.scraftz", &only_audio).is_err());
        assert_eq!(r.session().tracks.len(), 0, "a failed open leaves the document alone");
    }

    #[test]
    fn write_file_writes_to_disk_natively() {
        let p = std::env::temp_dir().join(format!("soundcraft-write-file-{}.txt", std::process::id()));
        write_file(&p.to_string_lossy(), b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        let _ = std::fs::remove_file(&p);
    }
}

#[cfg(test)]
mod sculpt_core_session_roundtrip_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sculpt_core_save_and_open_session_with_audio_and_fades() {
        let base = std::env::temp_dir().join(format!("sculpt-core-reopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let audio = base.join("incoming.wav");
        let mut wav = soundcraft_audio_io::wav_float_header(1, 48_000, 4800).unwrap();
        wav.extend((0..4800).flat_map(|i| ((i % 20) as f32 / 40.0).to_le_bytes()));
        std::fs::write(&audio, wav).unwrap();
        let mut first = Engine::default();
        first.execute("file.import_audio", &json!({"path": audio.to_str().unwrap()})).unwrap();
        let tid = first.session().tracks[0].id;
        let cid = first.session().tracks[0].clips()[0].id;
        let save = base.join("My Sculpt Session.scraft");
        first.execute("edit.select", &json!({"tracks":[tid.0], "start":0, "end":600})).unwrap();
        first.execute("edit.fades_create", &json!({"tracks":[tid.0], "start":0,"end":600})).unwrap();
        first.execute("session.save_as", &json!({"path":save.to_str().unwrap()})).unwrap();
        assert!(save.is_file());
        assert!(base.join("Audio Files").is_dir());
        let mut reopened = Engine::default();
        let missing = open_session(&mut reopened, save.to_str().unwrap()).unwrap();
        assert!(missing.is_empty(), "missing media: {missing:?}");
        assert_eq!(reopened.session().tracks[0].clips()[0].id, cid);
        assert_eq!(reopened.session().tracks[0].clips()[0].fade_in.len, 600);
        assert!(reopened.session().pool.get(reopened.session().sources[0].id).is_some());
        reopened.execute("session.save", &json!({})).unwrap();
        let mut again = Engine::default();
        again.execute("session.open", &json!({"path":base.join("My Sculpt Session").to_str().unwrap()})).unwrap();
        assert_eq!(again.session().tracks[0].clips()[0].fade_in.len, 600);
        let _ = std::fs::remove_dir_all(base);
    }
}

#[cfg(test)]
mod sculpt_core16_file_type_tests {
    use super::*;

    #[test]
    fn sculpt_core16_scdaw_is_native_and_scraft_remains_readable() {
        assert_eq!(soundcraft_model::SESSION_EXTENSION, "scdaw");
        let root = std::env::temp_dir().join(format!("sculpt-core16-types-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let native = root.join("Native.scdaw");
        let legacy = root.join("Legacy.scraft");
        let json = Session::default().to_json().unwrap();
        std::fs::write(&native, &json).unwrap();
        std::fs::write(&legacy, &json).unwrap();
        for p in [&native, &legacy] {
            let mut e = Engine::default();
            let missing = open_session(&mut e, p.to_str().unwrap()).unwrap();
            assert!(missing.is_empty());
            assert!(e.path.as_deref().is_some_and(|s| s == p.to_str().unwrap()));
        }
        assert_eq!(session_file(root.join("Native").to_str().unwrap()), native);
        assert_eq!(session_file(root.join("Legacy").to_str().unwrap()), legacy);
        assert_eq!(session_file(root.join("New Project").to_str().unwrap()), root.join("New Project.scdaw"));
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)]
mod offline_source_tests {
    use super::*;
    use soundcraft_audio_io::{
        BitDepth,
        stream_cache::{CacheConfig, StreamCache},
    };
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        path: PathBuf,
        audio: AudioBuffer,
    }
    impl Fixture {
        fn new(channels: usize) -> Self {
            let path =
                std::env::temp_dir().join(format!("sculpt-offline-command-{}-{}.wav", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
            let audio = AudioBuffer {
                sample_rate: 48_000,
                channels: (0..channels).map(|ch| (0..6000).map(|f| (f as f32 * 0.017 + ch as f32).sin() * 0.3).collect()).collect(),
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
    fn engine(fixture: &Fixture, cache: Option<&Arc<StreamCache>>) -> (Engine, TrackId) {
        let mut s = Session::default();
        let track = s.add_track(TrackKind::Audio, ChannelFormat::for_channels(fixture.audio.num_channels()), Some("Audio"));
        let source = add_source(&mut s, "Audio", fixture.audio.clone(), None, FileFormat::Wav);
        if let Some(cache) = cache {
            let disk = SourceAudio::from_stream(cache.open_source(&fixture.path).unwrap(), 140, |_, _| true).unwrap();
            s.pool.insert(source, Arc::new(disk));
        }
        let clip = Clip::audio(s.new_clip_id(), "Audio", source, 0, 0, 6000);
        s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
        (Engine::new(s), track)
    }
    #[test]
    fn disk_sources_stream_to_bundle_file_and_reopen_without_document_changes() {
        let fixture = Fixture::new(7);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let (e, _) = engine(&fixture, Some(&cache));
        let before = e.session().to_json().unwrap();
        let path = fixture.path.with_extension("scraftz");
        let destination = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        let (destination, fresh) = write_session_bundle(e.session(), destination).unwrap();
        destination.sync_all().unwrap();
        drop(destination);
        let mut restored = Engine::default();
        let bytes = std::fs::read(&path).unwrap();
        assert!(open_session_bytes(&mut restored, "Disk.scraftz", &bytes).unwrap().is_empty());
        let source = e.session().sources[0].id;
        assert_eq!(restored.session().pool.get(source).unwrap().buffer, fixture.audio);
        assert_eq!(e.session().to_json().unwrap(), before);
        assert!(!e.session().pool.get(source).unwrap().is_resident());
        assert!(!fresh.pool.get(source).unwrap().is_resident());
        std::fs::OpenOptions::new().write(true).open(&fixture.path).unwrap().set_len(100).unwrap();
        assert!(write_session_bundle(e.session(), std::io::Cursor::new(Vec::new())).is_err());
        assert_eq!(e.session().to_json().unwrap(), before);
        cache.shutdown(Duration::from_secs(1)).unwrap();
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn bundle_destination_failure_does_not_modify_source_document() {
        use std::io::{self, Seek, SeekFrom, Write};
        struct FailingDestination {
            cursor: io::Cursor<Vec<u8>>,
            budget: usize,
        }
        impl Write for FailingDestination {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.budget == 0 {
                    return Err(io::Error::other("injected bundle disk failure"));
                }
                let count = self.budget.min(bytes.len());
                self.budget -= count;
                self.cursor.write(bytes.get(..count).unwrap()).inspect(|&n| {
                    assert!(n <= 262_144);
                })
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl Seek for FailingDestination {
            fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
                self.cursor.seek(from)
            }
        }
        let fixture = Fixture::new(7);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let (e, _) = engine(&fixture, Some(&cache));
        let before = e.session().to_json().unwrap();
        for budget in [0, 256, 4096] {
            let destination = FailingDestination { cursor: io::Cursor::new(Vec::new()), budget };
            assert!(write_session_bundle(e.session(), destination).is_err());
            assert_eq!(e.session().to_json().unwrap(), before);
            assert!(!e.session().pool.get(e.session().sources[0].id).unwrap().is_resident());
        }
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn disk_source_save_copy_reuse_and_failure_preserve_audio_and_document() {
        let fixture = Fixture::new(7);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let (mut e, _) = engine(&fixture, Some(&cache));
        let dir = fixture.path.with_extension("project");
        let path = dir.join("Main.scdaw").to_string_lossy().into_owned();
        assert_eq!(save_session(&mut e, &path).unwrap(), 1);
        let source = e.session().sources[0].id;
        assert!(!e.session().pool.get(source).unwrap().is_resident());
        let mut restored = Engine::default();
        assert!(open_session(&mut restored, &path).unwrap().is_empty());
        assert_eq!(restored.session().pool.get(source).unwrap().buffer, fixture.audio);
        let copy = dir.join("Copy/Copy.scdaw").to_string_lossy().into_owned();
        let before = e.session().to_json().unwrap();
        assert_eq!(save_copy(&e, &copy).unwrap(), 1);
        assert_eq!(save_copy(&e, &copy).unwrap(), 0);
        assert_eq!(e.session().to_json().unwrap(), before);
        let audio = e.session().pool.get(source).unwrap();
        let target = dir.join("Copy/Audio Files");
        std::fs::write(target.join("Collision.wav"), b"unrelated").unwrap();
        assert_eq!(store_source_wav(&target, "Collision", audio).unwrap(), ("Collision_01.wav".into(), true));
        assert_eq!(std::fs::read(target.join("Collision.wav")).unwrap(), b"unrelated");
        std::fs::OpenOptions::new().write(true).open(&fixture.path).unwrap().set_len(100).unwrap();
        let fail = dir.join("Failed/Failed.scdaw");
        assert!(save_copy(&e, &fail.to_string_lossy()).is_err());
        assert!(!fail.exists());
        assert_eq!(std::fs::read_dir(dir.join("Failed/Audio Files")).unwrap().count(), 0);
        assert_eq!(e.session().to_json().unwrap(), before);
        cache.shutdown(Duration::from_secs(1)).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn offline_edit_failures_leave_document_and_undo_unchanged() {
        let fixture = Fixture::new(2);
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let (mut e, track) = engine(&fixture, Some(&cache));
        let before = e.session().to_json().unwrap();
        std::fs::OpenOptions::new().write(true).open(&fixture.path).unwrap().set_len(100).unwrap();
        for missing in [false, true] {
            if missing {
                let source = e.session().sources[0].id;
                e.session_mut().pool.remove(source);
                e.mark_clean();
            }
            for command in ["edit.strip_silence", "edit.consolidate", "edit.separate_at_transients", "track.commit", "edit.copy_audio_as_midi"] {
                let error = e.execute(command, &json!({"tracks":[track.0],"start":0,"end":6000})).unwrap_err();
                assert!(matches!(error, EngineError::Io(_)), "{command}: {error}");
                assert_eq!(e.session().to_json().unwrap(), before, "{command} changed the document");
                assert!(!e.can_undo());
                assert!(!e.is_dirty());
            }
        }
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn consolidate_and_commit_from_disk_match_resident_and_preserve_multichannel_width() {
        for channels in [1, 2, 7] {
            let fixture = Fixture::new(channels);
            let cache = StreamCache::new(CacheConfig::default()).unwrap();
            for command in ["edit.consolidate", "track.commit"] {
                let (mut streamed, track) = engine(&fixture, Some(&cache));
                let (mut resident, _) = engine(&fixture, None);
                let source = streamed.session().sources[0].id;
                let params = json!({"tracks":[track.0],"start":0,"end":6000});
                let a = streamed.execute(command, &params).unwrap();
                let b = resident.execute(command, &params).unwrap();
                assert_eq!(a, b);
                assert_eq!(streamed.session().to_json().unwrap(), resident.session().to_json().unwrap());
                let result = streamed.session().sources.last().unwrap().id;
                assert_eq!(streamed.session().pool.get(result).unwrap().buffer, resident.session().pool.get(result).unwrap().buffer);
                assert_eq!(streamed.session().pool.get(result).unwrap().channels(), channels);
                assert!(!streamed.session().pool.get(source).unwrap().is_resident());
                if command == "track.commit" {
                    assert_eq!(streamed.session().tracks.last().unwrap().channels(), channels);
                }
                assert!(streamed.undo());
                assert_eq!(streamed.session().sources.len(), 1);
                assert!(!streamed.session().pool.get(source).unwrap().is_resident());
            }
            cache.shutdown(Duration::from_secs(1)).unwrap();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod exclusive_backup_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("sculpt-backup-{}-{}-{stamp}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn backup_beyond_u32_max_preserves_old_bytes() {
        let fixture = Fixture::new();
        let file = fixture.0.join("Song.scdaw");
        std::fs::write(&file, b"current").unwrap();
        let dir = fixture.0.join("Backups");
        std::fs::create_dir(&dir).unwrap();
        let existing = dir.join(format!("Song backup {}.scdaw", u32::MAX));
        std::fs::write(&existing, b"older snapshot").unwrap();
        back_up(&file).unwrap();
        assert_eq!(std::fs::read(&existing).unwrap(), b"older snapshot");
        assert_eq!(std::fs::read(dir.join(format!("Song backup {}.scdaw", u64::from(u32::MAX) + 1))).unwrap(), b"current");
    }
    #[test]
    fn exhausted_counter_preserves_every_existing_file() {
        let fixture = Fixture::new();
        let file = fixture.0.join("Song.scdaw");
        std::fs::write(&file, b"current").unwrap();
        let dir = fixture.0.join("Backups");
        std::fs::create_dir(&dir).unwrap();
        let old = dir.join(format!("Song backup {}.scdaw", u64::MAX));
        std::fs::write(&old, b"original backup").unwrap();
        assert!(back_up(&file).unwrap_err().to_string().contains("counter exhausted"));
        assert_eq!(std::fs::read(&old).unwrap(), b"original backup");
        assert_eq!(std::fs::read(&file).unwrap(), b"current");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }
    #[test]
    fn occupied_candidate_file_and_directory_are_never_replaced() {
        let fixture = Fixture::new();
        let file = fixture.0.join("Song.scdaw");
        std::fs::write(&file, b"current").unwrap();
        let dir = fixture.0.join("Backups");
        std::fs::create_dir(&dir).unwrap();
        let first = dir.join("Song backup 1.scdaw");
        std::fs::write(&first, b"older").unwrap();
        let second = dir.join("Song backup 2.scdaw");
        std::fs::create_dir(&second).unwrap();
        let (number, destination) = copy_exclusive_backup(&file, &dir, "Song backup ", ".scdaw", 1).unwrap();
        assert_eq!(number, 3);
        assert_eq!(std::fs::read(destination).unwrap(), b"current");
        assert_eq!(std::fs::read(first).unwrap(), b"older");
        assert!(second.is_dir());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed_and_private_permissions_are_preserved() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        let file = fixture.0.join("Song.scdaw");
        std::fs::write(&file, b"current").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let dir = fixture.0.join("Backups");
        std::fs::create_dir(&dir).unwrap();
        let unrelated = fixture.0.join("other-data");
        std::fs::write(&unrelated, b"leave intact").unwrap();
        symlink(&unrelated, dir.join("Song backup 1.scdaw")).unwrap();
        let (number, destination) = copy_exclusive_backup(&file, &dir, "Song backup ", ".scdaw", 1).unwrap();
        assert_eq!(number, 2);
        assert_eq!(std::fs::read(unrelated).unwrap(), b"leave intact");
        assert!(std::fs::symlink_metadata(dir.join("Song backup 1.scdaw")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::metadata(destination).unwrap().permissions().mode() & 0o777, 0o600);
    }
    #[test]
    fn retention_keeps_ten_snapshots_even_when_numbers_have_gaps() {
        let fixture = Fixture::new();
        let file = fixture.0.join("Song.scraft");
        std::fs::write(&file, b"current").unwrap();
        let dir = fixture.0.join("Backups");
        std::fs::create_dir(&dir).unwrap();
        for number in [1, 2, 3, 4, 5, 6, 7, 8, 9, 100] {
            std::fs::write(dir.join(format!("Song backup {number}.scraft")), number.to_string()).unwrap();
        }
        std::fs::write(dir.join("notes.txt"), b"unrelated").unwrap();
        let destination = back_up(&file).unwrap();
        assert_eq!(destination.file_name().unwrap(), "Song backup 101.scraft");
        assert!(!dir.join("Song backup 1.scraft").exists());
        assert_eq!(std::fs::read(dir.join("Song backup 2.scraft")).unwrap(), b"2");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 11);
        assert_eq!(std::fs::read(dir.join("notes.txt")).unwrap(), b"unrelated");
    }
    #[test]
    fn copy_failure_removes_its_incomplete_destination() {
        let fixture = Fixture::new();
        let directory_source = fixture.0.join("source-dir");
        std::fs::create_dir(&directory_source).unwrap();
        let dir = fixture.0.join("Backups");
        std::fs::create_dir(&dir).unwrap();
        assert!(copy_exclusive_backup(&directory_source, &dir, "Song backup ", ".scdaw", 1).is_err());
        assert!(!dir.join("Song backup 1.scdaw").exists());
        assert!(directory_source.is_dir());
    }
    #[test]
    fn failed_backup_preserves_project_and_unsaved_document_in_native_legacy_and_bare_paths() {
        for name in ["Song.scdaw", "Song.scraft", "Song"] {
            let fixture = Fixture::new();
            let path = fixture.0.join(name);
            let mut e = Engine::new(Session::new("Song", Default::default()));
            if name == "Song" {
                std::fs::write(&path, e.session().to_json().unwrap()).unwrap();
            }
            save_session(&mut e, &path.to_string_lossy()).unwrap();
            let saved = std::fs::read(&path).unwrap();
            e.execute("track.new", &json!({"name":"Unsaved"})).unwrap();
            let pending = e.session().to_json().unwrap();
            let history = e.undo_history();
            std::fs::write(fixture.0.join("Backups"), b"not a directory").unwrap();
            assert!(e.execute("session.save", &json!({})).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), saved);
            assert_eq!(e.session().to_json().unwrap(), pending);
            assert_eq!(e.undo_history(), history);
            assert!(e.is_dirty());
        }
    }
    #[test]
    fn project_comparison_streams_exact_bytes_and_detects_content_or_length_changes() {
        let fixture = Fixture::new();
        let path = fixture.0.join("project");
        let mut bytes = vec![b'x'; 30_000];
        std::fs::write(&path, &bytes).unwrap();
        assert!(file_matches_bytes(&path, &bytes).unwrap());
        bytes[15_000] = b'y';
        assert!(!file_matches_bytes(&path, &bytes).unwrap());
        bytes.push(b'z');
        assert!(!file_matches_bytes(&path, &bytes).unwrap());
        assert!(!file_matches_bytes(&path, b"").unwrap());
    }
    #[test]
    fn concurrent_backup_reservations_have_unique_names_and_complete_bytes() {
        let fixture = Fixture::new();
        let file = fixture.0.join("Song.scdaw");
        std::fs::write(&file, b"current snapshot").unwrap();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let file = file.clone();
                std::thread::spawn(move || back_up(&file))
            })
            .collect();
        let paths: std::collections::BTreeSet<_> = workers.into_iter().map(|worker| worker.join().unwrap().unwrap()).collect();
        assert_eq!(paths.len(), 8);
        for path in paths {
            assert_eq!(std::fs::read(path).unwrap(), b"current snapshot");
        }
        assert_eq!(std::fs::read(&file).unwrap(), b"current snapshot");
        assert_eq!(std::fs::read_dir(fixture.0.join("Backups")).unwrap().count(), 8);
    }
}
