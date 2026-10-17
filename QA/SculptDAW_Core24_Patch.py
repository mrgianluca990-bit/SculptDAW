#!/usr/bin/env python3
"""Core24 DAW-only overlay. Must be run after Core20/21/22/23 in the macOS builder."""
from pathlib import Path
import re
ROOT=Path(__file__).resolve().parents[1]

def patch(path, old, new, tag):
    f=ROOT/path
    txt=f.read_text(encoding='utf-8')
    if tag in txt:
        print('ALREADY',tag)
        return
    n=txt.count(old)
    if n!=1:
        raise RuntimeError(f'{path}: {tag}: expected exactly one anchor; found {n}')
    f.write_text(txt.replace(old,new,1),encoding='utf-8')
    print('PATCH',tag)

# The DAW keeps its outbox renderer in QA so it survives the workflow's upstream
# checkout/restore phase, then materializes it as a Rust module before cargo check.
module_template = ROOT/'QA/sculpt_publish.rs'
if not module_template.is_file():
    raise RuntimeError('Missing QA/sculpt_publish.rs from Core24 overlay ZIP')
output_module = ROOT/'crates/ui-egui/src/sculpt_publish.rs'
source = module_template.read_text(encoding='utf-8')
if 'pub fn publish(app:&mut SoundApp,new_cue:bool)' not in source:
    raise RuntimeError('Core24 publish module is incomplete')
if not output_module.exists() or output_module.read_text(encoding='utf-8') != source:
    output_module.write_text(source,encoding='utf-8')
    print('PATCH SCULPT_CORE24_PUBLISH_SOURCE')

# ---- Native application menu: File, Setup, then remaining menu groups. ----
patch('apps/soundcraft/src/native_menu.rs',
'''        // SCULPT_CORE23_NATIVE_AUDIO_MENU: present even in the compact QA menu.
        let audio_menu = Submenu::new("Setup", true);
        native.add_item(app, &audio_menu, "Audio Settings...", "", Some("window.playback_engine".into()))?;
        native.add_item(app, &audio_menu, "I/O Setup...", "", Some("window.io_setup".into()))?;
        native.menu.append(&audio_menu)?;
        let aliases = menus::ui_aliases();
        for root in menus::tree() {
            let submenu = native.add_branch(app, &root, &aliases)?;
            native.menu.append(&submenu)?;
        }''',
'''        // SCULPT_CORE24_NATIVE_MENU: File (including Publish), Setup, remaining menus.
        let aliases = menus::ui_aliases();
        for root in menus::tree() {
            let submenu = native.add_branch(app, &root, &aliases)?;
            if root.label == "File" {
                let publish = Submenu::new("Publish in Show", true);
                native.add_item(app, &publish, "Publish in Show", "", Some("ui.publish_in_show".into()))?;
                native.add_item(app, &publish, "Publish in Show as a New Cue", "", Some("ui.publish_in_show_new".into()))?;
                submenu.append(&PredefinedMenuItem::separator())?;
                submenu.append(&publish)?;
            }
            native.menu.append(&submenu)?;
            if root.label == "File" {
                let setup = Submenu::new("Setup", true);
                native.add_item(app, &setup, "Audio Settings...", "", Some("window.playback_engine".into()))?;
                native.add_item(app, &setup, "I/O Setup...", "", Some("window.io_setup".into()))?;
                native.menu.append(&setup)?;
            }
        }''', 'SCULPT_CORE24_NATIVE_MENU')

# Egui menu fallback uses the same order and working command handlers.
patch('crates/ui-egui/src/menus.rs',
'''                // SCULPT_CORE23_EGUI_AUDIO_MENU: fallback menu when native host is unavailable.
                ui.menu_button("Setup", |ui| {
                    if ui.button("Audio Settings...").clicked() { app.ui.show_playback_engine=true; ui.close(); }
                    if ui.button("I/O Setup...").clicked() { app.ui.show_io_setup=true; ui.close(); }
                });
                let extra = ui_aliases();
                for root in tree() {''',
'''                // SCULPT_CORE24_EGUI_MENU: File + Publish, Setup, remaining menus.
                let extra = ui_aliases();
                for root in tree() {''', 'SCULPT_CORE24_EGUI_MENU')
