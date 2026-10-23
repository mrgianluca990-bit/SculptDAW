//! Clip group files (`.scgrp`): clips exported from a session (Clip List › Export Clip Groups)
//! and imported into another (File › Import › Clip Groups…).
//!
//! The file is JSON: clips per lane with starts relative to the group, plus the audio they play
//! as 32-bit float WAVs in a `<name> Audio/` folder next to it.

use crate::{Engine, EngineError, Result};
use serde::{Deserialize, Serialize};
#[cfg(target_arch = "wasm32")]
use soundcraft_audio_io::EncodeOptions;
use soundcraft_audio_io::FileFormat;
use soundcraft_model::{Clip, ClipContent, ClipId, SourceId, TrackId};
use soundcraft_time::Samples;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const EXTENSION: &str = "scgrp";
const FORMAT: &str = "soundcraft.clipgroup";
/// Refuse files that would place absurd numbers of clips.
const MAX_CLIPS: usize = 10_000;

#[derive(Debug, Serialize, Deserialize)]
struct GroupFile {
    format: String,
    version: u32,
    sample_rate: u32,
    length: Samples,
    /// Clips per lane (one lane per source track), starts relative to the group start.
    lanes: Vec<Vec<Clip>>,
    /// Source id (as written) → audio file path relative to the group file.
    audio: BTreeMap<u64, String>,
}

/// Write the clips `ids` (grouped by track) to `path`. Returns the number of clips written.
pub fn export(e: &Engine, ids: &[ClipId], path: &str) -> Result<usize> {
    if ids.len() > MAX_CLIPS {
        return Err(EngineError::BadParams("file.export_clip_groups".into(), "too many clips".into()));
    }
    let s = e.session();
    let mut lanes: Vec<(usize, Vec<Clip>)> = Vec::new();
    for id in ids {
        let Some((t, c)) = s.find_clip(*id) else { continue };
        let ti = s.tracks.iter().position(|x| x.id == t).unwrap_or(0);
        match lanes.iter_mut().find(|(i, _)| *i == ti) {
            Some((_, v)) => v.push(c.clone()),
            None => lanes.push((ti, vec![c.clone()])),
        }
    }
    let clips: Vec<&Clip> = lanes.iter().flat_map(|(_, v)| v.iter()).collect();
    let (Some(start), Some(end)) = (clips.iter().map(|c| c.start).min(), clips.iter().map(|c| c.start.saturating_add(c.length)).max()) else {
        return Err(EngineError::BadParams("file.export_clip_groups".into(), "no clips to export".into()));
    };
    let length = end
        .checked_sub(start)
        .filter(|n| *n > 0)
        .ok_or_else(|| EngineError::BadParams("file.export_clip_groups".into(), "clip group duration overflow".into()))?;
    let n = clips.len();
    lanes.sort_by_key(|(i, _)| *i);
    let p = with_extension(path);
    let stem = p.file_stem().and_then(|x| x.to_str()).unwrap_or("Clip Group").to_string();
    let audio_dir_name = format!("{} Audio", crate::io::sanitize_name(&stem));
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut audio = BTreeMap::new();
    let mut out_lanes = Vec::new();
    for (_, v) in lanes {
        let mut lane = Vec::new();
        for mut c in v {
            c.start =
                c.start.checked_sub(start).ok_or_else(|| EngineError::BadParams("file.export_clip_groups".into(), "clip start overflow".into()))?;
            if let ClipContent::Audio { source, .. } = c.content
                && let std::collections::btree_map::Entry::Vacant(slot) = audio.entry(source.0)
            {
                let buf = s.pool.get(source).ok_or_else(|| EngineError::Io(format!("clip group source {} is missing", source.0)))?;
                let src = s.source(source).ok_or_else(|| EngineError::Io(format!("clip group source {} has no metadata", source.0)))?;
                let stem = format!("{}_{}", crate::io::sanitize_name(&src.name), source.0);
                let audio_dir = dir.join(&audio_dir_name);
                std::fs::create_dir_all(&audio_dir).map_err(|err| EngineError::Io(format!("{audio_dir_name}: {err}")))?;
                #[cfg(not(target_arch = "wasm32"))]
                let name = crate::io::store_source_wav(&audio_dir, &stem, buf)?.0;
                #[cfg(target_arch = "wasm32")]
                let name = {
                    let name = format!("{stem}.wav");
                    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: soundcraft_audio_io::BitDepth::Float32, dither: false, bwf: None };
                    let bytes = soundcraft_audio_io::encode(&buf.buffer, &opts).map_err(|err| EngineError::Io(err.to_string()))?;
                    std::fs::write(audio_dir.join(&name), bytes).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
                    name
                };
                slot.insert(format!("{audio_dir_name}/{name}"));
            }
            lane.push(c);
        }
        out_lanes.push(lane);
    }
    let file = GroupFile { format: FORMAT.into(), version: 1, sample_rate: s.sample_rate.hz(), length, lanes: out_lanes, audio };
    let text = serde_json::to_string_pretty(&file).map_err(|err| EngineError::Io(err.to_string()))?;
    std::fs::write(&p, text).map_err(|err| EngineError::Io(format!("{}: {err}", p.display())))?;
    Ok(n)
}

