//! Sculpt DAW -> Sculpt Show v1 handoff preparation.
//! Never claims Show has ingested anything. The Show-side receiver is separate.
//! Original session/audio and origin manifest are never overwritten.
use crate::SoundApp;
use serde_json::{Value, json};
use soundcraft_model::{Route, Track, TrackKind};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fresh_uuid() -> Result<String, String> {
    // macOS supplies uuidgen, so no new Rust/Cargo.lock dependencies are introduced.
    let output = Command::new("/usr/bin/uuidgen").output().map_err(|e| format!("uuidgen: {e}"))?;
    if !output.status.success() {
        return Err("Unable to create an identifier for this cue".into());
    }
    let s = String::from_utf8(output.stdout).map_err(|e| e.to_string())?.trim().to_owned();
    if !valid_uuid(&s) {
        return Err("uuidgen returned an invalid identifier".into());
    }
    Ok(s)
}
fn valid_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}
fn get_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| format!("Show origin manifest is missing {key}"))
}
fn source_stem_uuid(track: &Track) -> Option<String> {
    let suffix = track.comments.split("Source stem UUID: ").nth(1)?;
    let uuid = suffix.trim().split_whitespace().next()?;
    valid_uuid(uuid).then(|| uuid.to_owned())
}
fn original_stem<'a>(manifest: &'a Value, track: &Track) -> Option<&'a Value> {
    let id = source_stem_uuid(track)?;
    manifest.get("stems")?.as_array()?.iter().find(|v| v.get("elementID").and_then(Value::as_str).is_some_and(|x| x.eq_ignore_ascii_case(&id)))
}
fn physical_output(track: &Track, session: &soundcraft_model::Session, old: Option<&Value>) -> usize {
    if let Route::Hardware(name) = &track.mixer.output {
        if let Some(out) = session.outputs.iter().find(|o| &o.name == name) {
            return usize::from(out.first_channel) + 1;
        }
    }
    old.and_then(|o| o.get("outputStartChannel")).and_then(Value::as_u64).map_or(1, |c| usize::try_from(c).unwrap_or(1))
}
fn clean_dir_name(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(55).collect();
    if s.is_empty() { "Cue".into() } else { s }
}
fn publish_folder(project: &Path, id: &str) -> Result<PathBuf, String> {
    // Every attempt is isolated so no previous exported WAV/manifest can be overwritten.
    let unique = fresh_uuid()?;
    let root = project.join("Sculpt Publish to Show").join(format!("{}-{}", clean_dir_name(id), unique));
    fs::create_dir_all(root.join("Audio")).map_err(|e| format!("Create publish workspace: {e}"))?;
    Ok(root)
}
fn value_array<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    v.get(key).and_then(Value::as_array).ok_or_else(|| format!("Malformed Show manifest: {key}"))
}
fn write_atomic(path: &Path, body: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body).map_err(|e| format!("Write manifest: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("Finalize manifest: {e}"))
}

/// Stage a return package, retaining IDs for update or creating fresh IDs for a new cue.
/// A Show receiver still needs to ingest it; this function never changes a Show project.
fn prepare_publication(app: &mut SoundApp, new_cue: bool) -> Result<Value, String> {
    if app.engine.transport.playing || app.engine.transport.recording {
        return Err("Stop playback/recording before publishing to Show".into());
    }
    if app.engine.is_dirty() {
        return Err("Save your Sculpt DAW session before publishing to Show".into());
    }
    // Own the path: capture_plugin_states and file.bounce_stems mutate SoundApp later.
    // Keeping a borrow of app.engine.path here would fail Rust's borrow checker.
    let current = app.engine.path.clone().ok_or("Save your session before publishing to Show")?;
    let current = fs::canonicalize(&current).map_err(|e| format!("Locate saved session: {e}"))?.to_string_lossy().into_owned();
    let session_file = Path::new(&current);
    let session_dir = session_file.parent().ok_or("The saved session has no parent folder")?;
    let origin_path = session_dir.join("sculpt-cue-bridge.json");
    let mut origin: Value = match fs::read(&origin_path) {
        Ok(data) => serde_json::from_slice(&data).map_err(|e| format!("Invalid Show bridge manifest: {e}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && new_cue => json!({
            "format":"com.sculpt.bridge.cue","version":1,"showName":"DAW publication",
            "cueListID":fresh_uuid()?,"cueID":fresh_uuid()?,"cueName":app.engine.session().name,
            "durationSeconds":0.0,"stems":[],"markers":[],"timecode":null,"targetSelection":true
        }),
        Err(_) => return Err("No Show origin found. Use Publish in Show as a New Cue for an independent DAW session.".into()),
    };
    if get_str(&origin, "format")? != "com.sculpt.bridge.cue" || origin.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("The session does not contain a supported Sculpt Show v1 manifest".into());
    }
    let _show_name = get_str(&origin, "showName")?;
    let cue_list = get_str(&origin, "cueListID")?;
    if !valid_uuid(cue_list) {
        return Err("Invalid cue list identifier in Show manifest".into());
    }
    let old_cue = get_str(&origin, "cueID")?;
    if !valid_uuid(old_cue) {
        return Err("Invalid cue identifier in Show manifest".into());
    }
    let old_cue = old_cue.to_owned();
    let _ = value_array(&origin, "stems")?;
    let prior_markers = value_array(&origin, "markers")?.clone();
    // Repeated updates use the previously acknowledged cue revision.
    let last_path = session_dir.join("sculpt-last-publish.json");
    let mut previous_tracks = json!({});
    if let Ok(bytes) = fs::read(&last_path) {
        if let Ok(last) = serde_json::from_slice::<Value>(&bytes) {
            if last.get("mode").and_then(Value::as_str) == Some("updateCue")
                && last.get("cueID").and_then(Value::as_str).is_some_and(|id| id.eq_ignore_ascii_case(&old_cue))
            {
                previous_tracks = last.get("trackElements").cloned().unwrap_or_else(|| json!({}));
            }
            if let Some(folder) = last.get("folder").and_then(Value::as_str) {
                let receipt = Path::new(folder).join("receipt.json");
                if let Ok(bytes) = fs::read(receipt) {
                    if let Ok(record) = serde_json::from_slice::<Value>(&bytes) {
                        if record.get("status").and_then(Value::as_str) == Some("applied")
                            && record.get("publishMode").and_then(Value::as_str) == Some("updateCue")
                            && record.get("cueID").and_then(Value::as_str).is_some_and(|v| v.eq_ignore_ascii_case(&old_cue))
                            && record.get("publicationID") == last.get("publicationID")
                        {
                            origin["originCue"] = record["originCue"].clone();
                        }
                    }
                }
            }
        }
    }
    let s = app.engine.session().clone();
    let tracks: Vec<_> = s.tracks.iter().filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi)).collect();
    if tracks.is_empty() {
        return Err("No audio or instrument tracks to publish".into());
    }
    if tracks.iter().any(|t| t.inactive) {
        return Err("Inactive tracks cannot be published; activate them first".into());
    }
    if tracks.len() > 64 {
        return Err("Show bridge supports no more than 64 stems".into());
    }
    let original_duration = origin.get("durationSeconds").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0);
    let marker_end = s.markers.iter().map(|m| m.start.max(0)).max().unwrap_or(0) as f64 / s.sample_rate.hz() as f64;
    let duration = (s.content_end().max(1) as f64 / s.sample_rate.hz() as f64).max(original_duration).max(marker_end);
    if duration > 4.0 * 3600.0 {
        return Err("Publishing sessions longer than 4 hours is not supported".into());
    }
    let destination_id = if new_cue { fresh_uuid()? } else { old_cue.clone() };
    let folder = publish_folder(session_dir, &destination_id)?;
    let audio_dir = folder.join("Audio");

    // Use the established offline stem renderer, not the physical playback outputs.
    // It renders all stems from time 0 to the same session end, with inserts/edits/fades.
    app.capture_plugin_states();
    let rendered = app.run("bridge.render_stems", json!({"dir":audio_dir.to_string_lossy().into_owned(),"end_seconds":duration}))?;
    let paths = value_array(&rendered, "files")?;
    if paths.len() != tracks.len() {
        return Err(format!("Incomplete offline export: {} file(s) for {} track(s)", paths.len(), tracks.len()));
    }
    let mut stems = Vec::<Value>::new();
    let mut track_elements = json!({});
    let mut used_elements = std::collections::HashSet::<String>::new();
    for track in tracks.iter() {
        let rendered =
            paths.iter().find(|v| v.get("trackID").and_then(Value::as_u64) == Some(track.id.0)).ok_or("Missing rendered track identifier")?;
        let wav = rendered.get("path").and_then(Value::as_str).ok_or("Bounce returned an invalid path")?;
        if !Path::new(wav).is_file() {
            return Err(format!("Missing bounce file: {wav}"));
        }
        let old = original_stem(&origin, track);
        let track_key = track.id.0.to_string();
        let prior = previous_tracks.get(&track_key).and_then(Value::as_str).or_else(|| old.and_then(|v| v.get("elementID")).and_then(Value::as_str));
        let element_id = match prior {
            Some(id) if !new_cue && valid_uuid(id) && !used_elements.contains(&id.to_ascii_uppercase()) => id.to_owned(),
            _ => fresh_uuid()?,
        };
        used_elements.insert(element_id.to_ascii_uppercase());
        track_elements[&track_key] = json!(element_id.clone());
        let first = physical_output(track, &s, old);
        if first < 1 || first + track.channels() - 1 > 64 {
            return Err(format!("{}: bounce cannot start at hardware channel {}", track.name, first));
        }
        stems.push(json!({
            "elementID":element_id,"name":track.name,"sourcePath":wav,
            "outputStartChannel":first,"gain":1.0,"muted":track.mixer.mute
        }));
    }
    let mut markers = Vec::<Value>::new();
    let mut used_markers = std::collections::HashSet::<String>::new();
    for m in &s.markers {
        if m.kind != soundcraft_model::MarkerKind::Marker {
            continue;
        }
        let marker_id = m.comments.strip_prefix("Sculpt Show marker UUID: ").filter(|id| valid_uuid(id));
        let old = prior_markers.iter().find(|v| {
            if let Some(id) = marker_id {
                v.get("id").and_then(Value::as_str).is_some_and(|v| v.eq_ignore_ascii_case(id))
            } else {
                v.get("name").and_then(Value::as_str) == Some(m.name.as_str())
            }
        });
        let id = if new_cue {
            fresh_uuid()?
        } else {
            match old.and_then(|v| v.get("id")).and_then(Value::as_str) {
                Some(id) if valid_uuid(id) && !used_markers.contains(id) => id.to_owned(),
                _ => fresh_uuid()?,
            }
        };
        used_markers.insert(id.clone());
        markers.push(json!({"id":id,"name":m.name,
            "seconds":(m.start.max(0) as f64)/(s.sample_rate.hz() as f64)}));
    }
    let cue_name = if new_cue { format!("{} (New Cue)", s.name) } else { s.name.clone() };
    let stems_count = stems.len();
    let markers_count = markers.len();
    origin["cueID"] = json!(destination_id.clone());
    origin["cueName"] = json!(cue_name);
    origin["durationSeconds"] = json!(duration);
    origin["stems"] = json!(stems);
    origin["markers"] = json!(markers);
    let publication_id = fresh_uuid()?;
    origin["publicationID"] = json!(publication_id.clone());
    origin["publishMode"] = json!(if new_cue { "newCue" } else { "updateCue" });
    origin["sessionSampleRate"] = json!(s.sample_rate.hz());
    origin["originSessionPath"] = json!(current);
    origin["outboundStatus"] = json!("prepared-not-applied");
    // Never modify the inbound show origin. Receiver will verify cue/list IDs.
    let manifest = folder.join("publication.sculptbridge");
    let body = serde_json::to_vec_pretty(&origin).map_err(|e| e.to_string())?;
    write_atomic(&manifest, &body)?;
    let pointer = json!({"folder":folder,"publicationID":publication_id,"cueID":old_cue,
        "mode":if new_cue{"newCue"}else{"updateCue"},"trackElements":track_elements});
    write_atomic(&last_path, &serde_json::to_vec(&pointer).map_err(|e| e.to_string())?)?;
    Ok(json!({"prepared":true,"delivered":false,"showApplied":false,
        "publicationID":publication_id,"manifest":manifest,"cueID":destination_id,
        "stems":stems_count,"markers":markers_count}))
}