patch('crates/ui-egui/src/menus.rs',
'''                        for c in &root.children {
                            menu_node(app, ui, c, &extra);
                        }
                        if root.label == "AudioSuite" {''',
'''                        for c in &root.children {
                            menu_node(app, ui, c, &extra);
                        }
                        if root.label == "File" { // SCULPT_CORE24_EGUI_PUBLISH
                            ui.separator();
                            ui.menu_button("Publish in Show", |ui| {
                                if ui.button("Publish in Show").clicked() {
                                    let _=app.run("ui.publish_in_show",json!({})); ui.close();
                                }
                                if ui.button("Publish in Show as a New Cue").clicked() {
                                    let _=app.run("ui.publish_in_show_new",json!({})); ui.close();
                                }
                            });
                        }
                        if root.label == "AudioSuite" {''', 'SCULPT_CORE24_EGUI_PUBLISH')
patch('crates/ui-egui/src/menus.rs',
'''                    });
                }
                let title = format!(''',
'''                    });
                    if root.label == "File" { // SCULPT_CORE24_EGUI_SETUP
                        ui.menu_button("Setup", |ui| {
                            if ui.button("Audio Settings...").clicked() { app.ui.show_playback_engine=true; ui.close(); }
                            if ui.button("I/O Setup...").clicked() { app.ui.show_io_setup=true; ui.close(); }
                        });
                    }
                }
                let title = format!(''', 'SCULPT_CORE24_EGUI_SETUP')
patch('crates/ui-egui/src/menus.rs',
'''    ("ui.bounce_dialog", "Bounce Mix…", "", None),''',
'''    ("ui.bounce_dialog", "Bounce Mix…", "", None),
    ("ui.publish_in_show", "Publish in Show", "", None), // SCULPT_CORE24_UI_COMMANDS
    ("ui.publish_in_show_new", "Publish in Show as a New Cue", "", None),''', 'SCULPT_CORE24_UI_COMMANDS')
patch('crates/ui-egui/src/menus.rs',
'''matches!(i, "audiosuite.process" | "ui.dock" | "ui.dock.reset")''',
'''matches!(i, "audiosuite.process" | "ui.dock" | "ui.dock.reset" | "ui.publish_in_show" | "ui.publish_in_show_new") /* SCULPT_CORE24_PUBLISH_ENABLED */''',
'SCULPT_CORE24_PUBLISH_ENABLED')
patch('crates/ui-egui/src/menus.rs',
'''    let r = match id {
        "window.mix" => {''',
'''    let r = match id {
        // SCULPT_CORE24_PUBLISH_COMMANDS: creates a handoff; Show ingestion is a separate feature.
        "ui.publish_in_show" | "ui.publish_in_show_new" => {
            let new_cue = id == "ui.publish_in_show_new";
            let result=crate::sculpt_publish::publish(app,new_cue);
            if let Err(message)=&result {app.ui.status=format!("Publish in Show: {message}");}
            return Some(result);
        }
        "window.mix" => {''', 'SCULPT_CORE24_PUBLISH_COMMANDS')
patch('crates/ui-egui/src/lib.rs',
'''pub mod score_editor;''',
'''pub mod score_editor;
pub mod sculpt_publish; // SCULPT_CORE24_PUBLISH_MODULE''', 'SCULPT_CORE24_PUBLISH_MODULE')

# Make the EDIT inspector scroll to the actual fader rather than overpainting it.
patch('crates/ui-egui/src/mix_window.rs',
'''    let h = ui.available_height().max(560.0);''',
'''    // SCULPT_CORE24_STRIP_HEIGHT: 4 banks of slots plus fader must not overlap.
    // The Edit inspector has a vertical ScrollArea; Mix retains its existing sizing.
    let h = ui.available_height().max(if width.is_some() { 1020.0 } else { 560.0 });''',
'SCULPT_CORE24_STRIP_HEIGHT')

