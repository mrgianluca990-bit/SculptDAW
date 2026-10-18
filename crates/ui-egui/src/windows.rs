//! Secondary floating windows (Window menu).

use crate::SoundApp;
use crate::theme::{Tokens, mono, rgb};
use egui::{Color32, Sense, vec2};
use serde_json::json;
use soundcraft_model::{AutomationMode, ClipContent};

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    audio_health(app, ctx);
    automation(app, ctx);
    color_palette(app, ctx);
    disk_usage(app, ctx);
    system_usage(app, ctx);
    task_manager(app, ctx);
    metadata(app, ctx);
    event_list(app, ctx);
    midi_keyboard(app, ctx);
    workspace(app, ctx);
    configurations(app, ctx);
    playback_engine(app, ctx);
    io_setup(app, ctx);
    shortcuts_window(app, ctx);
    clip_effects(app, ctx);
}

fn audio_health(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_audio_health;
    win(ctx, &mut open, "Session Audio Health", vec2(620.0, 360.0), |ui| {
        let refresh = ui.button("Refresh Report").clicked();
        if app.audio_health_report.is_none() || refresh {
            app.audio_health_report = Some(app.engine.execute("session.audio_health", &json!({})).map_err(|error| error.to_string()));
        }
        let Some(result) = &app.audio_health_report else { return };
        match result {
            Ok(report) => {
                ui.label(format!("{} audio clips checked · {} issues", report["audio_clips"], report["issue_count"]));
                ui.label(match report["active_audio_end_seconds"].as_f64() {
                    Some(seconds) => format!("Active audio ends at {seconds:.2} seconds"),
                    None => "Active audio end time is unknown (invalid session sample rate)".into(),
                });
                ui.label("Includes alternate takes and muted clips. Refresh after editing.");
                ui.separator();
                if report["healthy"] == true {
                    ui.label("All audio clips have loaded media within source bounds.");
                }
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    if let Some(issues) = report["issues"].as_array() {
                        for issue in issues {
                            ui.group(|ui| {
                                let text = |key: &str| issue[key].as_str().unwrap_or("");
                                ui.label(format!(
                                    "{} / {} / {}{}",
                                    text("track"),
                                    text("playlist"),
                                    text("clip"),
                                    if issue["active"] == true { "" } else { " (alternate playlist)" }
                                ));
                                ui.label(text("message"));
                                if let Some(path) = issue["path"].as_str() {
                                    ui.label(path);
                                }
                            });
                        }
                    }
                });
                ui.separator();
                ui.label("Checks loaded media only, not files on disk, signal levels, plugins or routing.");
            }
            Err(error) => {
                ui.label(error.to_string());
            }
        }
    });
    app.ui.show_audio_health = open;
    if !open {
        app.audio_health_report = None;
    }
}

fn win(ctx: &egui::Context, open: &mut bool, title: &str, size: egui::Vec2, body: impl FnOnce(&mut egui::Ui)) {
    if !*open {
        return;
    }
    egui::Window::new(title).open(open).default_size(size).show(ctx, body);
}

fn automation(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_automation;
    win(ctx, &mut open, "Automation", vec2(260.0, 260.0), |ui| {
        ui.label("Write automation for the selected tracks:");
        let tracks = app.engine.session().edit.selected_tracks.clone();
        ui.horizontal_wrapped(|ui| {
            for m in [
                AutomationMode::Off,
                AutomationMode::Read,
                AutomationMode::Touch,
                AutomationMode::Latch,
                AutomationMode::TouchLatch,
                AutomationMode::Write,
                AutomationMode::Trim,
            ] {
                if ui.button(m.label()).clicked() {
                    let ids: Vec<u64> = tracks.iter().map(|t| t.0).collect();
                    let _ = app.run("mix.automation_mode", json!({"tracks": ids, "mode": m.label()}));
                }
            }
        });
        ui.separator();
        ui.label("Enabled parameters");
        for f in ["volume", "pan", "mute", "send level", "send pan", "send mute", "plugin"] {
            let id = format!("automation.enable.{}", f.replace(' ', "_"));
            let mut on = !app.engine.session().edit.flag(&format!("{id}.off"));
            if ui.checkbox(&mut on, f).changed() {
                app.engine.session_mut().edit.set_flag(&format!("{id}.off"), !on);
            }
        }
        ui.separator();
        if ui.button("Write to Current").clicked() {
            let _ = app.run("automation.write_to_current", json!({}));
        }
        if ui.button("Thin All").clicked() {
            let _ = app.run("automation.thin_all", json!({}));
        }
    });
    app.ui.show_automation = open;
}

