//! SoundCraft desktop app.
//!
//! Usage: `soundcraft [--demo] [--control <port>] [--no-audio] [session.scraft | audio files…]`
//!
//! `--control <port>` (or `SOUNDCRAFT_CONTROL_PORT`) starts a localhost JSON-lines control server
//! (`0` picks a free port and prints it): `{"id":1,"method":"ui.inspect","params":{}}` →
//! `{"id":1,"ok":true,"result":…}`. See `soundcraft_ui_egui::control` for the methods.
//!
//! `log` records go to standard error and `<settings dir>/logs/soundcraft.log` (`RUST_LOG` sets the
//! levels); see [`logging`].
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod config;
mod control_server;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod logging;
#[cfg(target_os = "macos")]
mod native_menu;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // SCULPT_CORE21_FINDER_FFI_SCOPE: this one FFI module only
mod sculpt_open_documents; // SCULPT_CORE20_FINDER_MODULE

use soundcraft_engine::Engine;
use soundcraft_ui_egui::{Services, SoundApp, UiState, dialogs};
use std::sync::Arc;

struct App(SoundApp, Option<&'static logging::AppLogger>, #[cfg(target_os = "macos")] Option<native_menu::NativeMenu>);

/// Sends the title only when it changes, remembered here as X11 and Wayland can't read it back.
fn set_title(ctx: &egui::Context, title: String) {
    let id = egui::Id::new("window_title");
    if ctx.data(|d| d.get_temp::<String>(id)).as_deref() != Some(title.as_str()) {
        ctx.data_mut(|d| d.insert_temp(id, title.clone()));
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // What the realtime audio thread logged is written here, off that thread.
        if let Some(logger) = self.1 {
            logger.report_audio_thread();
        }
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.2 {
            menu.process(&mut self.0);
        }
        self.0.logic(ctx);
        #[cfg(target_os = "macos")]
        sculpt_drain_finder_documents(&mut self.0); // SCULPT_CORE19_FINDER_POLL
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.2 {
            menu.sync(&self.0);
        }
        // SCULPT_CORE20_FINDER_DRAIN: queued macOS Open Document events are
        // delivered on the GUI thread and use the existing document handler.
        #[cfg(target_os = "macos")]
        for path in sculpt_open_documents::drain() {
            open_path(&mut self.0, &path);
        }
        // Files dropped on the window: sessions open, audio/MIDI import.
        let dropped: Vec<String> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_string_lossy().into_owned()).filter(|s| !s.is_empty()).collect());
        for p in dropped {
            open_path(&mut self.0, &p);
        }
        if self.0.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) && dialogs::ask_to_save(&mut self.0, "app.quit", &serde_json::json!({})) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        soundcraft_ui_egui::fit_window_to_screen(ctx);
        let (inner, maximized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().maximized.unwrap_or(false)));
        self.0.ui.window_maximized = maximized;
        if let Some(r) = inner.filter(|_| !maximized) {
            self.0.ui.window_size = (r.size() * ctx.zoom_factor()).round().into();
        }
        self.0.ui.ui_scale = self.0.extra.ui_scale;
        // With the system menu bar there is no egui bar showing the window and dirty state, so the
        // title carries them; elsewhere the egui menu bar already does.
        let title = if self.0.native_menu_bar {
            format!(
                "{}{} — Sculpt DAW — {}",
                self.0.engine.session().name,
                if self.0.engine.is_dirty() { " *" } else { "" },
                if self.0.ui.window == soundcraft_ui_egui::MainWindow::Edit { "Edit" } else { "Mix" }
            )
        } else {
            format!("{} — Sculpt DAW", self.0.engine.session().name)
        };
        set_title(ctx, title);
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }
    fn on_exit(&mut self) {
        save_prefs(&self.0.ui);
        if let Some(logger) = self.1 {
            logger.report_audio_thread();
        }
        // The system Quit (⌘Q) ends the process from inside the event loop, so `run_native` never
        // returns: release the player's plugin instances and shut plugin hosting down here.
        // SCULPT_ORDERLY_PLUGIN_SHUTDOWN_V1: synchronize plugin ownership before
        // calling VST3 bundle exits, which are sensitive to thread ordering.
        if let Some(mut player) = self.0.player.take() {
            player.shutdown();
        }
        soundcraft_engine::shutdown_plugin_hosts();
    }
}

