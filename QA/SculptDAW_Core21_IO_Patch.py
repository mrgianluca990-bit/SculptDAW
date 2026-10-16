#!/usr/bin/env python3
"""Core21 DAW-only I/O: 64 physical channels, device fallback, Show routes.
Run after Core20 overlay; guard all replacements for source drift.
"""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def change(path, old, new, tag, count=1):
    p=ROOT/path
    src=p.read_text(encoding='utf-8')
    if tag in src:
        print('SKIP', tag)
        return
    n=src.count(old)
    if n != count: raise RuntimeError(f'{path}: {tag}: expected {count} anchors, got {n}')
    p.write_text(src.replace(old,new,count), encoding='utf-8')
    print('PATCH',tag)

MIX='crates/mix/src/lib.rs'
change(MIX,
'''    main_fmt: ChannelFormat,
    pub meters: HashMap<TrackId, StripMeter>,''',
'''    main_fmt: ChannelFormat,
    // SCULPT_CORE21_HW_FIELDS: independent physical output bus, 1..64.
    // Never allocate on the device callback; every channel has max_block capacity.
    hardware: Vec<Vec<f32>>,
    hardware_channels: usize,
    pub meters: HashMap<TrackId, StripMeter>,''', 'SCULPT_CORE21_HW_FIELDS')
change(MIX,
'''            main_fmt: ChannelFormat::Stereo,
            meters: HashMap::new(),''',
'''            main_fmt: ChannelFormat::Stereo,
            hardware: vec![vec![0.0; max_block]; 64], // SCULPT_CORE21_HW_INIT
            hardware_channels: 0,
            meters: HashMap::new(),''','SCULPT_CORE21_HW_INIT')
change(MIX,
'''    pub fn set_external_instances(&mut self, on: bool) {''',
'''    /// SCULPT_CORE21_HW_AVAILABILITY: 0 in offline/silent mode; the physical
    /// route then falls back to Main instead of muting the track.
    pub fn set_device_output_channels(&mut self, channels: usize) {
        self.hardware_channels = channels.min(64);
    }

    pub fn set_external_instances(&mut self, on: bool) {''','SCULPT_CORE21_HW_AVAILABILITY')
change(MIX,
'''        for c in &mut self.main {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }''',
'''        for c in &mut self.main {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        // SCULPT_CORE21_HW_CLEAR: distinct physical bus is zeroed every block.
        for c in &mut self.hardware {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }''','SCULPT_CORE21_HW_CLEAR')
change(MIX,'self.route_strip(t, st, pos, frames);',
       'self.route_strip(s, t, st, pos, frames); // SCULPT_CORE21_HW_PASS_SESSION',
       'SCULPT_CORE21_HW_PASS_SESSION')
change(MIX,'self.route_strip(t, &mut st, pos, frames);',
       'self.route_strip(s, t, &mut st, pos, frames); // SCULPT_CORE21_HW_PASS_AUX',
       'SCULPT_CORE21_HW_PASS_AUX')