fn color_palette(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_color_palette;
    win(ctx, &mut open, "Color Palette", vec2(300.0, 140.0), |ui| {
        ui.label("Apply to selected tracks (Shift: selected clips)");
        ui.horizontal_wrapped(|ui| {
            for (i, c) in soundcraft_model::TRACK_COLORS.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                ui.painter().rect_filled(r, 3.0, rgb(*c));
                if resp.clicked() {
                    if ui.input(|x| x.modifiers.shift) {
                        let _ = app.run("clip.color", json!({"color": c}));
                    } else {
                        let _ = app.run("track.color", json!({"color": i}));
                    }
                }
            }
        });
    });
    app.ui.show_color_palette = open;
}

fn disk_usage(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_disk_usage;
    win(ctx, &mut open, "Disk Usage", vec2(360.0, 160.0), |ui| {
        let s = app.engine.session();
        let bytes: u64 = s.sources.iter().map(|x| x.frames * u64::from(x.channels) * 4).sum();
        ui.label(format!("Session media: {} files, {:.1} MB in memory (32-bit float)", s.sources.len(), bytes as f64 / 1_048_576.0));
        let per_min = f64::from(s.sample_rate.hz()) * 60.0 * 3.0 / 1_048_576.0;
        ui.label(format!("Recording at 24-bit uses {per_min:.1} MB per mono track-minute"));
        if let Some(p) = &app.engine.path {
            ui.label(format!("Session file: {p}"));
        }
    });
    app.ui.show_disk_usage = open;
}

fn system_usage(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_system_usage;
    win(ctx, &mut open, "System Usage", vec2(320.0, 140.0), |ui| {
        ui.label(format!("UI frame time: {:.1} ms", app.frame_ms));
        ui.label(format!("Audio: {}", app.player.as_ref().map_or("no engine".to_string(), |p| format!("{} @ {} Hz", p.device_name, p.device_rate))));
        let plugins: usize = app.engine.session().tracks.iter().map(|t| t.mixer.inserts.iter().flatten().count()).sum();
        ui.label(format!("Active plugin instances: {plugins}"));
        ui.label(format!("Tracks: {}", app.engine.session().tracks.len()));
        ui.label(format!("Undo steps: {}", app.engine.undo_history().len()));
    });
    app.ui.show_system_usage = open;
}

fn task_manager(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_task_manager;
    win(ctx, &mut open, "Task Manager", vec2(320.0, 120.0), |ui| {
        ui.label("No background tasks are running.");
        ui.label(
            egui::RichText::new("Renders, bounces and AudioSuite processes run to completion before returning.")
                .small()
                .color(Tokens::current().text_dim),
        );
    });
    app.ui.show_task_manager = open;
}

fn metadata(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_metadata;
    win(ctx, &mut open, "Metadata Inspector", vec2(380.0, 260.0), |ui| {
        let s = app.engine.session();
        ui.label(egui::RichText::new(&s.name).strong());
        egui::Grid::new("meta").num_columns(2).striped(true).show(ui, |ui| {
            for (k, v) in [
                ("Sample rate", format!("{} Hz", s.sample_rate.hz())),
                ("Bit depth", format!("{:?}", s.bit_depth)),
                ("Timecode rate", s.frame_rate.label().to_string()),
                ("Tracks", s.tracks.len().to_string()),
                ("Clips", s.tracks.iter().map(|t| t.clips().len()).sum::<usize>().to_string()),
                ("Audio files", s.sources.len().to_string()),
                ("Markers", s.markers.len().to_string()),
                (
                    "Length",
                    soundcraft_time::format_position(s.content_end(), soundcraft_time::TimeFormat::MinSecs, s.sample_rate, &s.tempo, s.frame_rate, 0),
                ),
            ] {
                ui.label(k);
                ui.label(v);
                ui.end_row();
            }
        });
        ui.separator();
        let mut c = s.comments.clone();
        ui.label("Session comments");
        if ui.text_edit_multiline(&mut c).changed() {
            app.engine.session_mut().comments = c;
        }
    });
    app.ui.show_metadata = open;
}

