//! Helpers shared by the newer command modules: settings kept in `EditState.flags`/`values`,
//! per-track settings, target playlists, clip-gain envelopes and stretch rendering.

use super::*;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{Clip, ClipContent, EditState, Session, SourceId, TrackId};
use soundcraft_time::Samples;

/// Toggle (or set from `value`) a flag in `edit.flags`; returns the new state.
pub fn toggle_flag(e: &mut Engine, p: &Value, id: &str) -> Result<Value> {
    let s = e.session_mut();
    let on = p.get("value").and_then(Value::as_bool).unwrap_or(!s.edit.flag(id));
    s.edit.set_flag(id, on);
    Ok(json!({"flag": id, "value": on}))
}

/// Turn `id` on and every other flag of `group` off (radio-style menu items).
pub fn exclusive_flag(e: &mut Engine, id: &str, group: &[&str]) -> Result<Value> {
    let s = e.session_mut();
    for g in group {
        s.edit.set_flag(g, false);
    }
    s.edit.set_flag(id, true);
    Ok(json!({"flag": id, "value": true}))
}

/// Set several flags at once.
pub fn set_flags(e: &mut Engine, ids: &[&str], on: bool) -> Value {
    let s = e.session_mut();
    for id in ids {
        s.edit.set_flag(id, on);
    }
    json!({"flags": ids, "value": on})
}

/// String settings live in `edit.flags` as `key=value` (one per key).
pub fn set_text(edit: &mut EditState, key: &str, value: Option<&str>) {
    let prefix = format!("{key}=");
    edit.flags.retain(|f| !f.starts_with(&prefix));
    if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
        let v: String = v.chars().take(200).collect();
        edit.flags.insert(format!("{prefix}{v}"));
    }
}

pub fn text(edit: &EditState, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    edit.flags.iter().find_map(|f| f.strip_prefix(&prefix).map(str::to_string))
}

/// All `values` whose key starts with `prefix`, as a JSON object (prefix stripped).
pub fn values_with_prefix(edit: &EditState, prefix: &str) -> Value {
    let m: serde_json::Map<String, Value> =
        edit.values.iter().filter_map(|(k, v)| k.strip_prefix(prefix).map(|rest| (rest.to_string(), json!(v)))).collect();
    Value::Object(m)
}

/// Store a finite number from `p[key]` into `values[target]`, clamped. Returns whether it was given.
pub fn store_value(edit: &mut EditState, p: &Value, key: &str, target: &str, lo: f64, hi: f64) -> bool {
    match p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()) {
        Some(v) => {
            edit.values.insert(target.to_string(), v.clamp(lo, hi));
            true
        }
        None => false,
    }
}

/// Key for a per-track value, e.g. `rtp.12.velocity`.
pub fn track_key(prefix: &str, t: TrackId, name: &str) -> String {
    format!("{prefix}.{}.{name}", t.0)
}

/// The track's designated target playlist index, if valid.
pub fn target_playlist(s: &Session, t: TrackId) -> Option<usize> {
    let v = s.edit.values.get(&format!("playlist.target.{}", t.0)).copied()?;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    let i = v as usize;
    s.track(t).filter(|tr| i < tr.playlists.len()).map(|_| i)
}

/// Make `idx` the active playlist, remembering the previous one for Toggle Recent Playlist.
pub fn activate_playlist(s: &mut Session, t: TrackId, idx: usize) -> bool {
    let Some(tr) = s.track_mut(t) else { return false };
    if idx >= tr.playlists.len() || idx == tr.active_playlist {
        return false;
    }
    let prev = tr.active_playlist;
    tr.active_playlist = idx;
    s.edit.values.insert(format!("playlist.recent.{}", t.0), prev as f64);
    true
}

/// Interpolated value of a `(offset, dB)` envelope (0 when empty), like the model's clip gain.
pub fn env_db(env: &[(Samples, f32)], at: Samples) -> f32 {
    let mut prev: Option<(Samples, f32)> = None;
    for &(t, v) in env {
        if t >= at {
            return match prev {
                Some((pt, pv)) if t > pt => pv + (v - pv) * ((at - pt) as f32 / (t - pt) as f32),
                _ => v,
            };
        }
        prev = Some((t, v));
    }
    prev.map_or(0.0, |(_, v)| v)
}