change(MIX,
'''    fn route_strip(&mut self, t: &Track, strip: &mut Strip, pos: Samples, frames: usize) {''',
'''    fn route_strip(&mut self, s: &Session, t: &Track, strip: &mut Strip, pos: Samples, frames: usize) { // SCULPT_CORE21_HW_ROUTE_SIGNATURE''',
'SCULPT_CORE21_HW_ROUTE_SIGNATURE')
change(MIX,
'''            Route::Bus(b) => {
                if let Some(dst) = self.busses.get_mut(b) {
                    let m = build_matrix(src_fmt, strip.buf.len(), dst.fmt, dst.buf.len(), &spec, law);
                    apply_matrix(&strip.buf, frames, &mut dst.buf, &m, 1.0);
                }
            }
            _ => {}''',
'''            Route::Bus(b) => {
                if let Some(dst) = self.busses.get_mut(b) {
                    let m = build_matrix(src_fmt, strip.buf.len(), dst.fmt, dst.buf.len(), &spec, law);
                    apply_matrix(&strip.buf, frames, &mut dst.buf, &m, 1.0);
                }
            }
            // SCULPT_CORE21_HW_RENDER: a named I/O path is a physical endpoint,
            // NOT an internal bus. Never discard this signal when unavailable.
            Route::Hardware(name) => {
                let endpoint = s.outputs.iter().find(|p| p.name.eq_ignore_ascii_case(name));
                let assigned = endpoint.and_then(|p| {
                    let first = usize::from(p.first_channel);
                    let width = p.format.channels();
                    (width > 0 && first.checked_add(width).is_some_and(|end| end <= 64))
                        .then_some((first, width, p.format))
                });
                if let Some((first, width, format)) = assigned
                    && first + width <= self.hardware_channels
                {
                    if let Some(dst) = self.hardware.get_mut(first..first+width) {
                        let m = build_matrix(src_fmt, strip.buf.len(), format, dst.len(), &spec, law);
                        apply_matrix(&strip.buf, frames, dst, &m, 1.0);
                    }
                    // SCULPT_CORE21_BRIDGE_MAIN_PREVIEW: only Show's dedicated
                    // SCULPT HW outputs keep a parallel Main 1-2 monitor while
                    // editing. Do not double-sum an output already on Main.
                    if name.starts_with("SCULPT HW") && first >= self.main.len() {
                        let m = build_matrix(src_fmt, strip.buf.len(), self.main_fmt, self.main.len(), &spec, law);
                        apply_matrix(&strip.buf, frames, &mut self.main, &m, 1.0);
                    }
                } else {
                    // Device has too few output channels, or this path is absent:
                    // safe Main monitoring, retaining the original Hardware assignment.
                    let m = build_matrix(src_fmt, strip.buf.len(), self.main_fmt, self.main.len(), &spec, law);
                    apply_matrix(&strip.buf, frames, &mut self.main, &m, 1.0);
                }
            }
            _ => {}''','SCULPT_CORE21_HW_RENDER')
change(MIX,
'''                let v = src.and_then(|m| m.get(i)).copied().unwrap_or(0.0);''',
'''                // SCULPT_CORE21_HW_DEVICE_SUM: independent physical paths are
                // summed with the existing Main (physical channels 1..main_width).
                let v = src.and_then(|m| m.get(i)).copied().unwrap_or(0.0)
                    + self.hardware.get(c).and_then(|bus| bus.get(i)).copied().unwrap_or(0.0);''',
'SCULPT_CORE21_HW_DEVICE_SUM')
# Hardware direct-output regression; test 64th channel and missing device fallback.
p=ROOT/MIX
s=p.read_text(encoding='utf-8')
if 'SCULPT_CORE21_TEST_OUTPUT64' not in s:
    needle='    #[test]\n    fn mono_center_pan_is_minus_3db() {'
    assert s.count(needle)==1, 'missing mix regression insertion point'
    tests='''    // SCULPT_CORE21_TEST_OUTPUT64: physical 64 + safe stereo fallback.
    #[test]
    fn sculpt_core21_hardware_output64_and_missing_device_fallback() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.outputs.push(soundcraft_model::OutputPath {
            name: "SCULPT HW MONO 64".into(), first_channel: 63,
            format: ChannelFormat::Mono,
        });
        s.track_mut(t).unwrap().mixer.output = Route::Hardware("SCULPT HW MONO 64".into());
        let mut e = MixEngine::new(48_000.0, 512);
        e.set_device_output_channels(64);
        let mut out = vec![vec![0.0; 512]; 64];
        e.render(&s, 0, 512, &mut out);
        assert!(out[63][100].abs() > 0.1, "hardware 64 must carry audio");
        assert!(out[0][100].abs() > 0.1, "Show bridge output must also be previewed on Main");
        e.set_device_output_channels(2);
        e.render(&s, 512, 512, &mut out);
        assert!(out[0][100].abs() > 0.1, "unavailable hardware output must be monitored via Main");
        assert!(out[63][100].abs() < 1e-6, "unavailable output must not emit a signal");
    }

'''
    p.write_text(s.replace(needle,tests+needle,1),encoding='utf-8')