fn event_list(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_event_list;
    win(ctx, &mut open, "MIDI Event List", vec2(420.0, 360.0), |ui| {
        let Some(cid) = crate::midi_editor::target_clip(app) else {
            ui.label("Select a MIDI clip.");
            return;
        };
        let notes = match app.engine.session().find_clip(cid).map(|(_, c)| c.content.clone()) {
            Some(ClipContent::Midi { sequence }) => sequence.notes,
            _ => Vec::new(),
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("evl").num_columns(5).striped(true).show(ui, |ui| {
                for h in ["Start (ticks)", "Note", "Vel", "Length", ""] {
                    ui.label(egui::RichText::new(h).strong());
                }
                ui.end_row();
                for (i, n) in notes.iter().enumerate() {
                    ui.label(egui::RichText::new(n.start.to_string()).font(mono(11.0)));
                    ui.label(soundcraft_midi::note_name(n.pitch));
                    let mut v = i64::from(n.velocity);
                    if ui.add(egui::DragValue::new(&mut v).range(1..=127)).changed() {
                        let _ = app.engine.execute("midi.note_edit", &json!({"clip": cid.0, "index": i, "velocity": v}));
                    }
                    ui.label(n.length.to_string());
                    if ui.small_button("✕").clicked() {
                        let _ = app.run("midi.note_delete", json!({"clip": cid.0, "indices": [i]}));
                    }
                    ui.end_row();
                }
            });
        });
    });
    app.ui.show_event_list = open;
}

fn midi_keyboard(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_midi_keyboard;
    win(ctx, &mut open, "MIDI Keyboard", vec2(640.0, 110.0), |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(400.0), 80.0), Sense::click());
        let whites = 28;
        let w = r.width() / whites as f32;
        let mut wi = 0;
        let base = 48u8;
        let mut clicked: Option<u8> = None;
        let pos = resp.interact_pointer_pos();
        for p in base..base + 48 {
            if !matches!(p % 12, 1 | 3 | 6 | 8 | 10) {
                let kr = egui::Rect::from_min_size(egui::pos2(r.min.x + wi as f32 * w, r.min.y), vec2(w - 1.0, r.height()));
                ui.painter().rect_filled(kr, 2.0, Color32::from_rgb(235, 235, 235));
                if resp.clicked() && pos.is_some_and(|q| kr.contains(q)) {
                    clicked = Some(p);
                }
                wi += 1;
            }
        }
        wi = 0;
        for p in base..base + 48 {
            if matches!(p % 12, 1 | 3 | 6 | 8 | 10) {
                let kr = egui::Rect::from_min_size(egui::pos2(r.min.x + wi as f32 * w - w * 0.3, r.min.y), vec2(w * 0.6, r.height() * 0.6));
                ui.painter().rect_filled(kr, 2.0, Color32::from_rgb(20, 20, 20));
                if resp.clicked() && pos.is_some_and(|q| kr.contains(q)) {
                    clicked = Some(p);
                }
            } else {
                wi += 1;
            }
        }
        if let Some(p) = clicked
            && let Some(cid) = crate::midi_editor::target_clip(app)
        {
            // Step input: add the note at the insertion point (relative to the clip) and advance.
            let s = app.engine.session();
            let (clip_start, at) = (s.find_clip(cid).map_or(0, |(_, c)| c.start), s.edit.selection.start);
            let rel = s.tempo.samples_to_ticks(at, s.sample_rate) - s.tempo.samples_to_ticks(clip_start, s.sample_rate);
            let len = soundcraft_time::TICKS_PER_QUARTER / 2;
            let _ = app.run("midi.note_add", json!({"clip": cid.0, "pitch": p, "start_ticks": rel.max(0), "length_ticks": len}));
            let s = app.engine.session();
            let next = s.tempo.tick_to_samples(s.tempo.samples_to_ticks(at, s.sample_rate) + len, s.sample_rate);
            let _ = app.run("transport.locate", json!({"at": next}));
        }
        ui.label(egui::RichText::new("Click keys to step-enter notes into the selected MIDI clip at the insertion point.").small());
    });
    app.ui.show_midi_keyboard = open;
}