/// UI-only delivery follows the same testable preparation path.
pub fn publish(app: &mut SoundApp, new_cue: bool) -> Result<Value, String> {
    let mut result = prepare_publication(app, new_cue)?;
    let path = result.get("manifest").and_then(Value::as_str).ok_or("Publication has no manifest path")?;
    let delivered =
        Command::new("/usr/bin/open").args(["-b", "com.gianlucavergalito.sculptshow"]).arg(path).status().is_ok_and(|status| status.success());
    app.ui.status = if delivered {
        "Publication sent to Show. Show will confirm application or queue until cue stop.".into()
    } else {
        format!("Publication ready; in Sculpt Show choose Receive DAW Publication: {path}")
    };
    result["delivered"] = json!(delivered);
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn sculpt_bridge_prepares_saved_session_with_real_stems_and_extended_marker_timing() {
        let mut engine = soundcraft_engine::Engine::default();
        for channels in [1, 2] {
            let audio = soundcraft_audio_io::AudioBuffer { sample_rate: 48000, channels: vec![vec![0.25; 4800]; channels] };
            let bytes = soundcraft_audio_io::encode(&audio, &soundcraft_audio_io::EncodeOptions::default()).unwrap();
            soundcraft_engine::io::import_audio_bytes(&mut engine, "Same", &bytes, None, None, 0).unwrap();
        }
        engine.session_mut().tracks[0].mixer.mute = true;
        engine.session_mut().tracks[0].comments = "Source stem UUID: 33333333-3333-3333-3333-333333333333".into();
        engine.session_mut().tracks[1].comments = "Source stem UUID: 44444444-4444-4444-4444-444444444444".into();
        engine
            .execute(
                "markers.add",
                &json!({"name":"Renamed","at":{"seconds":0.2},
            "comments":"Sculpt Show marker UUID: 55555555-5555-5555-5555-555555555555"}),
            )
            .unwrap();
        let dir = std::env::temp_dir().join(format!("sculpt-publish-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        engine.execute("session.save", &json!({"path":dir.join("Test.scdaw")})).unwrap();
        let mut app = SoundApp::new(engine, None, crate::Services::default());
        let first = prepare_publication(&mut app, true).unwrap();
        let path = first["manifest"].as_str().unwrap();
        let packet: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(packet["stems"].as_array().unwrap().len(), 2);
        assert_eq!(packet["stems"][0]["muted"], true);
        assert_eq!(packet["durationSeconds"], 0.2);
        assert_eq!(packet["targetSelection"], true);
        assert_eq!(packet["markers"][0]["name"], "Renamed");
        let mut paths = std::collections::HashSet::new();
        for (i, stem) in packet["stems"].as_array().unwrap().iter().enumerate() {
            let path = stem["sourcePath"].as_str().unwrap();
            assert!(paths.insert(path));
            let audio = soundcraft_audio_io::decode(&fs::read(path).unwrap(), Some("wav")).unwrap().1;
            assert_eq!(audio.channels.len(), i + 1);
            assert_eq!(audio.channels[0].len(), 9600);
        }
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bridge/fixtures/origin-v1.json");
        let validate = |manifest: &str| {
            if let Ok(tool) = std::env::var("SCULPT_BRIDGE_VALIDATOR") {
                let output = Command::new(tool).arg(manifest).arg(&fixture_path).output().unwrap();
                assert!(
                    output.status.success(),
                    "Swift receiver rejected actual DAW output: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        };
        validate(path);
        // An independent session cannot update an unspecified Show cue.
        assert!(prepare_publication(&mut app, false).is_err());
        let second = prepare_publication(&mut app, true).unwrap();
        assert_ne!(first["publicationID"], second["publicationID"]);
        assert_ne!(first["cueID"], second["cueID"]);
        fs::copy(&fixture_path, dir.join("sculpt-cue-bridge.json")).unwrap();
        let update = prepare_publication(&mut app, false).unwrap();
        let manifest = update["manifest"].as_str().unwrap();
        let packet: Value = serde_json::from_slice(&fs::read(manifest).unwrap()).unwrap();
        assert_eq!(packet["cueID"], "22222222-2222-2222-2222-222222222222");
        assert_eq!(packet["markers"][0]["id"], "55555555-5555-5555-5555-555555555555");
        assert_eq!(packet["durationSeconds"], 0.25);
        validate(manifest);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sculpt_bridge_uuid_validation_and_track_identity() {
        assert!(valid_uuid("12345678-1234-1234-1234-1234567890ab"));
        assert!(!valid_uuid("12345678-1234-1234-1234-1234567890ag"));
        assert!(!valid_uuid("../../foo"));
        let mut track = Track::new(soundcraft_model::TrackId(1), "Bass", TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, [80, 80, 80]);
        track.comments = "Source stem UUID: 12345678-1234-1234-1234-1234567890ab".into();
        assert_eq!(source_stem_uuid(&track).as_deref(), Some("12345678-1234-1234-1234-1234567890ab"));
    }
}
