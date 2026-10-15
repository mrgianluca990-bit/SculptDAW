#!/usr/bin/env python3
"""Sculpt DAW Core 17 consolidated patch (after proven Core 15 + Core 16).
Applies ONLY the requested session, import, markers, group-edit, send and master-pan repairs.
Real macOS/rust integration is validated by GitHub Actions after patching.
"""
from pathlib import Path


def change(file, old, new, tag):
    p=Path(file)
    s=p.read_text(encoding='utf-8')
    if tag in s:
        print('ALREADY', tag)
        return
    count=s.count(old)
    if count != 1:
        raise RuntimeError(f'{file} {tag}: expected one anchor, found {count}: {old[:90]!r}')
    p.write_text(s.replace(old,new,1),encoding='utf-8')
    print('PATCHED',tag)

IO='crates/engine/src/io.rs'
CMD='crates/engine/src/cmd/file.rs'
EDIT='crates/ui-egui/src/edit_window.rs'
MIX='crates/ui-egui/src/mix_window.rs'
MAIN='apps/soundcraft/src/main.rs'
DIALOG='crates/ui-egui/src/dialogs.rs'
MENUS='sculpt/qa-core-menu.txt'
ENG='crates/engine/src/edit.rs'
AUDIO='crates/mix/src/lib.rs'
EVENT='crates/engine/src/cmd/event.rs'

# 1. For NEW projects, SessionName/SessionName.scdaw and Audio Files/.
# Existing file saves must stay at their original path (including legacy .scraft).
change(IO,'/// True when `command` would write a session over the existing `file`.', '''/// SCULPT_CORE17_PROJECT_FOLDER: a new native project always gets its own folder.
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

/// True when `command` would write a session over the existing `file`.''','SCULPT_CORE17_PROJECT_FOLDER')
change(CMD,'''            let path = str_param(p, "path").ok_or_else(|| bad("session.save_as", "`path` required"))?.to_string();
            refuse_replacing(e, "session.save_as", &crate::io::session_file(&path), p)?;''','''            let requested = str_param(p, "path").ok_or_else(|| bad("session.save_as", "`path` required"))?.to_string();
            // SCULPT_CORE17_SAVE_AS_FOLDER: keep audio next to its own project, not loose in a shared directory.
            let path = if bool_or(p, "project_folder", false) {
                crate::io::sculpt_project_file(&requested).to_string_lossy().into_owned()
            } else { requested };
            refuse_replacing(e, "session.save_as", &crate::io::session_file(&path), p)?;''','SCULPT_CORE17_SAVE_AS_FOLDER')
change(CMD,'''            let path = str_param(p, "path").ok_or_else(|| bad("session.save_copy", "`path` required"))?.to_string();
            refuse_replacing(e, "session.save_copy", &crate::io::session_file(&path), p)?;''','''            let requested = str_param(p, "path").ok_or_else(|| bad("session.save_copy", "`path` required"))?.to_string();
            // SCULPT_CORE17_SAVE_COPY_FOLDER
            let path = if bool_or(p, "project_folder", false) {
                crate::io::sculpt_project_file(&requested).to_string_lossy().into_owned()
            } else { requested };
            refuse_replacing(e, "session.save_copy", &crate::io::session_file(&path), p)?;''','SCULPT_CORE17_SAVE_COPY_FOLDER')
# The graphical native picker can return a filename, never a fully preorganized project folder.
change(DIALOG,'''                        v[key.as_str()] = json!(path);
                        if replaces {''','''                        v[key.as_str()] = json!(path);
                        // SCULPT_CORE17_PROJECT_UI: new session/save copy creates its own folder.
                        if cmd == "session.save_as" || cmd == "session.save_copy" {
                            v["project_folder"] = json!(true);
                        }
                        if replaces {''','SCULPT_CORE17_PROJECT_UI')
change(MAIN,'"session.open" => d.add_filter("Sculpt DAW Session", &["scraft"]),','"session.open" => d.add_filter("Sculpt DAW Session", &["scdaw", "scraft"]), // SCULPT_CORE17_OPEN_PICKER','SCULPT_CORE17_OPEN_PICKER')
change(MAIN,'let mut d = rfd::FileDialog::new().add_filter("Sculpt DAW Session", &["scraft"]);','let mut d = rfd::FileDialog::new().add_filter("Sculpt DAW Session", &["scdaw"]); // SCULPT_CORE17_SAVE_PICKER','SCULPT_CORE17_SAVE_PICKER')
# Keep Audio Files/ present even in an empty session.
change(IO,'''    let audio_dir = dir.join("Audio Files");
    let mut s = session.clone();''','''    let audio_dir = dir.join("Audio Files");
    // SCULPT_CORE17_AUDIO_FOLDER: project layout exists from first save.
    std::fs::create_dir_all(&audio_dir).map_err(|err| EngineError::Io(format!("{}: {err}", audio_dir.display())))?;
    let mut s = session.clone();''','SCULPT_CORE17_AUDIO_FOLDER')