fn workspace(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_workspace;
    if !open {
        return;
    }
    egui::Window::new("Workspace").open(&mut open).default_size(vec2(460.0, 420.0)).show(ctx, |ui| {
        let dir = if app.ui.workspace_dir.is_empty() { std::env::var("HOME").unwrap_or_else(|_| ".".into()) } else { app.ui.workspace_dir.clone() };
        ui.horizontal(|ui| {
            if ui.button("⬆").on_hover_text("Parent folder").clicked()
                && let Some(p) = std::path::Path::new(&dir).parent()
            {
                app.ui.workspace_dir = p.to_string_lossy().into_owned();
            }
            ui.label(egui::RichText::new(&dir).small());
        });
        ui.separator();
        let mut entries: Vec<(String, bool)> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.starts_with('.') {
                            return None;
                        }
                        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                        let ext = std::path::Path::new(&name).extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                        (is_dir
                            || [
                                "wav", "aif", "aiff", "flac", "mp3", "ogg", "m4a", "caf", "mid", "midi", "scraft",
                                "scdaw", /* SCULPT_CORE18_WORKSPACE_NATIVE */
                            ]
                            .contains(&ext.as_str()))
                        .then_some((name, is_dir))
                    })
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (name, is_dir) in entries.into_iter().take(2000) {
                let path = std::path::Path::new(&dir).join(&name).to_string_lossy().into_owned();
                let label = if is_dir { format!("📁 {name}") } else { format!("🔊 {name}") };
                let r = ui.selectable_label(false, label);
                if r.double_clicked() {
                    if is_dir {
                        app.ui.workspace_dir = path;
                    } else if name.ends_with(".scraft") || name.ends_with(".scdaw") {
                        // SCULPT_CORE18_WORKSPACE_OPEN
                        let _ = crate::dialogs::run_or_ask(app, "session.open", json!({"path": path}));
                    } else if name.ends_with(".mid") || name.ends_with(".midi") {
                        let _ = app.run("file.import_midi", json!({"path": path}));
                    } else {
                        let at = app.engine.session().edit.selection.start;
                        let _ = app.run("file.import_audio", json!({"path": path, "at": at}));
                    }
                }
            }
        });
        ui.label(egui::RichText::new("Double-click to import audio/MIDI at the insertion point or open a session.").small());
    });
    app.ui.show_workspace = open;
}

fn configurations(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_configurations;
    win(ctx, &mut open, "Window Configurations", vec2(320.0, 240.0), |ui| {
        if ui.button("New Configuration…").clicked() {
            let n = app.ui.configurations.len() + 1;
            let mut snap = app.ui.clone();
            snap.configurations.clear();
            if let Ok(v) = serde_json::to_value(&snap) {
                app.ui.configurations.push((format!("Configuration {n}"), v));
            }
        }
        ui.separator();
        let list = app.ui.configurations.clone();
        for (i, (name, v)) in list.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.button(format!("{}  {name}", i + 1)).clicked()
                    && let Ok(mut st) = serde_json::from_value::<crate::UiState>(v.clone())
                {
                    if let Err(error) = st.docking.validate().and_then(|()| st.mix_docking.validate_mix()) {
                        app.ui.status = format!("Window configuration could not be restored: {error}");
                    } else {
                        st.configurations = app.ui.configurations.clone();
                        st.theme = app.ui.theme;
                        st.show_configurations = true;
                        crate::docking::cancel_replaced_state(app, &st);
                        app.ui = st;
                    }
                }
                if ui.small_button("✕").clicked() {
                    app.ui.configurations.remove(i);
                }
            });
        }
    });
    app.ui.show_configurations = open;
}