/// Import a clip group file at `at`, one lane per track starting at `first` (new audio tracks
/// are made when the session runs out). Returns the ids of the placed clips.
pub fn import(e: &mut Engine, path: &str, first: Option<TrackId>, at: Samples) -> Result<Vec<ClipId>> {
    let id = "file.import_clip_groups";
    use std::io::Read;
    const MAX_GROUP_JSON: u64 = 64 * 1024 * 1024;
    let input = std::fs::File::open(path).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    let mut text = String::new();
    input.take(MAX_GROUP_JSON + 1).read_to_string(&mut text).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    if text.len() as u64 > MAX_GROUP_JSON {
        return Err(EngineError::BadParams(id.into(), "clip group JSON is too large".into()));
    }
    let mut file: GroupFile =
        serde_json::from_str(&text).map_err(|err| EngineError::BadParams(id.into(), format!("not a clip group file: {err}")))?;
    if file.format != FORMAT || file.version != 1 || file.sample_rate == 0 {
        return Err(EngineError::BadParams(id.into(), "not a SoundCraft clip group file".into()));
    }
    if file.lanes.iter().map(Vec::len).sum::<usize>() > MAX_CLIPS {
        return Err(EngineError::BadParams(id.into(), "clip group file has too many clips".into()));
    }
    if file.lanes.iter().flatten().any(|clip| matches!(clip.content, ClipContent::Audio { source, .. } if !file.audio.contains_key(&source.0))) {
        return Err(EngineError::BadParams(id.into(), "clip group is missing source references".into()));
    }
    let destination_rate = e.session().sample_rate.hz();
    for lane in &mut file.lanes {
        for clip in lane {
            if clip.start < 0 || clip.length <= 0 || clip.fade_in.len < 0 || clip.fade_out.len < 0 {
                return Err(EngineError::BadParams(id.into(), "invalid clip group timing".into()));
            }
            clip.start = at
                .checked_add(scale_time(clip.start, file.sample_rate, destination_rate)?)
                .ok_or_else(|| EngineError::BadParams(id.into(), "clip group placement overflow".into()))?;
            clip.length = scale_time(clip.length, file.sample_rate, destination_rate)?.max(1);
            if clip.start.checked_add(clip.length).is_none() {
                return Err(EngineError::BadParams(id.into(), "clip group end overflow".into()));
            }
            clip.fade_in.len = scale_time(clip.fade_in.len, file.sample_rate, destination_rate)?;
            clip.fade_out.len = scale_time(clip.fade_out.len, file.sample_rate, destination_rate)?;
            clip.sync_point = scale_time(clip.sync_point, file.sample_rate, destination_rate)?;
            for point in &mut clip.gain_env {
                point.0 = scale_time(point.0, file.sample_rate, destination_rate)?;
            }
            clip.gain_env.sort_by_key(|point| point.0);
            clip.gain_env.dedup_by(|a, b| {
                if a.0 == b.0 {
                    b.1 = a.1;
                    true
                } else {
                    false
                }
            });
            if let ClipContent::Audio { offset, .. } = &mut clip.content {
                *offset = scale_time(*offset, file.sample_rate, destination_rate)?;
            }
            clip.clamp_fades();
        }
    }
    let dir = Path::new(path).parent().map(Path::to_path_buf).unwrap_or_default();
    // Decode audio first so a bad file changes nothing.
    let mut decoded = BTreeMap::new();
    for (old, rel) in &file.audio {
        let full = dir.join(rel);
        if !full.starts_with(&dir) || rel.contains("..") {
            return Err(EngineError::BadParams(id.into(), format!("audio path escapes the group folder: {rel}")));
        }
        let bytes = std::fs::read(&full).map_err(|err| EngineError::Io(format!("{}: {err}", full.display())))?;
        let (_, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|err| EngineError::Io(format!("{rel}: {err}")))?;
        if buf.sample_rate != file.sample_rate {
            return Err(EngineError::BadParams(id.into(), format!("{rel}: rate differs from clip group")));
        }
        decoded.insert(*old, (rel.clone(), buf));
    }
    let s = e.session_mut();
    let mut map: BTreeMap<u64, SourceId> = BTreeMap::new();
    for (old, (rel, buf)) in decoded {
        let name = Path::new(&rel).file_name().and_then(|x| x.to_str()).unwrap_or("clip group").to_string();
        map.insert(old, crate::io::add_source(s, &name, buf, None, FileFormat::Wav));
    }
    let start_idx = first.and_then(|t| s.tracks.iter().position(|x| x.id == t));
    let mut placed = Vec::new();
    let group_id = s.alloc();
    for (li, lane) in file.lanes.into_iter().enumerate() {
        let audio_lane = lane.iter().any(|c| matches!(c.content, ClipContent::Audio { .. }));
        let target = start_idx.and_then(|i| s.tracks.get(i + li)).map(|t| t.id);
        let target = match target {
            Some(t) => t,
            None => {
                let channels = lane
                    .iter()
                    .find_map(|c| match c.content {
                        ClipContent::Audio { source, .. } => map.get(&source.0).and_then(|n| s.pool.get(*n)).map(|a| a.channels()),
                        _ => None,
                    })
                    .unwrap_or(2);
                let kind = if audio_lane { soundcraft_model::TrackKind::Audio } else { soundcraft_model::TrackKind::Midi };
                let fmt = soundcraft_model::ChannelFormat::for_channels(channels);
                s.add_track(kind, fmt, Some("Clip Group"))
            }
        };
        let mut imported = Vec::new();
        for mut c in lane {
            if let ClipContent::Audio { source, .. } = &mut c.content {
                let Some(n) = map.get(&source.0) else { continue };
                *source = *n;
            } else if !matches!(c.content, ClipContent::Midi { .. }) {
                continue;
            }
            c.id = s.new_clip_id();
            c.group = Some(group_id);
            c.clamp_fades();
            placed.push(c.id);
            imported.push(c);
        }
        // Replace old material first, then append the entire lane together. Sequential
        // place_clip calls would trim overlaps between clips belonging to this same group.
        for clip in &imported {
            crate::edit::clear_range(s, target, clip.range(), false);
        }
        if let Some(playlist) = s.track_mut(target).and_then(|track| track.playlist_mut()) {
            playlist.clips.extend(imported);
            playlist.sort();
        }
    }
    Ok(placed)
}

