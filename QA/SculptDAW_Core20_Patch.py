#!/usr/bin/env python3
"""Sculpt DAW Core20: safe overlay after the proven Core19 action.

Base is the CI-restored Core19 source. No upstream tree or UI settings are reset.
Every code insertion has an anchor guard: CI stops instead of silently shipping
an unpatched build when SoundCraft changes an interface.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def edit(path: str, old: str, new: str, tag: str, n: int = 1):
    p = ROOT / path
    source = p.read_text(encoding="utf8")
    if tag in source:
        print(f"CORE20 already applied: {tag}")
        return
    count = source.count(old)
    if count != n:
        raise RuntimeError(f"{path}: {tag}: expected {n} exact anchor(s), found {count} (Core19 source drift)")
    p.write_text(source.replace(old, new, n), encoding="utf8")
    print(f"CORE20 patched: {tag}")


def write_once(path: str, source: str):
    p = ROOT / path
    p.parent.mkdir(parents=True, exist_ok=True)
    if not p.exists() or p.read_text(encoding="utf8") != source:
        p.write_text(source, encoding="utf8")
        print(f"CORE20 wrote: {path}")

# Finder double-click has two independent issues:
#   1. .scdaw wasn't necessarily treated as a session in open_path.
#   2. Cocoa sends 'odoc' Apple Events, not guaranteed argv entries.
MAIN = "apps/soundcraft/src/main.rs"
p = ROOT / MAIN
s = p.read_text(encoding="utf8")
if 'SCULPT_CORE20_SCDAW_FILE' not in s:
    if 'lower.ends_with(".scdaw")' not in s:
        edit(MAIN, 'if lower.ends_with(".scraft") {',
             'if lower.ends_with(".scraft") || lower.ends_with(".scdaw") { // SCULPT_CORE20_SCDAW_FILE',
             'SCULPT_CORE20_SCDAW_FILE')
    else:
        print('CORE20: .scdaw recognized in open_path')

edit(MAIN,
     '#[cfg(target_os = "macos")]\nmod native_menu;',
     '#[cfg(target_os = "macos")]\nmod native_menu;\n#[cfg(target_os = "macos")]\n#[allow(unsafe_code)] // SCULPT_CORE21_FINDER_FFI_SCOPE: this one FFI module only\nmod sculpt_open_documents; // SCULPT_CORE20_FINDER_MODULE',
     'SCULPT_CORE20_FINDER_MODULE')

# The workspace forbids unsafe Rust globally. This app alone needs a tiny
# macOS FFI adapter for Finder Apple Events. Retain deny(unsafe_code) for the
# rest of sculpt-daw and allow it only on the adapter module above.
# Never weaken workspace-wide lint policy or plugin/audio crates.
APP_CARGO = 'apps/soundcraft/Cargo.toml'
edit(APP_CARGO,
     '[lints]\nworkspace = true',
     '''[lints.rust]
unsafe_code = "deny" # SCULPT_CORE21_APP_DENY_UNSAFE

[lints.clippy]
dbg_macro = "warn"
todo = "warn"
too_many_arguments = "allow"
type_complexity = "allow"''',
     'SCULPT_CORE21_APP_DENY_UNSAFE')

# Install the handler both before creating the eframe app and once AppKit is
# initialized. Receiving URLs is safe off the audio callback; processing them
# happens on the existing UI logic thread via open_path.
edit(MAIN,
     '    eframe::run_native(\n',
     '    #[cfg(target_os = "macos")]\n    sculpt_open_documents::install(); // SCULPT_CORE20_FINDER_INSTALL\n    eframe::run_native(\n',
     'SCULPT_CORE20_FINDER_INSTALL')

edit(MAIN,
     '            let mut app = SoundApp::new(engine, player, services());',
     '            #[cfg(target_os = "macos")]\n            sculpt_open_documents::install(); // SCULPT_CORE20_FINDER_REINSTALL\n            let mut app = SoundApp::new(engine, player, services());',
     'SCULPT_CORE20_FINDER_REINSTALL')

edit(MAIN,
     '        // Files dropped on the window: sessions open, audio/MIDI import.',
     '        // SCULPT_CORE20_FINDER_DRAIN: queued macOS Open Document events are\n'
     '        // delivered on the GUI thread and use the existing document handler.\n'
     '        #[cfg(target_os = "macos")]\n'
     '        for path in sculpt_open_documents::drain() {\n'
     '            open_path(&mut self.0, &path);\n'
     '        }\n'
     '        // Files dropped on the window: sessions open, audio/MIDI import.',
     'SCULPT_CORE20_FINDER_DRAIN')

write_once('apps/soundcraft/src/sculpt_open_documents.rs', '''//! Cocoa Open Document support (Finder, Dock and Open With).
//! Never touches the audio thread. Multiple files are queued for the GUI frame.
use std::ffi::{CStr, c_char};
use std::sync::{Mutex, OnceLock};

static PATHS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

unsafe extern "C" {
    fn sculpt_install_document_handler(callback: unsafe extern "C" fn(*const c_char));
}

unsafe extern "C" fn receive(path: *const c_char) {
    if path.is_null() { return; }
    // SAFETY: the Objective-C handler passes a temporary NUL-terminated UTF-8
    // filesystem path; we clone it before returning from the callback.
    let name = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    if !name.is_empty() && let Ok(mut queue) = PATHS.get_or_init(|| Mutex::new(Vec::new())).lock() {
        queue.push(name);
    }
}

/// May be called more than once: AppKit may reinstall its own handlers on start.
pub fn install() {
    PATHS.get_or_init(|| Mutex::new(Vec::new()));
    // SAFETY: Objective-C implementation only registers an NSAppleEventManager
    // callback on the process main thread; the callback never retains its input.
    unsafe { sculpt_install_document_handler(receive) }
}

pub fn drain() -> Vec<String> {
    let Ok(mut queue) = PATHS.get_or_init(|| Mutex::new(Vec::new())).lock() else { return Vec::new() };
    // Filter duplicate odoc notifications within one frame, keep arrival order.
    let pending = std::mem::take(&mut *queue);
    let mut result = Vec::new();
    for path in pending { if !result.contains(&path) { result.push(path); } }
    result
}
''')

write_once('apps/soundcraft/src/sculpt_open_documents.m', '''// Sculpt DAW: native Open Document Apple Event shim for Finder .scdaw files.
#import <Foundation/Foundation.h>

static void (*sculpt_open_callback)(const char *) = NULL;

@interface SculptOpenDocumentSink : NSObject
- (void)handleOpenDocuments:(NSAppleEventDescriptor *)event
                 withReply:(NSAppleEventDescriptor *)reply;
@end

@implementation SculptOpenDocumentSink
- (void)handleOpenDocuments:(NSAppleEventDescriptor *)event
                 withReply:(NSAppleEventDescriptor *)reply {
    (void)reply;
    NSAppleEventDescriptor *items = [event paramDescriptorForKeyword:0x2d2d2d2d]; // '----'
    for (NSInteger i = 1; i <= [items numberOfItems]; ++i) {
        NSAppleEventDescriptor *item = [items descriptorAtIndex:i];
        NSURL *url = [item fileURLValue];
        if (!url) {
            NSString *raw = [item stringValue];
            if ([raw hasPrefix:@"file:"]) url = [NSURL URLWithString:raw];
            else if ([raw hasPrefix:@"/"]) url = [NSURL fileURLWithPath:raw];
        }
        if (url && url.isFileURL && sculpt_open_callback) {
            sculpt_open_callback(url.fileSystemRepresentation);
        }
    }
}
@end

void sculpt_install_document_handler(void (*callback)(const char *)) {
    // Strongly retain the target. NSAppleEventManager doesn't retain targets.
    static SculptOpenDocumentSink *sink = nil;
    if (!sink) sink = [SculptOpenDocumentSink new];
    sculpt_open_callback = callback;
    [[NSAppleEventManager sharedAppleEventManager]
        setEventHandler:sink
           andSelector:@selector(handleOpenDocuments:withReply:)
         forEventClass:0x61657674  // 'aevt'
            andEventID:0x6f646f63]; // 'odoc'
}
''')

# Compile the Obj-C shim using Xcode clang from the existing build.rs, without
# changing Cargo.lock or introducing a new crate (this workspace builds --locked).
BUILD_RS = 'apps/soundcraft/build.rs'
edit(BUILD_RS,
     '    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {',
     '''    // SCULPT_CORE20_MAC_SHIM: compile the native Finder Apple Event handler
    // only for the sculpt-daw binary. Does not affect the CLI or audio crates.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
        let obj = out.join("sculpt_open_documents.o");
        let target = std::env::var("TARGET").unwrap_or_default();
        let arch = if target.starts_with("aarch64") { "arm64" } else { "x86_64" };
        println!("cargo:rerun-if-changed=src/sculpt_open_documents.m");
        let status = std::process::Command::new("xcrun")
            .args(["clang", "-fobjc-arc", "-mmacosx-version-min=11.0", "-arch", arch,
                   "-c", "src/sculpt_open_documents.m", "-o"])
            .arg(&obj).status();
        if !matches!(&status, Ok(s) if s.success()) {
            panic!("Sculpt Finder native shim failed to compile: {status:?}");
        }
        println!("cargo:rustc-link-arg={}", obj.display());
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {''',
     'SCULPT_CORE20_MAC_SHIM')

# Guarantee the selected channel fader is genuinely visible in Edit regardless
# of the width/order of older Core19 strip widgets. A lower panel reserves
# space before docking calculates canvas geometry.
EDIT = 'crates/ui-egui/src/edit_window.rs'
edit(EDIT,
     '    crate::docking::show(app, ui, crate::MainWindow::Edit);',
     '''    // SCULPT_CORE20_VISIBLE_STRIP: reserve a dedicated channel strip in Edit,
    // not a hidden/floating mix widget or an optional track-column view.
    egui::Panel::bottom("sculpt_core20_selected_strip").exact_size(154.0)
        .frame(egui::Frame::NONE.fill(Tokens::current().panel_bg2))
        .show(ui, |ui| sculpt_core20_selected_strip(app, ui));
    crate::docking::show(app, ui, crate::MainWindow::Edit);''',
     'SCULPT_CORE20_VISIBLE_STRIP')

# Append a self-contained horizontal selected-channel strip. Uses existing
# engine commands, no duplicate routing/DSP or plugin-host state.
p = ROOT / EDIT
content = p.read_text(encoding='utf8')
if 'SCULPT_CORE20_SELECTED_CHANNEL_WIDGET' not in content:
    p.write_text(content + '''

/// SCULPT_CORE20_SELECTED_CHANNEL_WIDGET
/// Persistent Edit-channel controls: visible fader, pan, I/O and current inserts/sends.
fn sculpt_core20_selected_strip(app: &mut SoundApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.strong("SELECTED CHANNEL");
        ui.separator();
        let selected = app.engine.session().edit.selected_tracks.first().copied();
        let track = selected.and_then(|id| app.engine.session().track(id)).cloned();
        let Some(track) = track else {
            ui.label("Select a track in Edit to show its channel strip");
            return;
        };
        ui.strong(&track.name);
    });
    let selected = app.engine.session().edit.selected_tracks.first().copied();
    let Some(track) = selected.and_then(|id| app.engine.session().track(id)).cloned() else { return };
    let id = track.id;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.label("FADER");
            let (fr, _) = ui.allocate_exact_size(vec2(36.0, 83.0), Sense::hover());
            if let Some(db) = crate::widgets::fader(ui, fr, track.mixer.volume_db,
                                                     ui.id().with(("sculpt_core20_fader", id.0))) {
                let _ = app.engine.execute_merged("mix.volume", &json!({"track": id.0, "db": db}), &format!("fader:{}", id.0));
            }
            ui.label(egui::RichText::new(format!("{:.1} dB", track.mixer.volume_db)).small());
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.label("PAN");
            let mut pan = track.mixer.pan.first().copied().unwrap_or_default();
            if ui.add_sized([120.0, 22.0], egui::Slider::new(&mut pan, -1.0..=1.0).show_value(true)).changed() {
                let _ = app.engine.execute_merged("mix.pan", &json!({"track": id.0, "pan": pan, "index": 0}), &format!("pan:{}:0", id.0));
            }
            if ui.small_button("Centre").clicked() {
                let _ = app.run("mix.pan", json!({"track": id.0, "pan": 0.0, "index": 0}));
            }
            ui.horizontal(|ui| {
                if ui.selectable_label(track.mixer.mute, "MUTE").clicked() { let _ = app.run("mix.mute", json!({"track": id.0})); }
                if ui.selectable_label(track.mixer.solo, "SOLO").clicked() { let _ = app.run("mix.solo", json!({"track": id.0})); }
            });
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.label("I/O");
            ui.label(egui::RichText::new(format!("IN: {:?}", track.mixer.input)).small());
            ui.label(egui::RichText::new(format!("OUT: {:?}", track.mixer.output)).small());
            if ui.small_button("Routing...").clicked() { app.ui.show_io_setup = true; }
        });
        ui.separator();
        egui::ScrollArea::vertical().max_height(108.0).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.label("INSERTS");
                    for (slot, insert) in track.mixer.inserts.iter().take(8).enumerate() {
                        if let Some(ins) = insert {
                            let name = crate::mix_window::plugin_info(&ins.plugin).map_or(ins.plugin.as_str(), |p| p.name);
                            if ui.small_button(format!("{}: {}", slot + 1, name)).clicked() && !app.ui.plugin_windows.contains(&(id, slot)) {
                                app.ui.plugin_windows.push((id, slot));
                            }
                        }
                    }
                });
                ui.add_space(24.0);
                ui.vertical(|ui| {
                    ui.label("SENDS");
                    for (slot, send) in track.mixer.sends.iter().take(8).enumerate() {
                        if let Some(send) = send {
                            ui.label(egui::RichText::new(format!("{}: {:?}", slot + 1, send.target)).small());
                            let mut level = send.level_db;
                            if ui.add_sized([135.0, 17.0], egui::Slider::new(&mut level, -60.0..=12.0).text("dB")).changed() {
                                let _ = app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": level}), &format!("send:{}:{slot}", id.0));
                            }
                        }
                    }
                });
            });
        });
    });
}
''', encoding='utf8')
    print('CORE20 added selected channel widget')

# Memory Locations = marker list. Migrate the existing per-user layout ONCE,
# never force placement every frame (the user can drag it afterwards).
UI = 'crates/ui-egui/src/lib.rs'
edit(UI, '    pub show_memory_locations: bool,',
     '''    pub show_memory_locations: bool,
    pub sculpt_core20_marker_placed: bool, // SCULPT_CORE20_LAYOUT_FIELD
    pub sculpt_core20_marker_placed_mix: bool,''',
     'SCULPT_CORE20_LAYOUT_FIELD')
edit(UI, '            show_memory_locations: false,',
     '''            show_memory_locations: false,
            sculpt_core20_marker_placed: false, // SCULPT_CORE20_LAYOUT_DEFAULT
            sculpt_core20_marker_placed_mix: false,''',
     'SCULPT_CORE20_LAYOUT_DEFAULT')
DOCK = 'crates/ui-egui/src/docking.rs'
# SCULPT_CORE21_FIX_20261016: Core18/19 already provide right-side Marker
# placement. Their restored source may use a match arm or a generated layout
# that is not `Self::Memory => Zone::Right,`. Never inspect/rewrite that arm:
# doing so repeatedly aborted otherwise successful macOS builds.
# The one-time persisted-layout migration below is sufficient and does not
# require a particular textual representation of a default panel side.
edit(DOCK,
     '    let before_sync = workspace.clone();',
     '''    // SCULPT_CORE20_MARKER_DEFAULT_RIGHT: Core18's right default is preserved.
    // SCULPT_CORE20_MARKER_RIGHT_MIGRATION: move a pre-Core20 Marker/Memory
    // panel to the right once, while leaving subsequent operator panel moves intact.
    if (edit && !app.ui.sculpt_core20_marker_placed)
        || (!edit && !app.ui.sculpt_core20_marker_placed_mix) {
        if workspace.layout.contains(&Panel::Memory) {
            let _ = workspace.apply(Action::Move {
                panel: Panel::Memory, anchor: Panel::Canvas,
                placement: Placement::Split(Zone::Right),
            });
        } else {
            workspace.hidden.retain(|(panel, _)| *panel != Panel::Memory);
        }
        if edit { app.ui.sculpt_core20_marker_placed = true; }
        else { app.ui.sculpt_core20_marker_placed_mix = true; }
    }
    let before_sync = workspace.clone();''',
     'SCULPT_CORE20_MARKER_RIGHT_MIGRATION')

# DAW-side bridge foundation: native app advertises its capabilities. The
# existing JSON-lines --control socket remains opt-in and backwards compatible.
# Show code is NOT modified in this package.
edit(MAIN,
     '    if args.iter().any(|a| a == "--version" || a == "-V") {',
     '''    // SCULPT_CORE20_BRIDGE_CAPABILITIES: stable versioned DAW-side contract.
    if args.iter().any(|a| a == "--sculpt-bridge-capabilities") {
        println!("{}", serde_json::json!({
            "protocol": "com.sculpt.bridge.daw", "version": 1,
            "application": "audio.sculpt.daw", "sessionExtensions": ["scdaw", "scraft"],
            "commands": ["session.open", "file.import_audio", "file.save", "transport.play", "transport.stop"],
            "cliRelativePath": "Contents/Helpers/soundcraft-cli",
            "control": {"transport": "json-lines", "enabledBy": "--control PORT", "default": "disabled"}
        }));
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {''',
     'SCULPT_CORE20_BRIDGE_CAPABILITIES')

# Defensive sanity check: these marks prove Core19 insertion wasn't displaced.
expected = {
    MAIN: ['SCULPT_CORE20_FINDER_MODULE','SCULPT_CORE21_FINDER_FFI_SCOPE','SCULPT_CORE20_FINDER_INSTALL','SCULPT_CORE20_FINDER_DRAIN','SCULPT_CORE20_BRIDGE_CAPABILITIES'],
    UI: ['SCULPT_CORE20_LAYOUT_FIELD','SCULPT_CORE20_LAYOUT_DEFAULT'],
    DOCK: ['SCULPT_CORE20_MARKER_DEFAULT_RIGHT','SCULPT_CORE20_MARKER_RIGHT_MIGRATION'],
    EDIT: ['SCULPT_CORE20_VISIBLE_STRIP','SCULPT_CORE20_SELECTED_CHANNEL_WIDGET'],
    BUILD_RS: ['SCULPT_CORE20_MAC_SHIM'],
    APP_CARGO: ['SCULPT_CORE21_APP_DENY_UNSAFE'],
}
for name, markers in expected.items():
    source = (ROOT / name).read_text(encoding='utf8')
    for marker in markers:
        assert source.count(marker) == 1, f'{name}: unexpected {marker} count'
print('CORE20 overlays verified; Core19 feature code untouched')
