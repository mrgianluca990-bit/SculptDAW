#!/usr/bin/env python3
"""Sculpt DAW Core23 DAW-only overlay: apply AFTER Core20,21,22.

Correct Core22 field feedback without modifying Show or repacking the app.
Fail loudly on changed source anchors. Idempotent by design.
"""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]

def patch(path: str, old: str, new: str, tag: str):
    f = ROOT / path
    s = f.read_text(encoding='utf-8')
    if tag in s:
        print('SKIP', tag)
        return
    count = s.count(old)
    if count != 1:
        raise RuntimeError(f'{path}: {tag}: expected exactly one anchor, found {count}')
    f.write_text(s.replace(old, new, 1), encoding='utf-8')
    print('PATCH', tag)

# 1. Edit inspector: same real Mix widget, but only requested sections.
# The Mix channel fader, pan, metering, inputs/outputs, insert/send controls
# remain the genuine implementation, not a second widget.
patch('crates/ui-egui/src/mix_window.rs',
    'let old_views=std::mem::replace(&mut app.ui.mix_views,vec!["all".to_string()]);',
    '''// SCULPT_CORE23_STRIP_SECTIONS: do not expose PREAMP/INSTRUMENT/OBJECT or EQ.
    let old_views=std::mem::replace(&mut app.ui.mix_views,
        ["io","inserts_ae","inserts_fj","sends_ae","sends_fj"]
           .into_iter().map(str::to_string).collect());''',
    'SCULPT_CORE23_STRIP_SECTIONS')
patch('crates/ui-egui/src/edit_window.rs',
    '''egui::Panel::left("sculpt_core22_channel_strip").resizable(true).default_size(118.0)
        .min_size(108.0).max_size(178.0)''',
    '''egui::Panel::left("sculpt_core22_channel_strip").resizable(true).default_size(180.0)
        .min_size(155.0).max_size(240.0) // SCULPT_CORE23_STRIP_WIDTH''',
    'SCULPT_CORE23_STRIP_WIDTH')
patch('crates/ui-egui/src/mix_window.rs',
    '''let strip_width = ui.available_width().clamp(108.0,178.0);''',
    '''let strip_width = ui.available_width().clamp(155.0,240.0); // SCULPT_CORE23_STRIP_FIT''',
    'SCULPT_CORE23_STRIP_FIT')

# 2. Force ONE new migration for previously saved Core22 preferences.
# Disable Core20 marker migration AFTER reset, otherwise it undoes our right
# split and causes the user's stale left-side memory pane.
patch('crates/ui-egui/src/lib.rs',
    '''    pub sculpt_core22_layout_migrated: bool, // SCULPT_CORE22_LAYOUT_FLAG''',
    '''    pub sculpt_core22_layout_migrated: bool, // SCULPT_CORE22_LAYOUT_FLAG
    pub sculpt_core23_layout_migrated: bool, // SCULPT_CORE23_LAYOUT_FLAG''',
    'SCULPT_CORE23_LAYOUT_FLAG')
patch('crates/ui-egui/src/lib.rs',
    '''            sculpt_core22_layout_migrated: false, // SCULPT_CORE22_LAYOUT_INIT''',
    '''            sculpt_core22_layout_migrated: false, // SCULPT_CORE22_LAYOUT_INIT
            sculpt_core23_layout_migrated: false, // SCULPT_CORE23_LAYOUT_DEFAULT''',
    'SCULPT_CORE23_LAYOUT_DEFAULT')
patch('crates/ui-egui/src/docking.rs',
    '''    let before_sync = workspace.clone();''',
    '''    // SCULPT_CORE23_RIGHT_DOCK_MIGRATION: a single intentional layout reset.
    // Both right-hand panes are docked; neither is a floating Memory window.
    if edit && !app.ui.sculpt_core23_layout_migrated {
        workspace = Workspace::default();
        app.ui.show_tracks_list = false;
        app.ui.show_clip_list = false;
        app.ui.show_midi_editor = false;
        app.ui.show_memory_locations = true;
        app.ui.sculpt_core20_marker_placed = true;
        app.ui.sculpt_core22_layout_migrated = true;
        app.ui.sculpt_core23_layout_migrated = true;
    }
    let before_sync = workspace.clone();''',
    'SCULPT_CORE23_RIGHT_DOCK_MIGRATION')

# 3. Make Audio Settings and I/O Setup visible in BOTH menu hosts.
# The native menu is constructed from a curated tree that excludes Setup;
# adding UI_COMMANDS alone would NOT make it visible.
patch('apps/soundcraft/src/native_menu.rs',
    '''        native.menu.append(&app_menu)?;
        let aliases = menus::ui_aliases();''',
    '''        native.menu.append(&app_menu)?;
        // SCULPT_CORE23_NATIVE_AUDIO_MENU: present even in the compact QA menu.
        let audio_menu = Submenu::new("Setup", true);
        native.add_item(app, &audio_menu, "Audio Settings...", "", Some("window.playback_engine".into()))?;
        native.add_item(app, &audio_menu, "I/O Setup...", "", Some("window.io_setup".into()))?;
        native.menu.append(&audio_menu)?;
        let aliases = menus::ui_aliases();''',
    'SCULPT_CORE23_NATIVE_AUDIO_MENU')
