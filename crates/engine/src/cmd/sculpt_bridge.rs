//! Offline bridge renderer: one aligned mono/stereo file per source track.
use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ChannelFormat, OutputPath, Route, TrackKind};
use std::{fs, path::Path};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(query "bridge.render_stems", "Render Show Stems", [], None,
        "{dir}: new empty output directory; edits/inserts/fader automation baked, mute retained in manifest",
        has_tracks, render)]
}

fn render(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "bridge.render_stems";
    let dir = str_param(p, "dir").ok_or_else(|| bad(id, "dir required"))?;
    let s = e.session();
    let tracks: Vec<_> = s.tracks.iter().filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi)).collect();
    let frames = s.content_end().max(1);
    // render_range caps allocations. Refuse rather than silently truncate long material.
    if tracks.is_empty() || tracks.len() > 64 || frames as u64 > (soundcraft_mix::MAX_RENDER_SAMPLES / 2) as u64 {
        return Err(bad(id, "empty, too many stems, or session exceeds offline render limit"));
    }
    // Shared aux/master processing is not separable into independent track stems.
    // Reject it explicitly; never silently publish a different mix.
    for t in &s.tracks {
        if matches!(t.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi) {
            if t.inactive
                || !(1..=2).contains(&t.channels())
                || !matches!(t.mixer.output, Route::Main | Route::Hardware(_))
                || t.mixer.sends.iter().flatten().any(|send| !send.mute)
            {
                return Err(bad(
                    id,
                    format!("{}: activate mono/stereo tracks with direct outputs; shared bus/send processing requires a consolidated return", t.name),
                ));
            }
        } else if !t.inactive
            && (matches!(t.kind, TrackKind::Master | TrackKind::Vca))
            && (t.mixer.volume_db != 0.0
                || t.mixer.trim_db != 0.0
                || t.mixer.mute
                || t.mixer.inserts.iter().flatten().any(|x| !x.bypass)
                || !t.automation.is_empty())
        {
            return Err(bad(id, "shared master/VCA processing must be consolidated before stem publication"));
        }
    }
    fs::create_dir_all(dir).map_err(|err| EngineError::Io(err.to_string()))?;
    if fs::read_dir(dir).map_err(|err| EngineError::Io(err.to_string()))?.next().is_some() {
        return Err(bad(id, "output directory must be empty"));
    }
    let mut files = Vec::new();
    for track in tracks {
        let mut isolated = s.clone();
        isolated.outputs = vec![OutputPath { name: "Bridge".into(), first_channel: 0, format: ChannelFormat::Stereo }];
        for t in &mut isolated.tracks {
            t.mixer.solo = false;
            t.mixer.solo_safe = false;
            t.mixer.vca = None;
            if t.id == track.id {
                t.mixer.output = Route::Main;
                t.mixer.mute = false;
                // Mono physical stems do not encode a stereo monitor pan law.
                if t.channels() == 1 {
                    t.mixer.pan = vec![0.0];
                    t.automation.retain(|lane| !matches!(lane.param, soundcraft_model::AutoParam::Pan(_)));
                }
            } else {
                t.inactive = true;
            }
        }
        let mut channels = soundcraft_mix::render_range(&isolated, Range::new(0, frames), 1024);
        if track.channels() == 1 {
            let left = channels.first().ok_or_else(|| bad(id, "renderer returned no audio"))?;
            channels = vec![left.iter().map(|x| x * std::f32::consts::SQRT_2).collect()];
        }
        if channels.len() != track.channels() || channels.iter().any(|c| c.len() != frames as usize) {
            return Err(bad(id, "incomplete render; publication aborted"));
        }
        let opts = soundcraft_audio_io::EncodeOptions {
            format: soundcraft_audio_io::FileFormat::Wav,
            bit_depth: soundcraft_audio_io::BitDepth::Float32,
            dither: false,
            bwf: None,
        };
        let audio = soundcraft_audio_io::AudioBuffer { sample_rate: s.sample_rate.hz(), channels };
        let bytes = soundcraft_audio_io::encode(&audio, &opts).map_err(|err| EngineError::Io(err.to_string()))?;
        // IDs prevent identical/normalized track names from overwriting another stem.
        let path = Path::new(dir).join(format!("track-{}.wav", track.id.0));
        fs::write(&path, bytes).map_err(|err| EngineError::Io(err.to_string()))?;
        files.push(json!({"trackID": track.id.0, "path": path, "channels": track.channels()}));
    }
    Ok(json!({"files": files}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sculpt_bridge_renders_duplicate_names_muted_mono_and_stereo_without_mutating_session() {
        let mut engine = Engine::default();
        for channels in [1, 2] {
            let buffer = soundcraft_audio_io::AudioBuffer { sample_rate: 48000, channels: vec![vec![0.25; 4800]; channels] };
            let bytes = soundcraft_audio_io::encode(&buffer, &soundcraft_audio_io::EncodeOptions::default()).unwrap();
            crate::io::import_audio_bytes(&mut engine, "Same", &bytes, None, None, 0).unwrap();
        }
        engine.session_mut().tracks[0].mixer.mute = true;
        let before = engine.session().clone();
        let dir = std::env::temp_dir().join(format!("sculpt-bridge-render-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let result = engine.execute("bridge.render_stems", &json!({"dir": dir})).unwrap();
        let files = result["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_ne!(files[0]["path"], files[1]["path"]);
        for (i, file) in files.iter().enumerate() {
            let bytes = fs::read(file["path"].as_str().unwrap()).unwrap();
            let audio = soundcraft_audio_io::decode(&bytes, Some("wav")).unwrap().1;
            assert_eq!(audio.channels.len(), i + 1);
            assert_eq!(audio.channels[0].len(), 4800);
            assert!((audio.channels[0][2000] - 0.25).abs() < 0.001);
        }
        assert_eq!(engine.session().to_json().unwrap(), before.to_json().unwrap());
        assert!(engine.execute("bridge.render_stems", &json!({"dir": dir})).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