# 2. One-click native multiselect import from every UI entry point.
change(DIALOG,'''    let Some(mut d) = app.dialogs.open.take() else { return };''','''    // SCULPT_CORE17_IMPORT_DIRECT: old contextual/shortcut path used to show an
    // extra text field, then a second Browse button. Use native picker immediately.
    if matches!(app.dialogs.open, Some(Dialog::PathPrompt { ref cmd, .. }) if cmd == "file.import_audio") {
        app.dialogs.open = None;
        if let Some(pick) = &app.services.pick_open_many {
            if let Some(paths) = pick("file.import_audio", &[]) {
                let at = app.engine.session().edit.selection.start;
                let result = app.run("file.import_audio", json!({"paths": paths, "at": at}));
                if let Err(e) = result { app.ui.status = e; }
            }
        }
        return;
    }
    let Some(mut d) = app.dialogs.open.take() else { return };''','SCULPT_CORE17_IMPORT_DIRECT')
# Improve direct menu path: process all audio in a single engine transaction.
change('crates/ui-egui/src/menus.rs','''                let mut imported = 0usize;
                let mut failed = 0usize;
                for file in paths {
                    if app.run("file.import_audio", json!({"path": file, "at": at})).is_ok() {
                        imported += 1;
                    } else {
                        failed += 1;
                    }
                }
                app.ui.status = format!("Imported {imported} audio file(s)" );
                if failed > 0 {
                    app.ui.status.push_str(&format!("; {failed} failed"));
                }''','''                // SCULPT_CORE17_IMPORT_GROUP: a single multi-file import action.
                match app.run("file.import_audio", json!({"paths": paths, "at": at})) {
                    Ok(v) => app.ui.status = format!("Audio imported: {}", v),
                    Err(e) => app.ui.status = e,
                }''','SCULPT_CORE17_IMPORT_GROUP')
# 3. Marker engine already exists. Restore UI access on the small Core menu,
# and add regression coverage for recall, rename, save/load and deletion.
menu=Path(MENUS)
content=menu.read_text(encoding='utf-8')
for path in ('View > Rulers > Markers','Window > Memory Locations'):
    if path not in content.splitlines(): content=content.rstrip()+'\n'+path+'\n'
menu.write_text(content, encoding='utf-8')
source=Path(EVENT)
content=source.read_text(encoding='utf-8')
if 'mod sculpt_core17_marker_tests' not in content:
    content += '''
#[cfg(test)]
mod sculpt_core17_marker_tests {
    use super::*;
    #[test]
    fn sculpt_core17_markers_survive_session_roundtrip_and_recall() {
        let mut e=Engine::default();
        let m=e.execute("markers.add", &json!({"at": 96000, "name": "Verse A"})).unwrap();
        let n=m["number"].as_u64().unwrap();
        assert_eq!(e.session().markers.len(), 1);
        e.execute("markers.edit", &json!({"number": n, "rename": "Verse B", "at": 192000})).unwrap();
        e.execute("markers.recall", &json!({"number": n})).unwrap();
        assert_eq!(e.session().edit.selection.start, 192000);
        let json=e.session().to_json().unwrap();
        let loaded=soundcraft_model::Session::from_json(&json).unwrap();
        assert_eq!(loaded.markers.len(), 1);
        assert_eq!(loaded.markers[0].name, "Verse B");
        assert_eq!(loaded.markers[0].start, 192000);
        e.execute("markers.delete", &json!({"number": n})).unwrap();
        assert!(e.session().markers.is_empty());
    }
}
'''
    source.write_text(content,encoding='utf-8')