patch('crates/ui-egui/src/menus.rs',
    '''                let extra = ui_aliases();
                for root in tree() {''',
    '''                // SCULPT_CORE23_EGUI_AUDIO_MENU: fallback menu when native host is unavailable.
                ui.menu_button("Setup", |ui| {
                    if ui.button("Audio Settings...").clicked() { app.ui.show_playback_engine=true; ui.close(); }
                    if ui.button("I/O Setup...").clicked() { app.ui.show_io_setup=true; ui.close(); }
                });
                let extra = ui_aliases();
                for root in tree() {''',
    'SCULPT_CORE23_EGUI_AUDIO_MENU')

# 4. Open the *actual device* at the widest supported f32 channel configuration.
# Core21 chooses ≤ session's current paths (12 in the reported show import),
# despite Show being able to use a 64-output CoreAudio interface.
patch('crates/playback/src/lib.rs',
    '''    let want_ch = want_ch.max(2);
    let want_rate = if settings.sample_rate>0 {settings.sample_rate} else {want_rate};''',
    '''    // SCULPT_CORE23_MAX_PHYSICAL_CHANNELS: prefer actual hardware capacity
    // up to the platform's 64-channel routing limit, not existing path count.
    let _requested_paths = want_ch;
    let want_ch = 64u16;
    let want_rate = if settings.sample_rate>0 {settings.sample_rate} else {want_rate};''',
    'SCULPT_CORE23_MAX_PHYSICAL_CHANNELS')

# 5. Clarify session vs device sample rates. Do not silently change the
# session sample-rate when the operator modifies the hardware configuration.
patch('crates/ui-egui/src/windows.rs',
    '''            ui.label("A device change is rejected during playback or recording. Stop transport first.");''',
    '''            // SCULPT_CORE23_RATE_WARNING: distinguish device rate from session rate.
            let project_rate=app.engine.session().sample_rate.hz();
            ui.label(format!("Session: {} Hz · Device target: {} Hz",project_rate,
                if cfg.sample_rate==0 {project_rate} else {cfg.sample_rate}));
            if cfg.sample_rate>0 && cfg.sample_rate!=project_rate {
                ui.colored_label(egui::Color32::YELLOW,
                    "WARNING: hardware rate differs from session; playback is resampled. The session rate is not changed.");
            }
            ui.label("A device change is rejected during playback or recording. Stop transport first.");''',
    'SCULPT_CORE23_RATE_WARNING')
patch('crates/engine/src/io.rs',
    '''    let channels = buf.num_channels();
    let s = e.session_mut();''',
    '''    // SCULPT_CORE23_IMPORT_RATE_NOTICE: sources are resampled by add_source.
    if buf.sample_rate>0 && buf.sample_rate!=e.session().sample_rate.hz() {
        e.message(format!("{}: imported at {} Hz, resampled to session {} Hz",
            name,buf.sample_rate,e.session().sample_rate.hz()));
    }
    let channels = buf.num_channels();
    let s = e.session_mut();''',
    'SCULPT_CORE23_IMPORT_RATE_NOTICE')

# 6. Group editing: expand associated clips before all editor operations.
# Never merge unrelated groups or silent tracks. Only active EDIT groups
# participate; MIX-only and disabled groups are left independent.
group_helpers=r'''
/// SCULPT_CORE23_GROUP_EDIT_HELPERS: resolve active Edit group membership.
pub fn sculpt_group_tracks(s: &soundcraft_model::Session, track: soundcraft_model::TrackId)
    -> Vec<soundcraft_model::TrackId> {
    let mut out = vec![track];
    for g in &s.groups {
        if g.active && g.edit && g.members.contains(&track) {
            for id in &g.members {
                if !out.contains(id) { out.push(*id); }
            }
        }
    }
    out
}

/// A clip gesture is applied to overlapping clips on other active Edit-group tracks.
/// Preserve the original clicked clip and avoid duplicate IDs.
pub fn sculpt_group_clips(s: &soundcraft_model::Session, selected: &[soundcraft_model::ClipId])
    -> Vec<soundcraft_model::ClipId> {
    let mut out = selected.to_vec();
    for cid in selected {
        let Some((track, clip)) = s.find_clip(*cid) else { continue };
        for member in sculpt_group_tracks(s, track) {
            let Some(t) = s.track(member) else { continue };
            for c in t.clips() {
                if c.start < clip.end() && c.end() > clip.start && !out.contains(&c.id) {
                    out.push(c.id);
                }
            }
        }
    }
    out
}
'''
f=ROOT/'crates/engine/src/edit.rs'
s=f.read_text(encoding='utf-8')
if 'SCULPT_CORE23_GROUP_EDIT_HELPERS' not in s:
    f.write_text(s+group_helpers,encoding='utf-8')
    print('PATCH SCULPT_CORE23_GROUP_EDIT_HELPERS')