PLAY='crates/playback/src/lib.rs'
change(PLAY,
'''    device_rate: f64,
    input: Option<Arc<record::InputRing>>,''',
'''    device_rate: f64,
    device_output_channels: usize, // SCULPT_CORE21_PLAY_DEVICE_FIELD
    input: Option<Arc<record::InputRing>>,''','SCULPT_CORE21_PLAY_DEVICE_FIELD')
change(PLAY,
'''            device_rate,
            input: None,''',
'''            device_rate,
            device_output_channels: nch, // SCULPT_CORE21_PLAY_DEVICE_DEFAULT
            input: None,''','SCULPT_CORE21_PLAY_DEVICE_DEFAULT')
change(PLAY,
'''                    let nch = main_channels(&s);
                    if self.pending.len() != nch {''',
'''                    // SCULPT_CORE21_PLAY_RESIZE: preserve physical channels on
                    // a session swap (especially when Show opens a multistem cue).
                    let nch = main_channels(&s).max(self.device_output_channels);
                    if self.pending.len() != nch {''','SCULPT_CORE21_PLAY_RESIZE')
change(PLAY,
'''        self.mix.monitor_only = !self.playing;
        self.mix.recording = self.recording;
        self.mix.render(&self.session, self.pos, n, &mut self.pending);''',
'''        self.mix.monitor_only = !self.playing;
        self.mix.recording = self.recording;
        self.mix.set_device_output_channels(self.device_output_channels); // SCULPT_CORE21_PLAY_ROUTE_DEVICE
        self.mix.render(&self.session, self.pos, n, &mut self.pending);''','SCULPT_CORE21_PLAY_ROUTE_DEVICE')
change(PLAY,
'''        let want_ch = u16::try_from(main_channels(&session)).unwrap_or(2);''',
'''        // SCULPT_CORE21_PLAY_REQUEST64: take hardware endpoints into account
        // before opening CoreAudio rather than silently truncating to Main 1-2.
        let want_ch = u16::try_from(sculpt_required_output_channels(&session)).unwrap_or(2);''',
'SCULPT_CORE21_PLAY_REQUEST64')
change(PLAY,
'''                let channels = usize::from(config.channels);
                let mut state = AudioState::new(rx, Arc::clone(&shared), session, f64::from(rate), gtx);''',
'''                let channels = usize::from(config.channels);
                let mut state = AudioState::new(rx, Arc::clone(&shared), session, f64::from(rate), gtx);
                state.device_output_channels = channels;
                let render_channels = channels.max(main_channels(&state.session));
                state.pending.resize_with(render_channels, || vec![0.0; BLOCK]); // SCULPT_CORE21_PLAY_64_BUFFER''',
'SCULPT_CORE21_PLAY_64_BUFFER')
# Output requirement helper; output routes only, output paths alone don't force
# a 64-channel device if nothing uses them.
change(PLAY,
'''/// Open the default output device: f32 at the session rate if possible, with as many channels as''',
'''/// SCULPT_CORE21_PLAY_DESIRED_CHANNELS: requested physical I/O width (max 64).
pub fn sculpt_required_output_channels(s: &Session) -> usize {
    let mut want = main_channels(s);
    for t in &s.tracks {
        if let soundcraft_model::Route::Hardware(path) = &t.mixer.output {
            if let Some(p) = s.outputs.iter().find(|o| o.name.eq_ignore_ascii_case(path)) {
                want = want.max(usize::from(p.first_channel).saturating_add(p.format.channels()).min(64));
            }
        }
    }
    want.clamp(2, 64)
}

/// Open the default output device: f32 at the session rate if possible, with as many channels as''',
'SCULPT_CORE21_PLAY_DESIRED_CHANNELS')
# Device fallback selects largest available native config, not a fictional 64.
# (Existing open_device already negotiates channel count <= request.)