# Correct group-relative trim rather than applying one absolute mouse timestamp to every track.
# Core23's old implementation cuts unrelated clips whenever group tracks are offset.
p='crates/ui-egui/src/edit_window.rs'
f=ROOT/p
s=f.read_text(encoding='utf-8')
if 'SCULPT_CORE24_RELATIVE_TRIM' not in s:
    start=s.index('fn trim_clip(app: &mut SoundApp, clip: ClipId, track: TrackId, to: Samples, start: bool) {')
    end=s.index('\nfn clip_context_menu(',start)
    fresh='''fn trim_clip(app: &mut SoundApp, clip: ClipId, _track: TrackId, to: Samples, start: bool) {
    // SCULPT_CORE24_RELATIVE_TRIM: target the corresponding clip on each linked track,
    // preserving the amount trimmed even when grouped clips start at different times.
    let s = app.engine.session().clone();
    let Some((_, origin)) = s.find_clip(clip) else { return };
    let delta = if start { to.saturating_sub(origin.start) } else { to.saturating_sub(origin.end()) };
    let related = soundcraft_engine::edit::sculpt_group_clips(&s, &[clip]);
    let old_tracks: Vec<u64> = s.edit.selected_tracks.iter().map(|t|t.0).collect();
    let old_selection = s.edit.selection;
    let mut edited=0usize;
    for cid in related {
        let Some((tid,c)) = s.find_clip(cid) else {continue};
        if tid != s.find_clip(clip).map(|(id,_)|id).unwrap_or(tid)
            && !(c.start < origin.end() && c.end() > origin.start) {continue}
        let new_edge = if start {c.start.saturating_add(delta).clamp(0,c.end()-1)}
            else {c.end().saturating_add(delta).max(c.start+1)};
        let range = if start {Range::new(new_edge,c.end())}else{Range::new(c.start,new_edge)};
        // Use the clip-specific operation, not track-wide trims. It clamps to the source file.
        let done=app.run("edit.trim_to_fill_selection",json!({
            "clips":[cid.0], "start":range.start, "end":range.end
        }));
        if done.is_ok(){edited+=1;}
    }
    let _=app.engine.execute("edit.select",&json!({
        "tracks":old_tracks,"start":old_selection.start,"end":old_selection.end}));
    app.ui.status=format!("Group trim: {} linked clip(s)",edited);
}
'''
    # Note: the engine's trim_to_fill currently sets BOTH bounds for all selected clips;
    # this method passes the specific clip's untouched boundary so it only changes one.
    f.write_text(s[:start]+fresh+s[end:],encoding='utf-8')
    print('PATCH SCULPT_CORE24_RELATIVE_TRIM')
else:print('ALREADY SCULPT_CORE24_RELATIVE_TRIM')

# Linked fade selections are not a selection of their underlying clips.
patch('crates/ui-egui/src/lib.rs',
'''    pub selected_fade: Option<FadeSelection>,''',
'''    pub selected_fade: Option<FadeSelection>,
    #[serde(skip)]
    pub selected_group_fades: Vec<FadeSelection>, // SCULPT_CORE24_FADE_STATE''', 'SCULPT_CORE24_FADE_STATE')
patch('crates/ui-egui/src/lib.rs',
'''            selected_fade: None,''',
'''            selected_fade: None,
            selected_group_fades: Vec::new(), // SCULPT_CORE24_FADE_DEFAULT''', 'SCULPT_CORE24_FADE_DEFAULT')
patch('crates/ui-egui/src/lib.rs',
'''                    self.ui.selected_fade = None;''',
'''                    self.ui.selected_fade = None;
                    self.ui.selected_group_fades.clear(); // SCULPT_CORE24_FADE_RESET''', 'SCULPT_CORE24_FADE_RESET')
patch('crates/ui-egui/src/edit_window.rs',
'''    sculpt_draw_crossfades(&painter, &s, &track, tl, lane, app.ui.selected_fade);''',
'''    // SCULPT_CORE24_FADE_LINKED_HIGHLIGHT: each linked track renders its selected fade.
    let active_fade = app.ui.selected_group_fades.iter().copied().find(|f|f.track==track.id)
        .or(app.ui.selected_fade);
    sculpt_draw_crossfades(&painter, &s, &track, tl, lane, active_fade);''', 'SCULPT_CORE24_FADE_LINKED_HIGHLIGHT')
patch('crates/ui-egui/src/edit_window.rs',
'''    let select_in = app.ui.selected_fade.is_some_and(|f| f.track == track.id && f.clip == clip.id && f.other.is_none() && f.fade_in);''',
'''    let select_in = app.ui.selected_group_fades.iter().any(|f| f.track == track.id && f.clip == clip.id && f.other.is_none() && f.fade_in)
        || app.ui.selected_fade.is_some_and(|f| f.track == track.id && f.clip == clip.id && f.other.is_none() && f.fade_in); // SCULPT_CORE24_FADE_IN_HIGHLIGHT''', 'SCULPT_CORE24_FADE_IN_HIGHLIGHT')
patch('crates/ui-egui/src/edit_window.rs',
'''    let select_out = app.ui.selected_fade.is_some_and(|f| f.track == track.id && f.clip == clip.id && f.other.is_none() && !f.fade_in);''',
'''    let select_out = app.ui.selected_group_fades.iter().any(|f| f.track == track.id && f.clip == clip.id && f.other.is_none() && !f.fade_in)
        || app.ui.selected_fade.is_some_and(|f| f.track == track.id && f.clip == clip.id && f.other.is_none() && !f.fade_in); // SCULPT_CORE24_FADE_OUT_HIGHLIGHT''', 'SCULPT_CORE24_FADE_OUT_HIGHLIGHT')