// SCULPT_CORE21_BRIDGE_RESTORE: Show's current import creates SCULPT HW
// physical output paths and stores stem->path in each track's comment. Resolve
// these after session load; do not require any Show source change.
fn sculpt_restore_show_routes(app: &mut SoundApp) {
    let s = app.engine.session();
    let available: Vec<String> = s.outputs.iter().map(|p| p.name.clone()).collect();
    let updates: Vec<(u64, String, String)> = s
        .tracks
        .iter()
        .filter_map(|t| {
            if serde_json::to_value(&t.mixer.output).ok() != Some(serde_json::Value::String("Main".into())) {
                return None;
            } // SCULPT_CORE21_ROUTE_JSON
            // Migrate only the unconverted Show import. A later operator choice
            // to return a track to Main must survive a save/reopen cycle.
            if !t.comments.contains("Prepared only:") {
                return None;
            }
            let start = t.comments.find("(SCULPT HW ")? + 1;
            let tail = &t.comments[start..];
            let end = tail.find(')')?;
            let path = &tail[..end];
            let name = available.iter().find(|n| n.eq_ignore_ascii_case(path))?.clone();
            Some((t.id.0, name, t.comments.clone()))
        })
        .collect();
    for (id, path, comments) in updates {
        if let Err(e) = app.run(
            "track.output",
            serde_json::json!({
                "tracks": [id], "output": path, "create_bus": false
            }),
        ) {
            log::warn!("Sculpt Show output route {id} -> {path}: {e}");
        } else {
            // After migration, stop treating an intentional future Main
            // assignment as a pending import route. Keep Show stem UUID/path.
            let migrated = comments.replace("Prepared only:", "Routing enabled:");
            if let Err(e) = app.run(
                "track.comments",
                serde_json::json!({
                    "track": id, "comments": migrated
                }),
            ) {
                log::warn!("Sculpt Show route annotation {id}: {e}");
            }
        }
    }
}

fn open_path(app: &mut SoundApp, p: &str) {
    let lower = p.to_ascii_lowercase();
    let r = if lower.ends_with(".scdaw") || lower.ends_with(".scraft") {
        // SCULPT_CORE16_OPEN_LEGACY
        dialogs::run_or_ask(app, "session.open", serde_json::json!({"path": p}))
    } else if lower.ends_with(".mid") || lower.ends_with(".midi") || lower.ends_with(".smf") {
        app.run("file.import_midi", serde_json::json!({"path": p}))
    } else {
        let at = app.engine.session().edit.selection.start;
        app.run("file.import_audio", serde_json::json!({"path": p, "at": at}))
    };
    if let Err(e) = r {
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
            app.ui.status =
                format!("Hardware routing needs {desired} channels; device exposes {actual}. Unavailable channels are monitored on Main.");
            log::warn!("Sculpt Bridge hardware output not fully available: requested {desired}, active {actual}");
        }
    }
}

/// SCULPT_CORE19_FINDER_DOCUMENTS: drain AppKit kAEOpenDocuments on the UI thread.
#[cfg(target_os = "macos")]
fn sculpt_drain_finder_documents(app: &mut SoundApp) {
    let spool = std::env::temp_dir().join(format!("sculpt-open-documents-{}.txt", std::process::id()));
    let Ok(data) = std::fs::read_to_string(&spool) else { return };
    let _ = std::fs::remove_file(&spool);
    for line in data.lines() {
        if !line.is_empty() {
            // The path is already a local filename, not a user-supplied shell command.
            open_path(app, line);
        }
    }
}

/// The per-user settings directory: `ui.json`, `Autosave/`, `Presets/` and `logs/` live here.
fn config_dir() -> Option<std::path::PathBuf> {
    config::config_dir()
}

fn prefs_path() -> Option<std::path::PathBuf> {
    config_dir().map(|b| b.join("ui.json"))
}

/// Runs without preferences (`SOUNDCRAFT_NO_PREFS`, agents' test runs) neither read nor write them.
fn prefs_enabled() -> bool {
    std::env::var_os("SOUNDCRAFT_NO_PREFS").is_none()
}

fn load_prefs() -> Option<UiState> {
    if !prefs_enabled() {
        return None;
    }
    let path = prefs_path()?;
    match config::load_ui(&path) {
        Ok(state) => state,
        Err(error) => {
            log::warn!("{error}");
            None
        }
    }
}

fn save_prefs(ui: &UiState) {
    if !prefs_enabled() {
        return;
    }
    if let Some(path) = prefs_path()
        && let Err(error) = config::save_ui(&path, ui)
    {
        log::warn!("{error}");
    }
}