/// Setup › Playback Engine / Hardware.
/// SCULPT_CORE22_AUDIO_SETTINGS: CoreAudio setup MUST precede I/O routing.
pub fn playback_engine(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_playback_engine;
    win(ctx, &mut open, "Sculpt · Audio Settings", vec2(580.0, 435.0), |ui| {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use soundcraft_playback::sculpt_audio_settings::{AudioSettings, available_output_devices};
            let id = egui::Id::new("sculpt_core22_audio_settings_draft");
            let mut cfg: AudioSettings = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_else(AudioSettings::read);
            ui.heading("CoreAudio interface");
            ui.label("A shared device profile is used by Sculpt Show and Sculpt DAW.");
            let devices = available_output_devices();
            ui.horizontal(|ui| {
                ui.label("Output device");
                egui::ComboBox::from_id_salt("sculpt_core22_audio_output")
                    .selected_text(if cfg.output_name.is_empty() { "System default" } else { &cfg.output_name })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut cfg.output_name, String::new(), "System default");
                        for d in &devices {
                            ui.selectable_value(&mut cfg.output_name, d.name.clone(), format!("{} · {} out", d.name, d.channels));
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Input device");
                ui.text_edit_singleline(&mut cfg.input_name)
                    .on_hover_text("Optional exact CoreAudio input device name. Leave blank for default input.");
            });
            ui.horizontal(|ui| {
                ui.label("Sample rate");
                egui::ComboBox::from_id_salt("sculpt_core22_rate")
                    .selected_text(if cfg.sample_rate == 0 { "Session rate".into() } else { format!("{} Hz", cfg.sample_rate) })
                    .show_ui(ui, |ui| {
                        for rate in [0, 44100, 48000, 88200, 96000, 176400, 192000] {
                            ui.selectable_value(&mut cfg.sample_rate, rate, if rate == 0 { "Session rate".into() } else { format!("{} Hz", rate) });
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Buffer");
                egui::ComboBox::from_id_salt("sculpt_core22_buffer")
                    .selected_text(if cfg.buffer_frames == 0 { "Device default".into() } else { format!("{} samples", cfg.buffer_frames) })
                    .show_ui(ui, |ui| {
                        for frames in [0, 64, 128, 256, 512, 1024, 2048] {
                            ui.selectable_value(
                                &mut cfg.buffer_frames,
                                frames,
                                if frames == 0 { "Device default".into() } else { format!("{} samples", frames) },
                            );
                        }
                    });
            });
            if let Some(device) = &app.player {
                ui.separator();
                ui.label(format!("Active: {} · {} Hz · {} output channels", device.device_name, device.device_rate, device.device_channels));
            }
            // SCULPT_CORE23_RATE_WARNING: distinguish device rate from session rate.
            let project_rate = app.engine.session().sample_rate.hz();
            ui.label(format!(
                "Session: {} Hz · Device target: {} Hz",
                project_rate,
                if cfg.sample_rate == 0 { project_rate } else { cfg.sample_rate }
            ));
            if cfg.sample_rate > 0 && cfg.sample_rate != project_rate {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "WARNING: hardware rate differs from session; playback is resampled. The session rate is not changed.",
                );
            }
            ui.label("A device change is rejected during playback or recording. Stop transport first.");
            let running = app.engine.transport.playing || app.engine.transport.recording;
            if ui.add_enabled(!running, egui::Button::new("Apply and reconnect audio")).clicked() {
                let previous = AudioSettings::read();
                // A new choice by name must be resolved to an UID by Show when next opened.
                if previous.output_name != cfg.output_name {
                    cfg.output_uid.clear();
                }
                match cfg.save() {
                    Ok(()) => {
                        let trial = soundcraft_playback::Player::new(app.engine.session_arc());
                        if trial.silent {
                            let _ = previous.save();
                            app.ui.status = "Audio setup failed; previous device configuration restored".into();
                        } else {
                            app.player = Some(trial);
                            app.ui.status = "Audio interface reconnected; reopen Sculpt Show when idle to share it".into();
                        }
                    }
                    Err(e) => app.ui.status = format!("Cannot save audio settings: {e}"),
                }
            }
            ui.ctx().data_mut(|d| d.insert_temp(id, cfg));
        }
        #[cfg(target_arch = "wasm32")]
        ui.label("Native CoreAudio settings require the macOS desktop app.");
    });
    app.ui.show_playback_engine = open;
}

/// Setup › I/O: busses and output paths.
/// SCULPT_CORE22_SIMPLE_IO: no user-editable matrix, just physical destinations.
pub fn io_setup(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_io_setup;
    win(ctx, &mut open, "Sculpt · I/O Setup", vec2(540.0, 490.0), |ui| {
        let count = app.player.as_ref().map(|p| usize::from(p.device_channels)).unwrap_or(0).min(64);
        ui.horizontal(|ui| {
            ui.heading("Physical outputs");
            if ui.button("Audio Settings…").clicked() {
                app.ui.show_playback_engine = true;
            }
        });
        ui.label(app.player.as_ref().map_or("No connected audio output".into(), |p| format!("{} · {} active outputs", p.device_name, count)));
        let selected = app.engine.session().edit.selected_tracks.first().copied();
        let Some(track) = selected else {
            ui.separator();
            ui.label("Select a track in Edit or Mix to assign its output.");
            return;
        };
        let current = app.engine.session().track(track).map(|t| format!("{:?}", t.mixer.output)).unwrap_or_default();
        ui.label(format!("Track output: {current}"));
        if ui.button("Main (monitor)").clicked() {
            let _ = app.run("track.output", json!({"tracks":[track.0],"output":"main","create_bus":false}));
        }
        ui.separator();
        ui.label("Assign channel(s), with the same numbering used in Sculpt Show:");
        egui::ScrollArea::vertical().max_height(275.0).show(ui, |ui| {
            if count == 0 {
                ui.label("Connect an interface in Audio Settings first.");
                return;
            }
            for first in 0..count {
                ui.horizontal(|ui| {
                    let mono = format!("SCULPT HW MONO {:02}", first + 1);
                    if ui.button(format!("Out {}", first + 1)).clicked() {
                        let exists = app.engine.session().outputs.iter().any(|p| p.name == mono);
                        if !exists {
                            let _ = app.run("setup.io", json!({"action":"create_output","name":mono,"first_channel":first,"format":"mono"}));
                        }
                        let _ = app.run("track.output", json!({"tracks":[track.0],"output":mono,"create_bus":false}));
                    }
                    if first + 1 < count {
                        let stereo = format!("SCULPT HW STEREO {:02}-{:02}", first + 1, first + 2);
                        if ui.button(format!("Out {}–{}", first + 1, first + 2)).clicked() {
                            let exists = app.engine.session().outputs.iter().any(|p| p.name == stereo);
                            if !exists {
                                let _ = app.run("setup.io", json!({"action":"create_output","name":stereo,"first_channel":first,"format":"stereo"}));
                            }
                            let _ = app.run("track.output", json!({"tracks":[track.0],"output":stereo,"create_bus":false}));
                        }
                    }
                });
            }
        });
    });
    app.ui.show_io_setup = open;
}

/// Setup › Keyboard Shortcuts.
pub fn shortcuts_window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_shortcuts;
    let mac = ctx.os().is_mac();
    win(ctx, &mut open, "Keyboard Shortcuts", vec2(520.0, 480.0), |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("sc").num_columns(3).striped(true).show(ui, |ui| {
                let engine = soundcraft_engine::command_specs().iter().map(|c| (c.shortcut, c.label, c.menu.join(" › ")));
                let windows = crate::menus::UI_COMMANDS.iter().map(|c| (c.3, c.1, c.2.replace(" > ", " › ")));
                for (shortcut, label, menu) in engine.chain(windows) {
                    let Some(shortcut) = shortcut else { continue };
                    ui.label(egui::RichText::new(crate::shortcuts::shortcut_label(shortcut, mac)).font(mono(11.0)));
                    ui.label(label);
                    ui.label(egui::RichText::new(menu).small());
                    ui.end_row();
                }
            });
        });
    });
    app.ui.show_shortcuts = open;
}