# 4. Group editing: selecting tracks should select simultaneous clips as a
# coherent drag group; mixed-source vertical moves preserve track row offsets.
change(EDIT,'''                let clips = if s.edit.selected_clips.contains(&c.id) { s.edit.selected_clips.clone() } else { vec![c.id] };
                Gesture::MoveClips { clips, grab_at: at_origin, delta: 0, from_track: track.id, to_track: track.id }''','''                // SCULPT_CORE17_GROUP_SELECTION: keep explicit clip selections;
                // otherwise dragging on a selected track brings matching-time clips
                // on the other selected tracks, not every clip in the session.
                let mut clips = if s.edit.selected_clips.contains(&c.id) {
                    s.edit.selected_clips.clone()
                } else if s.edit.selected_tracks.contains(&track.id) && s.edit.selected_tracks.len() > 1 {
                    let mut group = vec![c.id];
                    for t in &s.tracks {
                        if t.id != track.id && s.edit.selected_tracks.contains(&t.id) {
                            group.extend(t.clips().iter().filter(|other| other.range().overlaps(&c.range())).map(|other| other.id));
                        }
                    }
                    group
                } else { vec![c.id] };
                clips.retain(|id| *id != c.id);
                clips.insert(0, c.id); // drag-origin anchors vertical track offsets
                clips.dedup();
                Gesture::MoveClips { clips, grab_at: at_origin, delta: 0, from_track: track.id, to_track: track.id }''','SCULPT_CORE17_GROUP_SELECTION')
# Existing Core15 path chooses one to_track for ALL clips; map to destinations
# retaining relative row offset. Prevent invalid group moves by refusing all.
change(ENG,'''    let moving_ids: std::collections::HashSet<ClipId> = ids.iter().copied().collect();''','''    // SCULPT_CORE17_GROUP_TRACK_OFFSETS: for multi-track selections the
    // destination is relative to the first (dragged) clip's track.
    let mut destinations = std::collections::HashMap::<TrackId, TrackId>::new();
    if let Some(dest) = to_track {
        let order: Vec<TrackId> = s.tracks.iter().map(|t| t.id).collect();
        let Some(anchor) = ids.iter().find_map(|id| s.find_clip(*id).map(|(tid, _)| tid)) else { return 0 };
        let Some(from_idx) = order.iter().position(|t| *t == anchor) else { return 0 };
        let Some(to_idx) = order.iter().position(|t| *t == dest) else { return 0 };
        let shift = to_idx as isize - from_idx as isize;
        for id in ids {
            let Some((from, _)) = s.find_clip(*id) else { continue };
            let Some(idx) = order.iter().position(|t| *t == from) else { return 0 };
            let new_idx = idx as isize + shift;
            if new_idx < 0 || new_idx >= order.len() as isize { return 0; }
            let target = order[new_idx as usize];
            if s.track(target).is_none_or(|t| !t.kind.has_playlist()) { return 0; }
            destinations.insert(from, target);
        }
    }
    let moving_ids: std::collections::HashSet<ClipId> = ids.iter().copied().collect();''','SCULPT_CORE17_GROUP_TRACK_OFFSETS')
change(ENG,'relocated.push((to_track.unwrap_or(tid), c));','relocated.push((destinations.get(&tid).copied().unwrap_or(tid), c)); // SCULPT_CORE17_GROUP_TARGET','SCULPT_CORE17_GROUP_TARGET')
# Important: do not return early due to helper signature not being pub or ToolKind.
# 5. Master pan currently is a disconnected UI control: master DSP loop simply
# scales each channel. With default [-1,+1], a stereo dual panner is identity.
change(AUDIO,'''                for (ch, src) in strip.buf.iter().enumerate().take(self.main.len()) {
                    if let Some(m) = self.main.get_mut(ch) {
                        for i in 0..frames {
                            let g = lerp_gain(v0, v1, i, frames);
                            if let (Some(d), Some(x)) = (m.get_mut(i), src.get(i)) {
                                *d = x * g;
                            }
                        }
                    }
                }
                self.meters.entry(t.id).or_default().measure(&self.main, frames, 0.0);''','''                // SCULPT_CORE17_MASTER_PAN: previously the master panners
                // changed UI values but not actual stereo output samples.
                if self.main.len() == 2 && strip.buf.len() >= 2 && t.mixer.pan.len() >= 2 {
                    let p0 = t.mixer.pan[0].clamp(-1.0, 1.0);
                    let p1 = t.mixer.pan[1].clamp(-1.0, 1.0);
                    let (l0, r0) = (((1.0 - p0) * 0.5).sqrt(), ((1.0 + p0) * 0.5).sqrt());
                    let (l1, r1) = (((1.0 - p1) * 0.5).sqrt(), ((1.0 + p1) * 0.5).sqrt());
                    for i in 0..frames {
                        let g = lerp_gain(v0, v1, i, frames);
                        let x0 = strip.buf[0][i];
                        let x1 = strip.buf[1][i];
                        self.main[0][i] = (x0 * l0 + x1 * l1) * g;
                        self.main[1][i] = (x0 * r0 + x1 * r1) * g;
                    }
                } else {
                    for (ch, src) in strip.buf.iter().enumerate().take(self.main.len()) {
                        if let Some(m) = self.main.get_mut(ch) {
                            for i in 0..frames {
                                let g = lerp_gain(v0, v1, i, frames);
                                if let (Some(d), Some(x)) = (m.get_mut(i), src.get(i)) {
                                    *d = x * g;
                                }
                            }
                        }
                    }
                }
                self.meters.entry(t.id).or_default().measure(&self.main, frames, 0.0);''','SCULPT_CORE17_MASTER_PAN')

