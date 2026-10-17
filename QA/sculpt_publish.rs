//! Sculpt DAW -> Sculpt Show v1 handoff preparation.
//! Never claims Show has ingested anything. The Show-side receiver is separate.
//! Original session/audio and origin manifest are never overwritten.
use crate::SoundApp;
use serde_json::{json, Value};
use soundcraft_model::{Route, Track, TrackKind};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fresh_uuid() -> Result<String, String> {
    // macOS supplies uuidgen, so no new Rust/Cargo.lock dependencies are introduced.
    let output = Command::new("/usr/bin/uuidgen").output().map_err(|e| format!("uuidgen: {e}"))?;
    if !output.status.success() {return Err("Unable to create an identifier for this cue".into());}
    let s=String::from_utf8(output.stdout).map_err(|e|e.to_string())?.trim().to_owned();
    if !valid_uuid(&s) {return Err("uuidgen returned an invalid identifier".into());}
    Ok(s)
}
fn valid_uuid(s:&str)->bool {
    s.len()==36 && s.chars().enumerate().all(|(i,c)|
        if [8,13,18,23].contains(&i) {c=='-'}else{c.is_ascii_hexdigit()})
}
fn get_str<'a>(v:&'a Value,key:&str)->Result<&'a str,String>{
    v.get(key).and_then(Value::as_str).filter(|s|!s.is_empty())
        .ok_or_else(||format!("Show origin manifest is missing {key}"))
}
fn source_stem_uuid(track: &Track) -> Option<String> {
    let suffix=track.comments.split("Source stem UUID: ").nth(1)?;
    let uuid=suffix.trim().split_whitespace().next()?;
    valid_uuid(uuid).then(||uuid.to_owned())
}
fn original_stem<'a>(manifest:&'a Value,track:&Track)->Option<&'a Value>{
    let id=source_stem_uuid(track)?;
    manifest.get("stems")?.as_array()?.iter().find(|v|
        v.get("elementID").and_then(Value::as_str).is_some_and(|x|x.eq_ignore_ascii_case(&id)))
}
fn physical_output(track:&Track,session:&soundcraft_model::Session,old:Option<&Value>)->usize {
    if let Route::Hardware(name)=&track.mixer.output {
        if let Some(out)=session.outputs.iter().find(|o| &o.name==name) {
            return usize::from(out.first_channel)+1;
        }
    }
    old.and_then(|o|o.get("outputStartChannel")).and_then(Value::as_u64)
        .map_or(1,|c|usize::try_from(c).unwrap_or(1))
}
fn clean_dir_name(s:&str)->String{
    let s: String=s.chars().map(|c|if c.is_ascii_alphanumeric() || c=='-' || c=='_' {c}else{'_'}).take(55).collect();
    if s.is_empty(){"Cue".into()}else{s}
}
fn publish_folder(project:&Path, id:&str)->Result<PathBuf,String>{
    // Every attempt is isolated so no previous exported WAV/manifest can be overwritten.
    let unique=fresh_uuid()?;
    let root=project.join("Sculpt Publish to Show").join(format!("{}-{}",clean_dir_name(id),unique));
    fs::create_dir_all(root.join("Audio")).map_err(|e|format!("Create publish workspace: {e}"))?;
    Ok(root)
}
fn value_array<'a>(v:&'a Value,key:&str)->Result<&'a Vec<Value>,String>{
    v.get(key).and_then(Value::as_array).ok_or_else(||format!("Malformed Show manifest: {key}"))
}
fn write_atomic(path:&Path,body:&[u8])->Result<(),String>{
    let tmp=path.with_extension("json.tmp");
    fs::write(&tmp,body).map_err(|e|format!("Write manifest: {e}"))?;
    fs::rename(&tmp,path).map_err(|e|format!("Finalize manifest: {e}"))
}