fn services() -> Services {
    Services {
        pick_open: Some(Box::new(|purpose: &str, _exts: &[&str]| {
            let d = rfd::FileDialog::new();
            let d = match purpose {
                "session.open" => d.add_filter("Sculpt DAW Session", &["scdaw", "scraft"]), // SCULPT_CORE17_OPEN_PICKER
                "file.import_midi" => d.add_filter("MIDI", &["mid", "midi", "smf"]),
                _ => d.add_filter(
                    "Audio",
                    &["wav", "wave", "bwf", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "aac", "mp4", "caf", "mkv", "webm"],
                ),
            };
            d.pick_file().map(|p| p.to_string_lossy().into_owned())
        })),
        pick_open_many: Some(Box::new(|_purpose: &str, _exts: &[&str]| {
            rfd::FileDialog::new()
                .add_filter(
                    "Audio",
                    &["wav", "wave", "bwf", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "aac", "mp4", "caf", "mkv", "webm"],
                )
                .pick_files()
                .map(|files| files.into_iter().map(|p| p.to_string_lossy().into_owned()).collect())
        })),
        pick_save: Some(Box::new(|_purpose: &str, default: &str| {
            let path = std::path::Path::new(default);
            let mut d = rfd::FileDialog::new().add_filter("Sculpt DAW Session", &["scdaw"]); // SCULPT_CORE17_SAVE_PICKER
            if let Some(n) = path.file_name().and_then(|n| n.to_str()) {
                d = d.set_file_name(n);
            }
            d.save_file().map(|p| p.to_string_lossy().into_owned())
        })),
        pick_folder: Some(Box::new(|start: &str| {
            rfd::FileDialog::new().set_directory(start).pick_folder().map(|p| p.to_string_lossy().into_owned())
        })),
    }
}

/// The window, Dock, taskbar and Alt-Tab icon. macOS gets Apple's icon grid (transparent margin);
/// elsewhere the full tile. Regenerate with `packaging/icons.sh`.
fn app_icon() -> Option<egui::IconData> {
    #[cfg(target_os = "macos")]
    let png: &[u8] = include_bytes!("../../../assets/sculpt-daw/sculpt-daw-512.png");
    #[cfg(not(target_os = "macos"))]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/ai.storyteller.soundcraft.png");
    eframe::icon_data::from_png_bytes(png).map_err(|e| log::warn!("app icon: {e}")).ok()
}