# 6. Send popup should have only ONE dB amount control. Core16 already
# removes the duplicate scale; preserve that and hard-fail if regression.
assert 'send_level_scale(ui, send.level_db);' not in Path(MIX).read_text(encoding='utf-8'), 'send duplicated scale returned'

# New-project GUI flag regression: preserve programmatic legacy API.
project_tests = r'''
#[cfg(test)]
mod sculpt_core17_project_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn sculpt_core17_new_project_creates_folder_and_audio_files() {
        let root = std::env::temp_dir().join(format!("sculpt17-project-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let wanted = root.join("New Session.scdaw");
        let mut e = Engine::default();
        let result = e.execute("session.save_as", &json!({"path": wanted.to_string_lossy(), "project_folder": true})).unwrap();
        let file = root.join("New Session/New Session.scdaw");
        assert_eq!(result["path"].as_str(), file.to_str());
        assert!(file.is_file());
        assert!(root.join("New Session/Audio Files").is_dir());
        let mut other = Engine::default();
        assert!(other.execute("session.open", &json!({"path": file.to_string_lossy()})).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }
}
'''
pth = Path(CMD)
src = pth.read_text(encoding='utf-8')
if 'mod sculpt_core17_project_tests' not in src:
    pth.write_text(src + project_tests, encoding='utf-8')

# One undoable group move preserving track lanes and common time delta.
group_test = r'''
    #[test]
    fn sculpt_core17_multitrack_group_move_keeps_rows_and_relative_positions() {
        let (mut s,a) = session_with_clip();
        let b = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some("B"));
        let c = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some("C"));
        let aid = s.track(a).unwrap().clips()[0].id;
        let bid = s.new_clip_id();
        s.track_mut(b).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(bid,"B",SourceId(501),1000,1000,2000));
        assert_eq!(move_clips(&mut s,&[aid,bid],500,Some(b)),2);
        assert_eq!(s.find_clip(aid).map(|(tid,c)|(tid,c.start)),Some((b,1500)));
        assert_eq!(s.find_clip(bid).map(|(tid,c)|(tid,c.start)),Some((c,1500)));
    }
'''
pth = Path(ENG)
src = pth.read_text(encoding='utf-8')
if 'sculpt_core17_multitrack_group_move_keeps_rows_and_relative_positions' not in src:
    marker = '    #[test]\n    fn sculpt_core_group_move_keeps_crossfade() {'.replace('\n','\n')
    assert src.count(marker)==1,'missing group test anchor'
    pth.write_text(src.replace(marker,group_test+marker,1),encoding='utf-8')

# 7. Don't hide the marker menu in QA and keep audio formats stable.
for f, markers in {
  IO:['SCULPT_CORE17_PROJECT_FOLDER','SCULPT_CORE17_AUDIO_FOLDER'],CMD:['SCULPT_CORE17_SAVE_AS_FOLDER'],
  EDIT:['SCULPT_CORE17_GROUP_SELECTION'],ENG:['SCULPT_CORE17_GROUP_TRACK_OFFSETS'],
  AUDIO:['SCULPT_CORE17_MASTER_PAN'],EVENT:['sculpt_core17_markers_survive_session_roundtrip_and_recall'],
  DIALOG:['SCULPT_CORE17_IMPORT_DIRECT'],MAIN:['SCULPT_CORE17_OPEN_PICKER'],
}.items():
    src=Path(f).read_text(encoding='utf-8')
    for marker in markers: assert marker in src,(f,marker)
assert Path('assets/sculpt-daw/sculpt-daw-512.png').exists(), 'Approved amber icon missing from repository'
print('SCULPT_CORE17_CONSOLIDATED_PATCH_OK')