# Existing track.output currently converts an unknown physical path into an
# internal bus by default, leaving it silent to the hardware. Keep real output
# paths as named hardware routes.
ROUTE='crates/engine/src/cmd/track.rs'
change(ROUTE,
'''        && create_bus
        && !name.to_ascii_lowercase().starts_with("in ")''',
'''        && create_bus
        && !name.to_ascii_lowercase().starts_with("in ")
        && !s.outputs.iter().any(|out| out.name.eq_ignore_ascii_case(name)) // SCULPT_CORE21_ROUTE_PRESERVE_HARDWARE''',
'SCULPT_CORE21_ROUTE_PRESERVE_HARDWARE')
# The Show handoff restores routes for explicit track IDs even if no track
# happens to be selected in the reopened session. The engine's route() function
# itself checks tracks_required(), so the broader precondition is safe.
change(ROUTE,
       'cmd!("track.output", "Track Output", [], None, "{tracks?, output: main|none|bus name}", has_selection, |e, p| route(e, p, false)),',
       'cmd!("track.output", "Track Output", [], None, "{tracks?, output: main|none|bus name|hardware path}", has_tracks, |e, p| route(e, p, false)), // SCULPT_CORE21_BRIDGE_ROUTE_WITHOUT_SELECTION',
       'SCULPT_CORE21_BRIDGE_ROUTE_WITHOUT_SELECTION')

SETUP='crates/engine/src/cmd/setup_more.rs'
change(SETUP,
'''            let first = i64_or(p, "first_channel", 0).clamp(0, 1023) as u16;
            s.outputs.push(OutputPath { name, first_channel: first, format });''',
'''            // SCULPT_CORE21_IO_LIMIT64: no invalid/unplayable hardware paths.
            let requested = i64_or(p, "first_channel", 0);
            if requested < 0 || requested as usize + format.channels() > 64 {
                return Err(bad(id, "hardware output path exceeds channel 64"));
            }
            let first = requested as u16;
            s.outputs.push(OutputPath { name, first_channel: first, format });''',
'SCULPT_CORE21_IO_LIMIT64')

# Application reads the already imported Show metadata from session track
# comments, and assigns the existing SCULPT HW paths only when the track is
# still on Main. User-modified routes remain untouched.
MAIN='apps/soundcraft/src/main.rs'
change(MAIN,
'''fn open_path(app: &mut SoundApp, p: &str) {''',
'''// SCULPT_CORE21_BRIDGE_RESTORE: Show's current import creates SCULPT HW
// physical output paths and stores stem->path in each track's comment. Resolve
// these after session load; do not require any Show source change.
fn sculpt_restore_show_routes(app: &mut SoundApp) {
    let s = app.engine.session();
    let available: Vec<String> = s.outputs.iter().map(|p| p.name.clone()).collect();
    let updates: Vec<(u64, String, String)> = s.tracks.iter().filter_map(|t| {
        if serde_json::to_value(&t.mixer.output).ok() != Some(serde_json::Value::String("Main".into())) { return None; } // SCULPT_CORE21_ROUTE_JSON
        // Migrate only the unconverted Show import. A later operator choice
        // to return a track to Main must survive a save/reopen cycle.
        if !t.comments.contains("Prepared only:") { return None; }
        let start = t.comments.find("(SCULPT HW ")? + 1;
        let tail = &t.comments[start..];
        let end = tail.find(')')?;
        let path = &tail[..end];
        let name = available.iter().find(|n| n.eq_ignore_ascii_case(path))?.clone();
        Some((t.id.0, name, t.comments.clone()))
    }).collect();
    for (id, path, comments) in updates {
        if let Err(e) = app.run("track.output", serde_json::json!({
            "tracks": [id], "output": path, "create_bus": false
        })) {
            log::warn!("Sculpt Show output route {id} -> {path}: {e}");
        } else {
            // After migration, stop treating an intentional future Main
            // assignment as a pending import route. Keep Show stem UUID/path.
            let migrated = comments.replace("Prepared only:", "Routing enabled:");
            if let Err(e) = app.run("track.comments", serde_json::json!({
                "track": id, "comments": migrated
            })) {
                log::warn!("Sculpt Show route annotation {id}: {e}");
            }
        }
    }
}

fn open_path(app: &mut SoundApp, p: &str) {''',
'SCULPT_CORE21_BRIDGE_RESTORE')
# Output Route::Main is detected via serde; no extra desktop model dependency.
change(MAIN,
'''    if let Err(e) = r {
        log::warn!("{p}: {e}");
    }
}''',
'''    if let Err(e) = r {
        log::warn!("{p}: {e}");
    } else if lower.ends_with(".scdaw") || lower.ends_with(".scraft") {
        sculpt_restore_show_routes(app); // SCULPT_CORE21_BRIDGE_ON_OPEN
        // A newly opened Show session may require a different CoreAudio channel
        // count. Reopen the output only on this explicit file-open event.
        let desired = soundcraft_playback::sculpt_required_output_channels(app.engine.session());
        let current = app.player.as_ref().map_or(0, |player| usize::from(player.device_channels));
        if desired > current && !app.is_playing() {
            app.player = Some(soundcraft_playback::Player::new(app.engine.session_arc()));
        }
        let actual = app.player.as_ref().map_or(0, |p| usize::from(p.device_channels));
        if desired > actual {
            app.ui.status = format!("Hardware routing needs {desired} channels; device exposes {actual}. Unavailable channels are monitored on Main.");
            log::warn!("Sculpt Bridge hardware output not fully available: requested {desired}, active {actual}");
        }
    }
}''',
'SCULPT_CORE21_BRIDGE_ON_OPEN')
# Restore when the user opens from Finder while the UI is already running is
# handled by open_path in Core20's Cocoa document event queue.