patch('crates/ui-egui/src/edit_window.rs',
'''fn sculpt_delete_target(app: &mut SoundApp, target: FadeSelection) {
    let result = app.run("edit.fade_delete_target", json!({
        "track": target.track.0,
        "clip": target.clip.0,
        "other": target.other.map(|id| id.0),
        "fade_in": target.fade_in,
    }));
    match result {
        Ok(_) => app.ui.selected_fade = None,
        Err(e) => app.ui.status = e,
    }
}''',
'''// SCULPT_CORE24_FADE_LINKED_TARGETS: associate the matching fade handle on edit-group tracks.
fn sculpt_core24_fade_targets(s: &Session, selected: FadeSelection) -> Vec<FadeSelection> {
    let mut out=vec![selected];
    let Some((tid, original))=s.find_clip(selected.clip) else {return out};
    for t in soundcraft_engine::edit::sculpt_group_tracks(s,tid) {
        if t==tid {continue}
        let Some(track)=s.track(t) else {continue};
        let same = track.clips().iter().filter(|c|
            c.start<original.end() && c.end()>original.start &&
            (if selected.fade_in {c.fade_in.len>0}else{c.fade_out.len>0})
        ).min_by_key(|c| c.start.abs_diff(original.start));
        let Some(c)=same else {continue};
        let other=if selected.other.is_some() {
            track.clips().iter().find(|x|x.id!=c.id && x.start<c.end() && x.end()>c.start)
                .map(|x|x.id)
        } else {None};
        if selected.other.is_some() && other.is_none(){continue}
        out.push(FadeSelection{track:t,clip:c.id,other,fade_in:selected.fade_in});
    }
    out
}
fn sculpt_delete_target(app: &mut SoundApp, target: FadeSelection) {
    // SCULPT_CORE24_FADE_GROUP_DELETE: one context-menu action deletes matching group fades.
    let targets=if app.ui.selected_group_fades.contains(&target) {
        app.ui.selected_group_fades.clone()
    } else {
        sculpt_core24_fade_targets(app.engine.session(),target)
    };
    let mut deleted=0usize;
    for fade in targets {
        if app.run("edit.fade_delete_target", json!({
            "track": fade.track.0,"clip":fade.clip.0,
            "other":fade.other.map(|id|id.0),"fade_in":fade.fade_in,
        })).is_ok(){deleted+=1;}
    }
    app.ui.selected_fade=None;
    app.ui.selected_group_fades.clear();
    app.ui.status=format!("Deleted {} linked fade(s)",deleted);
}''', 'SCULPT_CORE24_FADE_GROUP_DELETE')
patch('crates/ui-egui/src/edit_window.rs',
'''            app.ui.selected_fade = Some(fade);
            ui.ctx().request_repaint();''',
'''            app.ui.selected_fade = Some(fade);
            app.ui.selected_group_fades=sculpt_core24_fade_targets(&s,fade); // SCULPT_CORE24_FADE_GROUP_SELECT
            ui.ctx().request_repaint();''', 'SCULPT_CORE24_FADE_GROUP_SELECT')
patch('crates/ui-egui/src/edit_window.rs',
'''        app.ui.selected_fade = None;
    }
    let upper = origin.y < lane.center().y;''',
'''        app.ui.selected_fade = None;
        app.ui.selected_group_fades.clear(); // SCULPT_CORE24_FADE_GROUP_CLEAR
    }
    let upper = origin.y < lane.center().y;''', 'SCULPT_CORE24_FADE_GROUP_CLEAR')

# Make conversion messages visible when import uses the menu's multi-file picker.
patch('crates/engine/src/io.rs',
'''    let channels = buf.num_channels();
    let s = e.session_mut();''',
'''    let channels = buf.num_channels();
    let original_rate = buf.sample_rate; // SCULPT_CORE24_IMPORT_ORIGINAL_RATE
    let s = e.session_mut();''', 'SCULPT_CORE24_IMPORT_ORIGINAL_RATE')
patch('crates/engine/src/io.rs',
'''    Ok(json!({"track": track, "clip": cid, "source": src, "frames": frames, "channels": channels}))''',
'''    Ok(json!({"track": track, "clip": cid, "source": src, "frames": frames, "channels": channels,
              "source_sample_rate":original_rate,"session_sample_rate":s.sample_rate.hz()})) // SCULPT_CORE24_IMPORT_RESULT_RATE''',
'SCULPT_CORE24_IMPORT_RESULT_RATE')
patch('crates/ui-egui/src/lib.rs',
'''                if id == "options.scrolling" {''',
'''                // SCULPT_CORE24_IMPORT_UI_WARNING: e.message alone is not the UI status.
                if id == "file.import_audio" {
                    let source=v.get("source_sample_rate").and_then(Value::as_u64).unwrap_or(0);
                    let rate=v.get("session_sample_rate").and_then(Value::as_u64).unwrap_or(0);
                    self.ui.status=if source>0 && source!=rate {
                        format!("Imported and RESAMPLED: {} Hz → {} Hz (session rate unchanged)",source,rate)
                    } else {format!("Imported audio at {} Hz",rate)};
                }
                if id == "options.scrolling" {''', 'SCULPT_CORE24_IMPORT_UI_WARNING')