/// Absolute clip gain (static gain + envelope) in dB at `rel` samples from the clip start.
pub fn clip_db(c: &Clip, rel: Samples) -> f32 {
    c.gain_db + if c.gain_env.is_empty() { 0.0 } else { env_db(&c.gain_env, rel) }
}

/// Replace a clip's gain with an absolute envelope (sorted, deduplicated, clamped).
pub fn set_clip_env(c: &mut Clip, mut pts: Vec<(Samples, f32)>) {
    pts.retain(|(o, v)| *o >= 0 && *o <= c.length && v.is_finite());
    pts.sort_by_key(|p| p.0);
    pts.dedup_by_key(|p| p.0);
    for p in &mut pts {
        p.1 = p.1.clamp(-144.0, 36.0);
    }
    c.gain_db = 0.0;
    let flat = pts.windows(2).all(|w| w.first().map(|a| a.1) == w.get(1).map(|b| b.1));
    if flat {
        c.gain_db = pts.first().map_or(0.0, |p| p.1);
        c.gain_env.clear();
    } else {
        c.gain_env = pts;
    }
}

/// Longest audio we render in one stretch (one hour at the session rate).
fn max_render(s: &Session) -> Samples {
    s.sample_rate.samples(3600.0)
}

/// Render the audio a clip plays, time-stretched (pitch preserved) to `out_len` samples, into a new
/// source. Returns `Ok(None)` for MIDI clips; unavailable audio is an explicit error.
pub fn render_stretched(s: &mut Session, cmd: &str, c: &Clip, out_len: Samples) -> Result<Option<SourceId>> {
    let ClipContent::Audio { source, offset } = c.content else { return Ok(None) };
    let audio = s.pool.get(source).cloned().ok_or_else(|| EngineError::Io(format!("{}: source audio is unavailable", c.name)))?;
    if out_len <= 0 || out_len > max_render(s) {
        return Err(bad(cmd, "the target length must be between 1 sample and one hour"));
    }
    let stretch = if c.stretch.is_finite() && c.stretch > 0.0 { c.stretch } else { 1.0 };
    let src_len = ((c.length as f64) / stretch).round().max(1.0);
    let ratio = out_len as f64 / src_len;
    if !(0.1..=10.0).contains(&ratio) {
        return Err(bad(cmd, format!("stretch ratio {ratio:.3} is outside 0.1..10")));
    }
    let a = usize::try_from(offset.max(0)).unwrap_or(0);
    let n = src_len as usize;
    let want = usize::try_from(out_len).map_err(|_| bad(cmd, "target frame count exceeds host range"))?;
    if want > soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES
        || want.checked_mul(audio.channels()).is_none_or(|n| n > soundcraft_mix::MAX_RENDER_SAMPLES)
    {
        return Err(bad(cmd, "time-stretch output exceeds the offline sample budget"));
    }
    if audio.sample_rate() != s.sample_rate.hz() {
        return Err(bad(cmd, "convert the source to the session sample rate before time-stretch"));
    }
    let ch = audio.read_region(a, n, soundcraft_mix::MAX_RENDER_SAMPLES).map_err(|error| EngineError::Io(error.to_string()))?.channels;
    let sr = s.sample_rate;
    let out = soundcraft_dsp::offline::time_stretch(&ch, ratio, sr.as_f64() as f32);
    if out.len() != audio.channels() || out.iter().any(|channel| channel.len() != want) {
        return Err(EngineError::Io("time-stretch returned an incomplete result".into()));
    }
    let buf = AudioBuffer { sample_rate: sr.hz(), channels: out };
    Ok(Some(crate::io::add_source(s, &format!("{}-TCE", c.name), buf, None, FileFormat::Wav)))
}

/// Replace clip `old` on track `t` with `new` (overwriting whatever `new` covers).
pub fn replace_clip(s: &mut Session, t: TrackId, old: soundcraft_model::ClipId, new: Clip) {
    if let Some(pl) = s.track_mut(t).and_then(|tr| tr.playlist_mut()) {
        pl.clips.retain(|c| c.id != old);
    }
    crate::edit::place_clip(s, t, new);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_settings_round_trip() {
        let mut ed = EditState::default();
        set_text(&mut ed, "engine.device", Some("Built-in"));
        set_text(&mut ed, "engine.device", Some("Interface"));
        assert_eq!(text(&ed, "engine.device").as_deref(), Some("Interface"));
        set_text(&mut ed, "engine.device", None);
        assert_eq!(text(&ed, "engine.device"), None);
    }

    #[test]
    fn envelope_interpolates() {
        let env = [(0, 0.0f32), (100, -10.0)];
        assert!((env_db(&env, 50) + 5.0).abs() < 1e-6);
        assert_eq!(env_db(&[], 50), 0.0);
        assert_eq!(env_db(&env, 500), -10.0);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)]