WIN='crates/ui-egui/src/windows.rs'
change(WIN,
'''                ui.heading("Output Paths");''',
'''                // SCULPT_CORE21_IO_MATRIX64: clear device+path distinction,
                // 8x8 channel patching, track assignment and safety warnings.
                ui.heading("Hardware I/O · 64-channel patchbay");
                let physical = app.player.as_ref().map_or(0, |p| usize::from(p.device_channels));
                let dev_name = app.player.as_ref().map_or("No audio device", |p| p.device_name.as_str());
                ui.label(format!("CoreAudio: {dev_name} · {physical} active output channels"));
                ui.label("Outputs beyond the device count stay saved and monitor on Main. Reconnect via Playback Engine after changing the interface.");
                let paths = app.engine.session().outputs.clone();
                let selected = app.engine.session().edit.selected_tracks.clone();
                ui.horizontal(|ui| {
                    ui.strong("Selected tracks:");
                    ui.label(format!("{}", selected.len()));
                    if ui.small_button("Reconnect audio device").clicked() {
                        app.player = Some(soundcraft_playback::Player::new(app.engine.session_arc()));
                    }
                });
                egui::ScrollArea::vertical().max_height(155.0).show(ui, |ui| {
                    egui::Grid::new("sculpt_hw_patch_64").num_columns(8).spacing([6.0, 4.0]).show(ui, |ui| {
                        for c in 0..64usize {
                            let hits: Vec<_> = paths.iter().filter(|p| {
                                let start = usize::from(p.first_channel);
                                c >= start && c < start + p.format.channels()
                            }).collect();
                            let tip = if hits.is_empty() { format!("Output {} · free", c+1) }
                                else { format!("Output {} · paths: {}", c+1, hits.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")) };
                            let label = if c >= physical { format!("{:02} !",c+1) }
                                else { format!("{:02} {}",c+1, if hits.is_empty() { "·" } else { "●" }) };
                            if ui.small_button(label).on_hover_text(tip).clicked() {
                                // A click creates one MONO path only if absent and patches
                                // all selected tracks; separate stereo paths can be created below.
                                let name = format!("SCULPT HW MONO {:02}",c+1);
                                if !paths.iter().any(|p| p.name == name) {
                                    let _=app.run("setup.io",json!({"action":"create_output", "name":name, "first_channel":c, "format":"mono"}));
                                }
                                for id in &selected {
                                    let _=app.run("track.output",json!({"tracks":[id.0],"output":name,"create_bus":false}));
                                }
                            }
                            if (c+1)%8==0 { ui.end_row(); }
                        }
                    });
                });
                if paths.iter().any(|p| usize::from(p.first_channel)+p.format.channels() > physical) {
                    ui.colored_label(egui::Color32::YELLOW, "Some configured output paths exceed this device's active channels; Main fallback is enabled.");
                }
                let conflicts: Vec<String> = paths.iter().enumerate().flat_map(|(i,p)| {
                    paths.iter().skip(i+1).filter_map(move |q| {
                        let a=usize::from(p.first_channel);
                        let b=usize::from(q.first_channel);
                        (a < b+q.format.channels() && b < a+p.format.channels())
                            .then(|| format!("{} ↔ {}",p.name,q.name))
                    })
                }).collect();
                if !conflicts.is_empty() {
                    ui.colored_label(egui::Color32::YELLOW,
                        format!("Overlapping hardware paths (may be intentional): {}",conflicts.join(", ")));
                }
                ui.heading("Output Paths");''',
'SCULPT_CORE21_IO_MATRIX64')
change(WIN,
'''                if ui.button("+ Add Stereo Output Path").clicked() {''',
'''                // SCULPT_CORE21_IO_MONO_STEREO: explicit physical mono/stereo paths.
                let selected_key=egui::Id::new("sculpt_new_io_first");
                let mut physical_first:usize=ui.ctx().data(|d|d.get_temp(selected_key)).unwrap_or(1);
                ui.horizontal(|ui| {
                    ui.label("Physical channel (1–64):");
                    ui.add(egui::DragValue::new(&mut physical_first).range(1..=64));
                });
                ui.ctx().data_mut(|d|d.insert_temp(selected_key,physical_first));
                if ui.button("+ Add Mono Output Path").clicked() {
                    let name=format!("SCULPT HW MONO {:02}",physical_first);
                    let _=app.run("setup.io",json!({"action":"create_output","name":name,"first_channel":physical_first-1,"format":"mono"}));
                }
                if ui.button("+ Add Stereo Output Path").clicked() {''',
'SCULPT_CORE21_IO_MONO_STEREO')
# Modify existing +Stereo to follow the chosen physical channel; still preserve
# old sequential default if UI wasn't used, now use explicit mapping.
change(WIN,
'''                    let paths = &app.engine.session().outputs;
                    let first = paths.iter().map(|p| p.first_channel as usize + p.format.channels()).max().unwrap_or(0);
                    let name = format!("Out {}-{}", first + 1, first + 2);
                    let _ = app.run("setup.io", json!({"action": "create_output", "name": name, "first_channel": first, "format": "stereo"}));''',
'''                    let first = physical_first.saturating_sub(1).min(62); // SCULPT_CORE21_STEREO_EXPLICIT
                    let name = format!("SCULPT HW STEREO {:02}-{:02}", first + 1, first + 2);
                    let _ = app.run("setup.io", json!({"action": "create_output", "name": name, "first_channel": first, "format": "stereo"}));''',
'SCULPT_CORE21_STEREO_EXPLICIT')
# A proper output-path selector for the current selected tracks, no bus aliases.
change(WIN,
'''                if ui.button("+ Add Mono Output Path").clicked() {''',
'''                // SCULPT_CORE21_IO_ASSIGN: assign saved paths to selected channels.
                let saved_paths=app.engine.session().outputs.clone();
                ui.horizontal(|ui| {
                    ui.label("Assign selected track output:");
                    egui::ComboBox::from_id_salt("sculpt_hw_select_path")
                        .selected_text("Select hardware path...")
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(false, "Main monitor").clicked() {
                                for id in &selected { let _=app.run("track.output",json!({"tracks":[id.0],"output":"main","create_bus":false})); }
                            }
                            for path in &saved_paths {
                                if ui.selectable_label(false, &path.name).clicked() {
                                    for id in &selected { let _=app.run("track.output",json!({"tracks":[id.0],"output":path.name,"create_bus":false})); }
                                }
                            }
                        });
                });
                if ui.button("+ Add Mono Output Path").clicked() {''',
'SCULPT_CORE21_IO_ASSIGN')
# Expose physical paths in every mixer channel's output chooser, avoiding
# hidden-only I/O assignments in the Setup dialog.
MIX_UI='crates/ui-egui/src/mix_window.rs'
change(MIX_UI,
'''    ui.separator();
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();''',
'''    // SCULPT_CORE21_MIX_OUTPUT_MENU: direct hardware output selection.
    if !input {
        let paths = app.engine.session().outputs.clone();
        ui.separator();
        ui.strong("Hardware outputs");
        for path in paths.iter().skip(1) {
            let end=usize::from(path.first_channel)+path.format.channels();
            if ui.button(format!("{} · Ch {}–{}",path.name,path.first_channel+1,end)).clicked() {
                let _=app.run("track.output",json!({"tracks":[id.0],"output":path.name,"create_bus":false}));
                ui.close();
            }
        }
        if ui.button("I/O Setup...").clicked() {
            app.ui.show_io_setup=true;
            ui.close();
        }
    }
    ui.separator();
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();''',
'SCULPT_CORE21_MIX_OUTPUT_MENU')