patch('crates/ui-egui/src/menus.rs',
'''                let mut imported = 0usize;
                let mut failed = 0usize;''',
'''                let mut imported = 0usize;
                let mut failed = 0usize;
                let mut resampled = 0usize; // SCULPT_CORE24_IMPORT_MENU_COUNT
                let mut conversions=Vec::<String>::new();''', 'SCULPT_CORE24_IMPORT_MENU_COUNT')
patch('crates/ui-egui/src/menus.rs',
'''                    if app.run("file.import_audio", json!({"path": file, "at": at})).is_ok() {
                        imported += 1;
                    } else {
                        failed += 1;
                    }''',
'''                    // SCULPT_CORE24_IMPORT_MENU_RATES: record each converter ratio.
                    match app.run("file.import_audio",json!({"path":file,"at":at})) {
                        Ok(v)=>{
                            imported+=1;
                            let a=v.get("source_sample_rate").and_then(Value::as_u64);
                            let b=v.get("session_sample_rate").and_then(Value::as_u64);
                            if let (Some(a),Some(b))=(a,b) {
                                if a!=b {resampled+=1;conversions.push(format!("{a}→{b} Hz"));}
                            }
                        }
                        Err(_)=>failed+=1,
                    }''', 'SCULPT_CORE24_IMPORT_MENU_RATES')
patch('crates/ui-egui/src/menus.rs',
'''                app.ui.status = format!("Imported {imported} audio file(s)" );''',
'''                app.ui.status = format!("Imported {imported} audio file(s) at session {} Hz; {resampled} RESAMPLED: {}",app.engine.session().sample_rate.hz(),conversions.join(", ")); // SCULPT_CORE24_IMPORT_MENU_NOTICE''',
'SCULPT_CORE24_IMPORT_MENU_NOTICE')

# Ensure Main-route rendering for separately-bounced hardware-routed stems.
patch('crates/engine/src/io.rs',
'''        for t in &mut solo.tracks {
            t.mixer.solo = t.id == id;
        }
        let (ch, fmt) = render_main(&solo, r, fold_stereo);''',
'''        for t in &mut solo.tracks {
            t.mixer.solo = t.id == id;
            if t.id == id {
                // SCULPT_CORE24_STEM_MONITOR: a hardware-only route would render silence
                // in an offline Main bounce. Preserve the original session unchanged.
                t.mixer.output = soundcraft_model::Route::Main;
            }
        }
        let (ch, fmt) = render_main(&solo, r, fold_stereo);''',
'SCULPT_CORE24_STEM_MONITOR')

# Source regression guards; host compile remains the authority on Swift/Rust/macOS.
checks={
 'apps/soundcraft/src/native_menu.rs':['SCULPT_CORE24_NATIVE_MENU'],
 'crates/ui-egui/src/menus.rs':['SCULPT_CORE24_EGUI_PUBLISH','SCULPT_CORE24_PUBLISH_COMMANDS','SCULPT_CORE24_IMPORT_MENU_NOTICE'],
 'crates/ui-egui/src/lib.rs':['SCULPT_CORE24_FADE_STATE','SCULPT_CORE24_IMPORT_UI_WARNING'],
 'crates/ui-egui/src/edit_window.rs':['SCULPT_CORE24_RELATIVE_TRIM','SCULPT_CORE24_FADE_GROUP_DELETE'],
 'crates/engine/src/io.rs':['SCULPT_CORE24_IMPORT_ORIGINAL_RATE','SCULPT_CORE24_STEM_MONITOR'],
 'crates/ui-egui/src/mix_window.rs':['SCULPT_CORE24_STRIP_HEIGHT'],
}
for path,tags in checks.items():
    src=(ROOT/path).read_text(encoding='utf-8')
    for tag in tags:
        if tag not in src:raise RuntimeError(f'{path}: missing {tag}')
    if path=='crates/ui-egui/src/edit_window.rs' and src.count('fn sculpt_delete_target(')!=1:
        raise RuntimeError('fade target function duplicated')
print('SCULPT CORE24: DAW-only publish UI, group fade/trim, import notices, fader sizing source guards OK')