mod disk_stretch_tests {
    use super::*;
    use soundcraft_audio_io::{
        BitDepth, EncodeOptions,
        stream_cache::{CacheConfig, StreamCache},
    };
    use soundcraft_model::{ChannelFormat, SourceAudio, TrackKind};
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn prepare(channels: usize) -> (Fixture, Arc<StreamCache>, Session, Session, Clip) {
        let path = std::env::temp_dir().join(format!("sculpt-stretch-{}-{}.wav", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
        let audio = AudioBuffer {
            sample_rate: 48_000,
            channels: (0..channels).map(|c| (0..6000).map(|f| (f as f32 * 0.017 + c as f32).sin() * 0.3).collect()).collect(),
        };
        std::fs::write(
            &path,
            soundcraft_audio_io::encode(&audio, &EncodeOptions { bit_depth: BitDepth::Float32, dither: false, ..EncodeOptions::default() }).unwrap(),
        )
        .unwrap();
        let mut resident = Session::default();
        let t = resident.add_track(TrackKind::Audio, ChannelFormat::for_channels(channels), None);
        let source = crate::io::add_source(&mut resident, "fixture", audio, None, FileFormat::Wav);
        let clip = Clip::audio(resident.new_clip_id(), "region", source, 503, 0, 4096);
        resident.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(clip.clone());
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut disk = resident.clone();
        disk.pool.insert(source, Arc::new(SourceAudio::from_stream(cache.open_source(&path).unwrap(), 140, |_, _| true).unwrap()));
        (Fixture(path), cache, disk, resident, clip)
    }
    #[test]
    fn stretch_reads_selected_region_and_matches_resident_for_all_channels() {
        for channels in [1, 2, 7] {
            let (_fixture, cache, mut disk, mut resident, clip) = prepare(channels);
            let a = render_stretched(&mut disk, "test", &clip, 5000).unwrap().unwrap();
            let b = render_stretched(&mut resident, "test", &clip, 5000).unwrap().unwrap();
            assert_eq!(disk.pool.get(a).unwrap().buffer, resident.pool.get(b).unwrap().buffer);
            assert_eq!(disk.pool.get(a).unwrap().frames(), 5000);
            assert_eq!(disk.pool.get(a).unwrap().channels(), channels);
            assert!(!disk.pool.get(clip.source().unwrap()).unwrap().is_resident());
            let mut engine = Engine::new(disk);
            let before = engine.session_arc();
            let revision = engine.revision;
            assert_eq!(engine.execute("session.audio_health", &json!({})).unwrap()["healthy"], true);
            assert!(Arc::ptr_eq(&before, &engine.session_arc()));
            assert_eq!(engine.revision, revision);
            assert!(!engine.is_dirty());
            assert!(!engine.can_undo());
            cache.shutdown(Duration::from_secs(1)).unwrap();
        }
    }
    #[test]
    fn stretch_rejects_media_changes_missing_sources_and_dsp_truncation_before_editing() {
        let (fixture, cache, mut disk, _resident, clip) = prepare(2);
        let before = disk.to_json().unwrap();
        std::fs::OpenOptions::new().write(true).open(&fixture.0).unwrap().set_len(100).unwrap();
        assert!(render_stretched(&mut disk, "test", &clip, 5000).is_err());
        assert_eq!(disk.to_json().unwrap(), before);
        disk.pool.remove(clip.source().unwrap());
        assert!(render_stretched(&mut disk, "test", &clip, 5000).is_err());
        assert_eq!(disk.to_json().unwrap(), before);
        cache.shutdown(Duration::from_secs(1)).unwrap();
        let (_fixture, cache, mut disk, _resident, mut clip) = prepare(1);
        disk.sample_rate = soundcraft_time::SampleRate::new(192_000).unwrap();
        let want = soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES as i64 + 1;
        clip.length = want;
        assert!(render_stretched(&mut disk, "test", &clip, want).unwrap_err().to_string().contains("budget"));
        assert_eq!(cache.stats().read_bytes, 0);
        cache.shutdown(Duration::from_secs(1)).unwrap();
    }
}