fn scale_time(value: Samples, from: u32, to: u32) -> Result<Samples> {
    let numerator = i128::from(value) * i128::from(to);
    let denominator = i128::from(from);
    let rounded = if numerator >= 0 { (numerator + denominator / 2) / denominator } else { (numerator - denominator / 2) / denominator };
    Samples::try_from(rounded).map_err(|_| EngineError::BadParams("file.import_clip_groups".into(), "sample-rate timing overflow".into()))
}

fn with_extension(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.extension().is_none() { p.with_extension(EXTENSION) } else { p }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn invalid_group_rate_and_timing_fail_before_document_changes() {
        use super::*;
        let dir = std::env::temp_dir().join(format!("sculpt-group-invalid-time-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Invalid.scgrp");
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let before = e.session_arc();
        let history = e.undo_history();
        let clip = Clip::midi(ClipId(1), "Overflow", i64::MAX - 1, 1, soundcraft_midi::Sequence::default());
        for (version, rate) in [(2, 48_000), (1, 0), (1, 1)] {
            let file = GroupFile {
                format: FORMAT.into(),
                version,
                sample_rate: rate,
                length: i64::MAX,
                lanes: vec![vec![clip.clone()]],
                audio: BTreeMap::new(),
            };
            std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
            assert!(import(&mut e, &path.to_string_lossy(), None, 0).is_err());
            assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()));
            assert_eq!(e.undo_history(), history);
        }
        assert_eq!(scale_time(88_200, 88_200, 48_000).unwrap(), 48_000);
        assert_eq!(scale_time(-3, 96_000, 48_000).unwrap(), -2);
        assert!(scale_time(i64::MAX, 1, 192_000).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn disk_group_export_reuses_audio_preserves_collisions_and_keeps_json_on_failure() {
        use super::*;
        use soundcraft_audio_io::{
            AudioBuffer, BitDepth, EncodeOptions,
            stream_cache::{CacheConfig, StreamCache},
        };
        use std::{sync::Arc, time::Duration};
        let dir = std::env::temp_dir().join(format!("sculpt-group-disk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("input.wav");
        let buffer = AudioBuffer { sample_rate: 48_000, channels: vec![vec![1.5; 6000]; 7] };
        let bytes = soundcraft_audio_io::encode(&buffer, &EncodeOptions { bit_depth: BitDepth::Float32, ..EncodeOptions::default() }).unwrap();
        std::fs::write(&original, &bytes).unwrap();
        let cache = StreamCache::new(CacheConfig::default()).unwrap();
        let mut e = Engine::default();
        crate::io::import_audio_bytes(&mut e, "Seven", &bytes, None, None, 0).unwrap();
        let source = e.session().sources[0].id;
        let audio = soundcraft_model::SourceAudio::from_stream(cache.open_source(&original).unwrap(), 140, |_, _| true).unwrap();
        e.session_mut().pool.insert(source, Arc::new(audio));
        let clip = e.session().tracks[0].clips()[0].id;
        let audio_dir = dir.join("Group Audio");
        std::fs::create_dir(&audio_dir).unwrap();
        let collision = audio_dir.join(format!("Seven_{}.wav", source.0));
        std::fs::write(&collision, b"previous unrelated media").unwrap();
        let path = dir.join("Group.scgrp");
        let before = e.session_arc();
        export(&e, &[clip], &path.to_string_lossy()).unwrap();
        let text = std::fs::read(&path).unwrap();
        let file: GroupFile = serde_json::from_slice(&text).unwrap();
        let copied = std::fs::read(dir.join(file.audio.get(&source.0).unwrap())).unwrap();
        assert_eq!(copied, bytes);
        assert_eq!(std::fs::read(&collision).unwrap(), b"previous unrelated media");
        export(&e, &[clip], &path.to_string_lossy()).unwrap();
        assert_eq!(std::fs::read_dir(&audio_dir).unwrap().count(), 2);
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        std::fs::OpenOptions::new().write(true).open(original).unwrap().set_len(100).unwrap();
        assert!(export(&e, &[clip], &path.to_string_lossy()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), text);
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        cache.shutdown(Duration::from_secs(1)).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn multichannel_group_import_preserves_overlap_and_scales_all_sample_times() {
        use super::*;
        use soundcraft_model::{ChannelFormat, TrackKind};
        let dir = std::env::temp_dir().join(format!("sculpt-group-time-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Group.scgrp");
        let mut e = Engine::default();
        let s = e.session_mut();
        s.sample_rate = soundcraft_time::SampleRate::new(96_000).unwrap();
        let source = crate::io::add_source(s, "Seven", soundcraft_audio_io::AudioBuffer::new(96_000, 7, 8000), None, FileFormat::Wav);
        let track = s.add_track(TrackKind::Audio, ChannelFormat::for_channels(7), None);
        let mut ids = Vec::new();
        for start in [0, 2000] {
            let mut clip = Clip::audio(s.new_clip_id(), "Overlap", source, 400, start, 4000);
            clip.fade_in.len = 800;
            clip.fade_out.len = 600;
            clip.sync_point = 1000;
            clip.gain_env = vec![(0, 0.0), (2000, -6.0), (4000, 0.0)];
            ids.push(clip.id);
            s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
        }
        export(&e, &ids, &path.to_string_lossy()).unwrap();
        let mut destination = Engine::default();
        let result = destination.execute("file.import_clip_groups", &json!({"path":path, "at":10_000})).unwrap();
        let placed: Vec<_> = result["clips"].as_array().unwrap().iter().map(|id| ClipId(id.as_u64().unwrap())).collect();
        assert_eq!(destination.session().tracks[0].channels(), 7);
        assert_eq!(placed.len(), 2);
        for (i, id) in placed.iter().enumerate() {
            let (_, clip) = destination.session().find_clip(*id).unwrap();
            assert_eq!(clip.start, 10_000 + i as i64 * 1000);
            assert_eq!(clip.length, 2000);
            assert_eq!(clip.fade_in.len, 400);
            assert_eq!(clip.fade_out.len, 300);
            assert_eq!(clip.sync_point, 500);
            assert_eq!(clip.gain_env, vec![(0, 0.0), (1000, -6.0), (2000, 0.0)]);
            assert!(matches!(clip.content, ClipContent::Audio { offset: 200, .. }));
        }
        assert!(destination.undo());
        assert!(destination.session().tracks.is_empty());
        assert!(destination.redo());
        assert!(placed.iter().all(|id| destination.session().find_clip(*id).is_some()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_then_import_round_trips_audio_clips() {
        let dir = std::env::temp_dir().join(format!("sc-grp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("drums.scgrp");
        let path = path.to_string_lossy().to_string();
        let mut e = crate::demo::demo_engine();
        let kick = e.session().tracks.iter().find(|t| t.name == "Kick").map(|t| t.id).unwrap();
        let ids: Vec<_> = e.session().track(kick).unwrap().clips().iter().take(2).map(|c| c.id).collect();
        let r = e.execute("file.export_clip_groups", &json!({"path": path, "clips": ids.iter().map(|c| c.0).collect::<Vec<_>>()})).unwrap();
        assert_eq!(r["clips"], 2);
        let before = e.session().tracks.len();
        let at = 30_000_000;
        let r = e.execute("file.import_clip_groups", &json!({"path": path, "at": at, "track": "Bass"})).unwrap();
        let placed: Vec<_> = r["clips"].as_array().unwrap().iter().filter_map(serde_json::Value::as_u64).collect();
        assert_eq!(placed.len(), 2);
        let bass = e.session().tracks.iter().find(|t| t.name == "Bass").map(|t| t.id).unwrap();
        for id in &placed {
            let (t, c) = e.session().find_clip(soundcraft_model::ClipId(*id)).unwrap();
            assert_eq!(t, bass);
            assert!(c.start >= at && c.group.is_some());
            assert!(matches!(c.content, soundcraft_model::ClipContent::Audio { source, .. } if e.session().pool.contains(source)));
        }
        e.undo();
        assert_eq!(e.session().tracks.len(), before);
        assert!(e.session().find_clip(soundcraft_model::ClipId(placed[0])).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_foreign_files_and_escaping_paths() {
        let dir = std::env::temp_dir().join(format!("sc-grp-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("x.scgrp");
        std::fs::write(
            &p,
            r#"{"format":"soundcraft.clipgroup","version":1,"sample_rate":48000,"length":1,"lanes":[],"audio":{"1":"../../etc/passwd"}}"#,
        )
        .unwrap();
        let mut e = crate::demo::demo_engine();
        assert!(e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).is_err());
        std::fs::write(&p, "{}").unwrap();
        assert!(e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