/// Clip Effects: per-clip EQ, dynamics and gain (stored as `clip_fx.<clip>.*` values).
pub fn clip_effects(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_clip_effects;
    win(ctx, &mut open, "Clip Effects", vec2(560.0, 420.0), |ui| {
        let clip = app.engine.session().edit.selected_clips.first().copied().or_else(|| {
            let s = app.engine.session();
            s.tracks
                .iter()
                .filter(|t| s.edit.selected_tracks.contains(&t.id))
                .flat_map(|t| t.clips().iter())
                .find(|c| c.range().contains(s.edit.selection.start) && c.is_audio())
                .map(|c| c.id)
        });
        let Some(cid) = clip else {
            ui.label("Select an audio clip.");
            return;
        };
        let name = app.engine.session().find_clip(cid).map(|(_, c)| c.name.clone()).unwrap_or_default();
        let bypass = app.engine.session().edit.flag(&format!("clip_fx.bypass.{}", cid.0));
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&name).strong());
            if ui.selectable_label(bypass, "Bypass").clicked() {
                let _ = app.run("clip.effects_bypass", json!({"clips": [cid.0]}));
            }
            if ui.button("Render").clicked() {
                let _ = app.run("clip.effects_render", json!({"clips": [cid.0]}));
            }
            if ui.button("Clear").clicked() {
                let _ = app.run("edit.clear_clip_effects", json!({"clips": [cid.0]}));
            }
        });
        let get = |app: &SoundApp, k: &str, d: f32| app.engine.session().edit.values.get(&format!("clip_fx.{}.{k}", cid.0)).map_or(d, |v| *v as f32);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |cols| {
                for (col, (title, plugin, prefix)) in cols.iter_mut().zip([("EQ", "eq_7band", "eq."), ("Dynamics", "compressor", "comp.")]) {
                    col.label(egui::RichText::new(title).strong());
                    let Some(info) = soundcraft_dsp::plugin_info(plugin) else { continue };
                    for p in info.params {
                        let key = format!("{prefix}{}", p.id);
                        let mut v = get(app, &key, p.default);
                        let before = v;
                        col.add(egui::Slider::new(&mut v, p.min..=p.max).text(p.name));
                        if (v - before).abs() > f32::EPSILON {
                            let mut m = serde_json::Map::new();
                            m.insert(key.clone(), json!(v));
                            let _ = app.engine.execute_merged(
                                "clip.effects_set",
                                &json!({"clips": [cid.0], "params": m}),
                                &format!("cfx:{}:{key}", cid.0),
                            );
                        }
                    }
                }
            });
            let mut g = get(app, "gain", 0.0);
            let before = g;
            ui.add(egui::Slider::new(&mut g, -36.0..=24.0).text("Clip effects gain (dB)"));
            if (g - before).abs() > f32::EPSILON {
                let _ =
                    app.engine.execute_merged("clip.effects_set", &json!({"clips": [cid.0], "params": {"gain": g}}), &format!("cfx:{}:gain", cid.0));
            }
        });
    });
    app.ui.show_clip_effects = open;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod audio_health_tests {
    use super::*;

    #[test]
    fn window_command_renders_report_and_close_clears_snapshot() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.run("window.audio_health", json!({"value": true})).unwrap();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| audio_health(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert_eq!(app.audio_health_report.as_ref().unwrap().as_ref().unwrap()["healthy"], true);
        assert!(!app.engine.is_dirty());
        app.run("window.hide_floating", json!({})).unwrap();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| audio_health(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert!(!app.ui.show_audio_health);
        assert!(app.audio_health_report.is_none());
        app.run("window.audio_health", json!({"value": true})).unwrap();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| audio_health(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert!(app.audio_health_report.is_some());
    }
}