/// Stage a return package, retaining IDs for update or creating fresh IDs for a new cue.
/// A Show receiver still needs to ingest it; this function never changes a Show project.
pub fn publish(app:&mut SoundApp,new_cue:bool)->Result<Value,String>{
    if app.engine.transport.playing || app.engine.transport.recording {
        return Err("Stop playback/recording before publishing to Show".into());
    }
    if app.engine.is_dirty() {
        return Err("Save your Sculpt DAW session before publishing to Show".into());
    }
    let current=app.engine.path.as_ref().ok_or("Save your session before publishing to Show")?;
    let session_file=Path::new(current);
    let session_dir=session_file.parent().ok_or("The saved session has no parent folder")?;
    let origin_path=session_dir.join("sculpt-cue-bridge.json");
    let origin_data=fs::read(&origin_path).map_err(|_|format!(
        "No Show origin manifest alongside session ({}). Open a cue from Show first.",origin_path.display()))?;
    let mut origin: Value=serde_json::from_slice(&origin_data).map_err(|e|format!("Invalid Show bridge manifest: {e}"))?;
    if get_str(&origin,"format")? != "com.sculpt.bridge.cue" ||
        origin.get("version").and_then(Value::as_u64)!=Some(1) {
        return Err("The session does not contain a supported Sculpt Show v1 manifest".into());
    }
    let _show_name=get_str(&origin,"showName")?;
    let cue_list=get_str(&origin,"cueListID")?;
    if !valid_uuid(cue_list){return Err("Invalid cue list identifier in Show manifest".into());}
    let old_cue=get_str(&origin,"cueID")?;
    if !valid_uuid(old_cue){return Err("Invalid cue identifier in Show manifest".into());}
    let old_cue=old_cue.to_owned();
    let _=value_array(&origin,"stems")?;
    let prior_markers=value_array(&origin,"markers")?.clone();
    let s=app.engine.session().clone();
    let tracks:Vec<_>=s.tracks.iter().filter(|t|
        matches!(t.kind,TrackKind::Audio|TrackKind::Instrument|TrackKind::Midi)
    ).collect();
    if tracks.is_empty(){return Err("No audio or instrument tracks to publish".into());}
    if tracks.iter().any(|t|t.inactive || t.mixer.mute) {
        return Err("Muted or inactive tracks cannot be published yet; unmute/activate them to avoid missing stems".into());
    }
    if tracks.len()>64 {return Err("Show bridge supports no more than 64 stems".into());}
    let duration=s.content_end().max(1) as f64 / s.sample_rate.hz() as f64;
    if duration>4.0*3600.0 {return Err("Publishing sessions longer than 4 hours is not supported".into());}
    let destination_id=if new_cue{fresh_uuid()?}else{old_cue.clone()};
    let folder=publish_folder(session_dir,&destination_id)?;
    let audio_dir=folder.join("Audio");

    // Use the established offline stem renderer, not the physical playback outputs.
    // It renders all stems from time 0 to the same session end, with inserts/edits/fades.
    app.capture_plugin_states();
    let rendered=app.run("file.bounce_stems",json!({
        "dir":audio_dir.to_string_lossy().into_owned(),
        "start":{"seconds":0.0},"end":{"seconds":duration},
        "format":"wav","bit_depth":"24","fold_down":"stereo"
    }))?;
    let paths=value_array(&rendered,"files")?;
    if paths.len()!=tracks.len(){return Err(format!(
        "Incomplete offline export: {} file(s) for {} track(s)",paths.len(),tracks.len()));}
    let mut stems=Vec::<Value>::new();
    for (track,path) in tracks.iter().zip(paths.iter()) {
        let wav=path.as_str().ok_or("Bounce returned an invalid path")?;
        if !Path::new(wav).is_file(){return Err(format!("Missing bounce file: {wav}"));}
        let old=original_stem(&origin,track);
        let element_id=if new_cue {fresh_uuid()?}
            else {match old.and_then(|v|v.get("elementID")).and_then(Value::as_str) {
                Some(id) if valid_uuid(id)=>id.to_owned(),_=>fresh_uuid()?
            }};
        let first=physical_output(track,&s,old);
        if !(1..=63).contains(&first) {
            return Err(format!("{}: stereo bounce cannot start at hardware channel {}",track.name,first));
        }
        stems.push(json!({
            "elementID":element_id,"name":track.name,"sourcePath":wav,
            "outputStartChannel":first,"gain":1.0,"muted":false
        }));
    }
    let mut markers=Vec::<Value>::new();
    for m in &s.markers {
        if m.kind!=soundcraft_model::MarkerKind::Marker {continue}
        let old=prior_markers.iter().find(|v|v.get("name").and_then(Value::as_str)==Some(m.name.as_str()));
        let id=if new_cue {fresh_uuid()?}
            else {match old.and_then(|v|v.get("id")).and_then(Value::as_str) {
                Some(id) if valid_uuid(id)=>id.to_owned(),_=>fresh_uuid()?
            }};
        markers.push(json!({"id":id,"name":m.name,
            "seconds":(m.start.max(0) as f64)/(s.sample_rate.hz() as f64)}));
    }
    let cue_name=if new_cue {format!("{} (New Cue)",s.name)}else{s.name.clone()};
    let stems_count=stems.len();
    let markers_count=markers.len();
    origin["cueID"]=json!(destination_id.clone());
    origin["cueName"]=json!(cue_name);
    origin["durationSeconds"]=json!(duration);
    origin["stems"]=json!(stems);
    origin["markers"]=json!(markers);
    origin["publishMode"]=json!(if new_cue{"newCue"}else{"updateCue"});
    origin["sessionSampleRate"]=json!(s.sample_rate.hz());
    origin["originSessionPath"]=json!(current);
    origin["outboundStatus"]=json!("prepared-not-applied");
    // Never modify the inbound show origin. Receiver will verify cue/list IDs.
    let manifest=folder.join("sculpt-cue-bridge.json");
    let body=serde_json::to_vec_pretty(&origin).map_err(|e|e.to_string())?;
    write_atomic(&manifest,&body)?;
    let notice=format!("Show handoff prepared: {} stems, {} markers. Pending Show import. Folder: {}",
        stems_count, markers_count,folder.display());
    app.ui.status=notice;
    Ok(json!({"prepared":true,"showApplied":false,"mode":if new_cue{"newCue"}else{"updateCue"},
         "folder":folder.to_string_lossy().into_owned(),"manifest":manifest.to_string_lossy().into_owned(),
         "cueID":destination_id,"stems":stems_count,"markers":markers_count}))
}