patch('crates/engine/src/cmd/edit.rs',
    '''    let list_changes = clips.is_some() || sources.is_some() || st.is_some();''',
    '''    // SCULPT_CORE23_GROUP_CLIP_SELECTION: clip selection spans all active Edit-group members.
    let clips = clips.map(|ids| crate::edit::sculpt_group_clips(e.session(),&ids));
    let list_changes = clips.is_some() || sources.is_some() || st.is_some();''',
    'SCULPT_CORE23_GROUP_CLIP_SELECTION')
patch('crates/ui-egui/src/edit_window.rs',
    '''                Gesture::MoveClips { clips, grab_at: at_origin, delta: 0, from_track: track.id, to_track: track.id }''',
    '''                // SCULPT_CORE23_GROUP_DRAG: all linked clips move together.
                let clips=soundcraft_engine::edit::sculpt_group_clips(&s,&clips);
                Gesture::MoveClips { clips, grab_at: at_origin, delta: 0, from_track: track.id, to_track: track.id }''',
    'SCULPT_CORE23_GROUP_DRAG')
patch('crates/ui-egui/src/edit_window.rs',
    '''                let _ = app.run("edit.fades_create", json!({"tracks": [track.0], "start": a, "end": b, "shape": "equal power"}));''',
    '''                // SCULPT_CORE23_GROUP_FADE: apply same time range to grouped tracks.
                let tracks=soundcraft_engine::edit::sculpt_group_tracks(s,track).iter().map(|t|t.0).collect::<Vec<_>>();
                let _ = app.run("edit.fades_create", json!({"tracks": tracks, "start": a, "end": b, "shape": "equal power"}));''',
    'SCULPT_CORE23_GROUP_FADE')
patch('crates/ui-egui/src/edit_window.rs',
    '''            let _ = app.run("edit.fades_create", json!({"tracks": [track.0], "start": start, "end": end, "shape": "equal power"}));''',
    '''            // SCULPT_CORE23_GROUP_CROSSFADE: same seam on linked tracks.
            let tracks=soundcraft_engine::edit::sculpt_group_tracks(s,track).iter().map(|t|t.0).collect::<Vec<_>>();
            let _ = app.run("edit.fades_create", json!({"tracks": tracks, "start": start, "end": end, "shape": "equal power"}));''',
    'SCULPT_CORE23_GROUP_CROSSFADE')
patch('crates/ui-egui/src/edit_window.rs',
    '''            let _ = app.run("edit.trim_to_fill_selection", json!({"clips": [clip.0], "start": r.start, "end": r.end}));''',
    '''            let clips=soundcraft_engine::edit::sculpt_group_clips(app.engine.session(),&[clip])
                .iter().map(|c|c.0).collect::<Vec<_>>(); // SCULPT_CORE23_GROUP_TRIM_EXTEND
            let _ = app.run("edit.trim_to_fill_selection", json!({"clips": clips, "start": r.start, "end": r.end}));''',
    'SCULPT_CORE23_GROUP_TRIM_EXTEND')
patch('crates/ui-egui/src/edit_window.rs',
    '''            let _ = app.run(cmd, json!({"tracks": [track.0], "at": to}));''',
    '''            let tracks=soundcraft_engine::edit::sculpt_group_tracks(app.engine.session(),track)
                .iter().map(|t|t.0).collect::<Vec<_>>(); // SCULPT_CORE23_GROUP_TRIM
            let _ = app.run(cmd, json!({"tracks": tracks, "at": to}));''',
    'SCULPT_CORE23_GROUP_TRIM')

# Refuse to silently ship the previous failure modes.
expect={
'crates/ui-egui/src/docking.rs':['SCULPT_CORE23_RIGHT_DOCK_MIGRATION'],
'crates/ui-egui/src/menus.rs':['SCULPT_CORE23_EGUI_AUDIO_MENU'],
'apps/soundcraft/src/native_menu.rs':['SCULPT_CORE23_NATIVE_AUDIO_MENU'],
'crates/playback/src/lib.rs':['SCULPT_CORE23_MAX_PHYSICAL_CHANNELS'],
'crates/ui-egui/src/mix_window.rs':['SCULPT_CORE23_STRIP_SECTIONS'],
'crates/ui-egui/src/edit_window.rs':['SCULPT_CORE23_GROUP_DRAG','SCULPT_CORE23_GROUP_CROSSFADE'],
'crates/engine/src/edit.rs':['SCULPT_CORE23_GROUP_EDIT_HELPERS'],
'crates/engine/src/cmd/edit.rs':['SCULPT_CORE23_GROUP_CLIP_SELECTION'],
'crates/engine/src/io.rs':['SCULPT_CORE23_IMPORT_RATE_NOTICE'],
}
for path,tags in expect.items():
    s=(ROOT/path).read_text(encoding='utf-8')
    for tag in tags:
        assert s.count(tag)==1,(path,tag,s.count(tag))
assert 'vec!["all".to_string()]' not in (ROOT/'crates/ui-egui/src/mix_window.rs').read_text(encoding='utf-8')
print('SCULPT CORE23: all DAW-only regression guards passed')