# Versioned DAW-side feature negotiation: Show may query this without starting
# a session or confusing it with a headless CLI runtime.
change(MAIN,
'''            "cliRelativePath": "Contents/Helpers/soundcraft-cli",''',
'''            "cliRelativePath": "Contents/Helpers/soundcraft-cli",
            "hardwareRouting": {"enabled": true, "maxOutputChannels": 64,
                                "pathFormats": ["mono", "stereo"],
                                "bridgeMainPreview": true,
                                "unavailableOutputFallback": "main"}, // SCULPT_CORE21_BRIDGE_HW_CAPS''',
'SCULPT_CORE21_BRIDGE_HW_CAPS')

# Required functionality is visible from existing Setup > I/O menu,
# in Edit's lower Channel Strip -> Routing..., and in Mix's menu.

# Guard expected patched subsystems. A stale/rebased workflow must stop rather
# than quietly ship a build with routes still silent.
checks={
 MIX:['SCULPT_CORE21_HW_RENDER','SCULPT_CORE21_HW_DEVICE_SUM','SCULPT_CORE21_TEST_OUTPUT64'],
 PLAY:['SCULPT_CORE21_PLAY_REQUEST64','SCULPT_CORE21_PLAY_64_BUFFER','SCULPT_CORE21_PLAY_DESIRED_CHANNELS'],
 ROUTE:['SCULPT_CORE21_ROUTE_PRESERVE_HARDWARE', 'SCULPT_CORE21_BRIDGE_ROUTE_WITHOUT_SELECTION'],
 MAIN:['SCULPT_CORE21_BRIDGE_ON_OPEN','SCULPT_CORE21_BRIDGE_RESTORE','SCULPT_CORE21_BRIDGE_HW_CAPS'],
 WIN:['SCULPT_CORE21_IO_MATRIX64','SCULPT_CORE21_IO_ASSIGN'],
 MIX_UI:['SCULPT_CORE21_MIX_OUTPUT_MENU'],
 SETUP:['SCULPT_CORE21_IO_LIMIT64'],
}
for path, markers in checks.items():
    s=(ROOT/path).read_text(encoding='utf-8')
    for mark in markers:
        if s.count(mark)!=1:
            raise AssertionError(f'{path}: {mark}: expected exactly one occurrence, got {s.count(mark)}')
print('SCULPT CORE21: hardware output, 64-channel matrix, Show mapping source guards OK')