fn main() -> eframe::Result {
    // First, so every start-up record (and the engine's panic hook, installed with the first
    // Engine) is captured; see `logging`.
    let logger = logging::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // SCULPT_CORE20_BRIDGE_CAPABILITIES: stable versioned DAW-side contract.
    if args.iter().any(|a| a == "--sculpt-bridge-capabilities") {
        println!(
            "{}",
            serde_json::json!({
                "protocol": "com.sculpt.bridge.daw", "version": 1,
                "application": "audio.sculpt.daw", "sessionExtensions": ["scdaw", "scraft"],
                "commands": ["session.open", "file.import_audio", "file.save", "transport.play", "transport.stop"],
                "cliRelativePath": "Contents/Helpers/soundcraft-cli",
                "hardwareRouting": {"enabled": true, "maxOutputChannels": 64,
                                    "pathFormats": ["mono", "stereo"],
                                    "bridgeMainPreview": true,
                                    "unavailableOutputFallback": "main"}, // SCULPT_CORE21_BRIDGE_HW_CAPS
                "control": {"transport": "json-lines", "enabledBy": "--control PORT", "default": "disabled"}
            })
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Sculpt DAW {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let demo = args.iter().any(|a| a == "--demo" || a == "--sample");
    let no_audio = args.iter().any(|a| a == "--no-audio");
    let control_port: Option<u16> = args
        .iter()
        .position(|a| a == "--control")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .or_else(|| std::env::var("SOUNDCRAFT_CONTROL_PORT").ok().and_then(|p| p.parse().ok()));
    let files: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !(a.starts_with("--") || (i > &0 && args.get(i - 1).is_some_and(|p| p == "--control"))))
        .map(|(_, a)| a.clone())
        .collect();
    log::info!("SoundCraft {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    // The log file lives in the settings directory; opened after the arguments, so `--version`
    // leaves no file behind. Records logged until now are written to it first. Runs without
    // preferences (agents' test runs) log to standard error only, so they don't rotate away the
    // user's own logs.
    if let Some(logger) = logger {
        match config_dir() {
            _ if !prefs_enabled() => logger.no_file(),
            Some(dir) => match logger.attach_dir(&dir.join(logging::LOG_DIR)) {
                Ok(path) => log::info!("log file {}", path.display()),
                Err(e) => log::warn!("no log file: {e}"),
            },
            None => {
                logger.no_file();
                log::warn!("no log file: no settings directory (profile override is empty or platform configuration is unavailable)");
            }
        }
    }

    let engine = if demo { soundcraft_engine::demo::demo_engine() } else { Engine::default() };
    let prefs = load_prefs().unwrap_or_default();
    let mut options = eframe::NativeOptions {
        // SCULPT_MAC_TOUCHBAR_EXIT_PILOT_V1: macOS 26 AppKit KVO crash on shutdown.
        // Sculpt already saves its UI/window preferences separately via save_prefs.
        #[cfg(target_os = "macos")]
        persist_window: false,
        #[cfg(target_os = "macos")]
        run_and_return: false,
        viewport: egui::ViewportBuilder::default()
            .with_title("Sculpt DAW")
            .with_inner_size(sane_window_size(prefs.window_size))
            .with_maximized(prefs.window_maximized)
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_app_id("audio.sculpt.daw"),
        ..Default::default()
    };
    if let Some(icon) = app_icon() {
        options.viewport = options.viewport.with_icon(icon);
    }
    // Before eframe creates the wgpu instance: default Windows to DirectX 12 with FXC (see graphics.rs).
    #[cfg(target_os = "windows")]
    graphics::configure(&mut options, eframe::wgpu::Backends::from_env(), eframe::wgpu::Dx12Compiler::from_env());
    #[cfg(target_os = "macos")]
    sculpt_open_documents::install(); // SCULPT_CORE20_FINDER_INSTALL
    eframe::run_native(
        "Sculpt DAW",
        options,
        Box::new(move |cc| {
            let player = if no_audio { None } else { Some(soundcraft_playback::Player::new(Arc::new(engine.session().clone()))) };
            #[cfg(target_os = "macos")]
            sculpt_open_documents::install(); // SCULPT_CORE20_FINDER_REINSTALL
            let mut app = SoundApp::new(engine, player, services());
            for (flag, cmd) in
                [("edit_view.io", "view.edit_io"), ("edit_view.inserts_ae", "view.edit_inserts_ae"), ("edit_view.inserts_fj", "view.edit_inserts_fj")]
            {
                if !app.engine.session().edit.flag(flag) {
                    let _ = app.run(cmd, serde_json::json!({}));
                }
            }
            app.autosave_dir = prefs_path().and_then(|p| p.parent().map(|d| d.join("Autosave")));
            app.preset_dir = prefs_path().and_then(|p| p.parent().map(|d| d.join("Presets")));
            app.extra.ui_scale = prefs.ui_scale;
            app.ui = prefs;
            // SCULPT_CORE_EIGHT_SLOTS_DEFAULT_V1: ensure F-H is available in Mix by default.
            // Existing A-E toggles and custom UI preferences remain intact.
            for (first, second) in [("inserts_ae", "inserts_fj"), ("sends_ae", "sends_fj")] {
                if app.ui.mix_views.iter().any(|k| k == first) && !app.ui.mix_views.iter().any(|k| k == second) {
                    app.ui.mix_views.push(second.to_string());
                }
            }
            if let Some(port) = control_port {
                let rx = control_server::start(port, cc.egui_ctx.clone());
                app = app.with_control(rx);
            }
            for f in &files {
                open_path(&mut app, f);
            }
            // If the system menu bar cannot be built, keep the egui menu bar rather than fail to start.
            #[cfg(target_os = "macos")]
            let menu = match native_menu::NativeMenu::new(&app, &cc.egui_ctx) {
                Ok(m) => {
                    app.native_menu_bar = true;
                    Some(m)
                }
                Err(e) => {
                    log::warn!("system menu bar unavailable, using the in-window menu bar: {e}");
                    None
                }
            };
            Ok(Box::new(App(
                app,
                logger,
                #[cfg(target_os = "macos")]
                menu,
            )))
        }),
    )
}

/// A saved window size, or the default when the prefs file holds nonsense (NaN, negative, tiny).
fn sane_window_size([w, h]: [f32; 2]) -> [f32; 2] {
    if w.is_finite() && h.is_finite() && w >= 960.0 && h >= 600.0 { [w.min(16_384.0), h.min(16_384.0)] } else { [1600.0, 1000.0] }
}

#[cfg(test)]
mod tests {
    use super::set_title;

    #[test]
    fn the_title_is_sent_only_when_it_changes() {
        let ctx = egui::Context::default();
        let titles = |title: &str| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| set_title(ui.ctx(), title.to_string()));
            out.textures_delta.clear();
            out.viewport_output.values().flat_map(|v| &v.commands).filter(|c| matches!(c, egui::ViewportCommand::Title(_))).count()
        };
        assert_eq!(titles("Song — Sculpt DAW"), 1);
        assert_eq!(titles("Song — Sculpt DAW"), 0);
        assert_eq!(titles("Song * — Sculpt DAW"), 1);
    }
}
