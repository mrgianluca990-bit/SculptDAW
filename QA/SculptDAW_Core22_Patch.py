#!/usr/bin/env python3
"""Sculpt DAW Core22 source overlay; run AFTER Core20 and Core21, on CI-restored tree.
Fail on source drift instead of shipping a partially applied release.
"""
from pathlib import Path
import re

ROOT=Path(__file__).resolve().parents[1]

def transform(path, old, new, tag):
    f=ROOT/path
    s=f.read_text(encoding='utf-8')
    if tag in s:
        print('ALREADY',tag);return
    n=s.count(old)
    if n != 1:
        raise RuntimeError(f'{path}: {tag} expected one source anchor; found {n}')
    f.write_text(s.replace(old,new,1),encoding='utf-8')
    print('PATCH',tag)

def rewrite_function(path, sig, replacement, tag):
    p=ROOT/path; s=p.read_text(encoding='utf-8')
    if tag in s:
        print('ALREADY',tag);return
    start=s.find(sig)
    if start<0 or s.find(sig,start+len(sig))>=0:raise RuntimeError(f'{path} missing/duplicate {sig}')
    brace=s.find('{',start); depth=0; i=brace
    # Function bodies edited here contain no raw strings with unmatched braces.
    while i<len(s):
        if s[i]=='{': depth+=1
        elif s[i]=='}':
            depth-=1
            if depth==0:
                p.write_text(s[:start]+replacement+s[i+1:],encoding='utf-8');print('PATCH',tag);return
        i+=1
    raise RuntimeError(f'unclosed function {sig}')

SETTINGS='crates/playback/src/sculpt_audio_settings.rs'
settings_source=r'''//! SCULPT_CORE22_SHARED_AUDIO: a simple atomic shared audio profile for Show and DAW.
//! Both apps read ~/Library/Application Support/Sculpt/AudioSettings.conf.
//! Hardware UID is authoritative for Show; DAW matches the physical output's
//! unique display name through CPAL 0.16 (which does not expose a UID API).
//! If names are duplicated, DAW refuses the switch instead of guessing.
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioSettings {
    pub output_uid: String,
    pub output_name: String,
    pub input_name: String,
    pub sample_rate: u32,
    pub buffer_frames: u32,
}
impl AudioSettings {
    pub fn file() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from).map(|h|
            h.join("Library/Application Support/Sculpt/AudioSettings.conf"))
    }
    pub fn read() -> Self {
        let Some(file) = Self::file() else { return Self::default() };
        let Ok(s) = std::fs::read_to_string(file) else { return Self::default() };
        let mut data = Self::default();
        if s.lines().next() != Some("SCULPT_AUDIO_V1") { return data }
        for line in s.lines().skip(1) {
            let Some((key,value))=line.split_once('=') else {continue};
            match key {
                "uid" => data.output_uid=value.to_string(),
                "name" => data.output_name=value.to_string(),
                "input" => data.input_name=value.to_string(),
                "rate" => data.sample_rate=value.parse().unwrap_or(0),
                "buffer" => data.buffer_frames=value.parse().unwrap_or(0),
                _ => (),
            }
        }
        data
    }
    pub fn save(&self) -> Result<(), String> {
        let path=Self::file().ok_or("HOME not set")?;
        let dir=path.parent().ok_or("missing config parent")?;
        std::fs::create_dir_all(dir).map_err(|e|e.to_string())?;
        let clean=|s: &str|s.replace('\r'," " ).replace('\n'," " );
        let body=format!("SCULPT_AUDIO_V1\nuid={}\nname={}\ninput={}\nrate={}\nbuffer={}\n",
              clean(&self.output_uid),clean(&self.output_name),clean(&self.input_name),
              self.sample_rate,self.buffer_frames);
        let tmp=path.with_extension("conf.tmp");
        std::fs::write(&tmp,body).map_err(|e|e.to_string())?;
        std::fs::rename(&tmp,path).map_err(|e|e.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct AudioDeviceInfo { pub name: String, pub channels: u16 }

pub fn available_output_devices() -> Vec<AudioDeviceInfo> {
    use cpal::traits::{DeviceTrait,HostTrait};
    let Ok(devices)=cpal::default_host().output_devices() else {return Vec::new()};
    devices.filter_map(|d|{
        #[allow(deprecated)]
        let name=d.name().ok()?;
        let channels=d.supported_output_configs().ok()?
            .filter(|c|c.sample_format()==cpal::SampleFormat::F32)
            .map(|c|c.channels()).max().unwrap_or(0);
        (channels>0).then_some(AudioDeviceInfo{name,channels})
    }).collect()
}

#[cfg(test)]
mod sculpt_core22_audio_tests {
    use super::*;
    #[test]
    fn defaults_do_not_force_a_device() {
        let p=AudioSettings::default();
        assert!(p.output_uid.is_empty());
        assert_eq!(p.sample_rate,0);
    }
}
'''
p=ROOT/SETTINGS;p.parent.mkdir(parents=True,exist_ok=True)
p.write_text(settings_source,encoding='utf-8')
transform('crates/playback/src/lib.rs','pub mod record;','pub mod record;\n#[cfg(not(target_arch = "wasm32"))]\npub mod sculpt_audio_settings; // SCULPT_CORE22_AUDIO_MODULE','SCULPT_CORE22_AUDIO_MODULE')

# Explicit selection from a shared profile. No silent device fallback on an explicit selection.
transform('crates/playback/src/lib.rs',
'''    let device = host.default_output_device().ok_or_else(|| "no default output device".to_string())?;''',
'''    // SCULPT_CORE22_EXPLICIT_DEVICE: select the same named CoreAudio device as Show.
    // CPAL 0.16 has no portable device UID; reject ambiguous duplicate names.
    let settings=crate::sculpt_audio_settings::AudioSettings::read();
    let device = if settings.output_name.trim().is_empty() {
        host.default_output_device().ok_or_else(|| "no default output device".to_string())?
    } else {
        #[allow(deprecated)]
        let mut matches=host.output_devices().map_err(|e|e.to_string())?
          .filter(|d| d.name().is_ok_and(|name| name==settings.output_name));
        let match_one=matches.next().ok_or_else(||format!("Selected CoreAudio interface disconnected: {}",settings.output_name))?;
        if matches.next().is_some(){return Err(format!("Ambiguous interface name: {} (use a uniquely named CoreAudio device)",settings.output_name));}
        match_one
    };''', 'SCULPT_CORE22_EXPLICIT_DEVICE')
transform('crates/playback/src/lib.rs',
'''    let want_ch = want_ch.max(2);
    let mut chosen''',
'''    let want_ch = want_ch.max(2);
    let want_rate = if settings.sample_rate>0 {settings.sample_rate} else {want_rate};
    // SCULPT_CORE22_REQUEST_RATE
    let mut chosen''','SCULPT_CORE22_REQUEST_RATE')
transform('crates/playback/src/lib.rs',
'''    let config = match chosen {
        Some(c) => c,''',
'''    if settings.sample_rate>0 && chosen.is_none() {
        return Err(format!("{} Hz unsupported by selected interface {}",want_rate,settings.output_name));
    }
    let mut config = match chosen { // SCULPT_CORE22_BUFFER_CONFIG
        Some(c) => c,''','SCULPT_CORE22_BUFFER_CONFIG')
transform('crates/playback/src/lib.rs',
'''    Ok((device, config, name))
}''',
'''    if settings.buffer_frames>0 {
        if ![64,128,256,512,1024,2048].contains(&settings.buffer_frames) {
            return Err("Unsupported requested audio buffer size".into());
        }
        config.buffer_size=cpal::BufferSize::Fixed(settings.buffer_frames);
    }
    Ok((device, config, name)) // SCULPT_CORE22_BUFFER_APPLIED
}''','SCULPT_CORE22_BUFFER_APPLIED')

# Record input follows the configured interface name rather than switching silently.
transform('crates/playback/src/record.rs',
'''        let device = host.default_input_device().ok_or("no audio input device")?;''',
'''        // SCULPT_CORE22_INPUT_DEVICE: explicit input choice from shared audio settings.
        let configured=crate::sculpt_audio_settings::AudioSettings::read();
        let device=if configured.input_name.trim().is_empty() {
            host.default_input_device().ok_or("no audio input device")?
        } else {
            #[allow(deprecated)]
            let mut matches=host.input_devices().map_err(|e|e.to_string())?
                .filter(|d|d.name().is_ok_and(|name|name==configured.input_name));
            let selected=matches.next().ok_or_else(||format!("Input interface disconnected: {}",configured.input_name))?;
            if matches.next().is_some(){return Err("Ambiguous input interface name".into())}
            selected
        };''','SCULPT_CORE22_INPUT_DEVICE')

# Cold Finder launch: re-register odoc when AppKit finishes launching.
# This covers the first cold-start document, not only subsequent Open With events.
objc='apps/soundcraft/src/sculpt_open_documents.m'
transform(objc,
    '#import <Foundation/Foundation.h>',
    '#import <Foundation/Foundation.h>\n#import <AppKit/AppKit.h> // SCULPT_CORE22_FINDER_APPKIT',
    'SCULPT_CORE22_FINDER_APPKIT')
transform(objc,
    '@interface SculptOpenDocumentSink : NSObject\n- (void)handleOpenDocuments:(NSAppleEventDescriptor *)event',
    '@interface SculptOpenDocumentSink : NSObject\n- (void)sculptInstallFinderHandler:(NSNotification *)event; // SCULPT_CORE22_FINDER_COLD_BOOT\n- (void)handleOpenDocuments:(NSAppleEventDescriptor *)event',
    'SCULPT_CORE22_FINDER_COLD_BOOT')
transform(objc,
    '@implementation SculptOpenDocumentSink\n',
    """@implementation SculptOpenDocumentSink
// SCULPT_CORE22_FINDER_LAUNCH_OBSERVER: AppKit can reset odoc at startup.
- (void)sculptInstallFinderHandler:(NSNotification *)notification {
    (void)notification;
    [[NSAppleEventManager sharedAppleEventManager]
        setEventHandler:self andSelector:@selector(handleOpenDocuments:withReply:)
          forEventClass:0x61657674 andEventID:0x6f646f63];
}
""",
    'SCULPT_CORE22_FINDER_LAUNCH_OBSERVER')
transform(objc,
    '    if (!sink) sink = [SculptOpenDocumentSink new];',
    """    if (!sink) {
        sink = [SculptOpenDocumentSink new];
        // SCULPT_CORE22_COLD_REGISTRATION: install before Finder's odoc event.
        [[NSNotificationCenter defaultCenter] addObserver:sink
            selector:@selector(sculptInstallFinderHandler:)
            name:NSApplicationWillFinishLaunchingNotification object:nil];
        [[NSNotificationCenter defaultCenter] addObserver:sink
            selector:@selector(sculptInstallFinderHandler:)
            name:NSApplicationDidFinishLaunchingNotification object:nil];
    }""",
    'SCULPT_CORE22_COLD_REGISTRATION')

# Selected channel in EDIT uses the ACTUAL mixer channel strip implementation.
mix='crates/ui-egui/src/mix_window.rs'
mix_path=ROOT/mix
src=mix_path.read_text(encoding='utf-8')
if 'SCULPT_CORE22_SHARED_STRIP' not in src:
    src += '''
// SCULPT_CORE22_SHARED_STRIP: the Edit inspector renders this same mixer strip.
// Selection is shared in session.edit.selected_tracks across both views.
pub(crate) fn sculpt_core22_selected_strip(app: &mut SoundApp, ui: &mut Ui) {
    let id=app.engine.session().edit.selected_tracks.first().copied();
    let Some(id)=id else {ui.label("Select a track to display its channel strip");return};
    let old_views=std::mem::replace(&mut app.ui.mix_views,vec!["all".to_string()]);
    let old_narrow=std::mem::replace(&mut app.ui.narrow_mix,false);
    let snap=app.player.as_ref().filter(|_|app.meters_live()).map(|p|p.meters());
    egui::ScrollArea::vertical().id_salt("sculpt_edit_strip_scroll")
        .auto_shrink([false,false]).show(ui, |ui| {
            strip(app,ui,id,snap.as_ref());
        });
    app.ui.narrow_mix=old_narrow;
    app.ui.mix_views=old_views;
}
'''
    mix_path.write_text(src,encoding='utf-8')
# Select a track when clicking strip background (not only the narrow nameplate).
transform(mix,
'''    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());''',
'''    let (r, strip_response) = ui.allocate_exact_size(vec2(w, h), Sense::click()); // SCULPT_CORE22_CLICK_STRIP''','SCULPT_CORE22_CLICK_STRIP')
transform(mix,
'''    if nresp.clicked() {
        crate::edit_window::click_track(app, id, ui.input(|i| i.modifiers), |_, t| !t.hidden && t.kind != TrackKind::Video);
    }''',
'''    if nresp.clicked() || strip_response.clicked() { // SCULPT_CORE22_SELECTION_SYNC
        crate::edit_window::click_track(app, id, ui.input(|i| i.modifiers), |_, t| !t.hidden && t.kind != TrackKind::Video);
    }''','SCULPT_CORE22_SELECTION_SYNC')

# Remove Core20's bottom horizontal strip, and host the exact vertical Mix strip at the left.
edit='crates/ui-egui/src/edit_window.rs';s=(ROOT/edit).read_text(encoding='utf-8')
pattern=r'(?s)    // SCULPT_CORE20_VISIBLE_STRIP: reserve a dedicated channel strip in Edit,.*?    crate::docking::show\(app, ui, crate::MainWindow::Edit\);'
new='''    // SCULPT_CORE22_EDIT_LEFT_STRIP: channel strip identical to the mixer, never bottom docked.
    egui::SidePanel::left("sculpt_core22_channel_strip").resizable(true).default_width(118.0)
        .min_width(108.0).max_width(178.0)
        .frame(egui::Frame::NONE.fill(t.panel_bg2))
        .show_inside(ui,|ui|crate::mix_window::sculpt_core22_selected_strip(app,ui));
    crate::docking::show(app, ui, crate::MainWindow::Edit);'''
if s.count('SCULPT_CORE22_EDIT_LEFT_STRIP')==0:
    s,n=re.subn(pattern,new,s,count=1)
    if n!=1:raise RuntimeError('edit_window: missing Core20 bottom-strip anchor')
    (ROOT/edit).write_text(s,encoding='utf-8')

# Right pane: Memory Locations above Groups; remove Tracks side pane.
dock='crates/ui-egui/src/docking.rs'
# Core18-21 may already have moved Marker to the right. Never require the old
# Zone::Bottom form: that caused several historical compile failures.
p=ROOT/dock; existing=p.read_text(encoding='utf-8')
# The proven Core18/Core20 source may represent Memory's default side via a
# match arm, a persisted-layout migration, or a generated layout.
# Never inspect it textually again. Insert only the new Groups default side
# at History's stable arm, and retain the proven Memory behavior unchanged.
if 'SCULPT_CORE22_MEMORY_RIGHT' not in existing:
    anchor = '            Self::History => Zone::Right,'
    if existing.count(anchor) != 1:
        raise RuntimeError(f'{dock}: Groups default side anchor missing: {existing.count(anchor)}')
    existing = existing.replace(anchor,
        '            Self::Groups => Zone::Right, // SCULPT_CORE22_MEMORY_RIGHT\n' + anchor, 1)
    p.write_text(existing, encoding='utf-8')
for old,new,tag in [
 ('    Memory,\n    History,','    Memory,\n    Groups, // SCULPT_CORE22_GROUP_PANEL\n    History,','SCULPT_CORE22_GROUP_PANEL'),
 ('&[Self::Canvas, Self::Tracks, Self::Clips, Self::Midi, Self::Memory, Self::History];',
  '&[Self::Canvas, Self::Tracks, Self::Clips, Self::Midi, Self::Memory, Self::Groups, Self::History]; // SCULPT_CORE22_GROUP_ALL','SCULPT_CORE22_GROUP_ALL'),
 ('            Self::Memory => "memory",','            Self::Memory => "memory",\n            Self::Groups => "groups", // SCULPT_CORE22_GROUP_ID','SCULPT_CORE22_GROUP_ID'),
 ('            Self::Memory => "Memory Locations",','            Self::Memory => "Memory Locations",\n            Self::Groups => "Groups", // SCULPT_CORE22_GROUP_LABEL','SCULPT_CORE22_GROUP_LABEL'),
 ('            Panel::Memory => crate::panels::memory_locations(app, ui),','            Panel::Memory => crate::panels::memory_locations(app, ui),\n            Panel::Groups => crate::panels::groups_only(app, ui), // SCULPT_CORE22_GROUP_RENDER','SCULPT_CORE22_GROUP_RENDER'),
 ('        Panel::Memory => (egui::vec2(260.0, 160.0), 1_000_000.0),','        Panel::Memory => (egui::vec2(220.0, 130.0), 470.0),\n        Panel::Groups => (egui::vec2(220.0, 130.0), 470.0), // SCULPT_CORE22_GROUP_LIMIT','SCULPT_CORE22_GROUP_LIMIT')]:
    transform(dock,old,new,tag)
# Default Edit arrangement: center document, upper-right Memory, lower-right Groups.
# Do not compare with an exact Core19 source block: historical patches have
# legitimately changed the default layout several times.
p=ROOT/dock;s=p.read_text(encoding='utf-8')
if 'SCULPT_CORE22_EDIT_DOCK' not in s:
    owner=s.find('impl Default for Workspace {')
    begin=s.find('            layout: Layout {',owner)
    if owner<0 or begin<0:raise RuntimeError('Workspace default layout not found')
    brace=s.find('{',begin);depth=0;end=-1
    for pos in range(brace,len(s)):
        if s[pos]=='{':depth+=1
        elif s[pos]=='}':
            depth-=1
            if depth==0:end=pos+1;break
    if end<0:raise RuntimeError('Workspace Layout unbalanced')
    updated="""            layout: Layout {
                // SCULPT_CORE22_EDIT_DOCK: session center, Memory and Groups on right.
                root: Some(split(SplitAxis::Horizontal,SplitSize::FixedSecond(295.0),
                    tabs(Panel::Canvas),
                    split(SplitAxis::Vertical,SplitSize::FixedFirst(240.0),
                        tabs(Panel::Memory),tabs(Panel::Groups)))),
                floating: vec![],
            }"""
    p.write_text(s[:begin]+updated+s[end:],encoding='utf-8')

# mix does NOT get the new Edit-right side; it retains all available channel area.
transform(dock,
'''    let mut workspace = Workspace::default();
    let _ = workspace.set_visible(Panel::Clips, false);''',
'''    let mut workspace = Workspace::default();
    // SCULPT_CORE22_MIX_DOCK: main Mix canvas uses the full display.
    let _ = workspace.set_visible(Panel::Groups, false);
    let _ = workspace.set_visible(Panel::Memory, false);
    let _ = workspace.set_visible(Panel::Clips, false);''','SCULPT_CORE22_MIX_DOCK')
# Migrate saved pre-Core22 layouts once.
transform('crates/ui-egui/src/lib.rs',
'''    pub show_io_setup: bool,''',
'''    pub show_io_setup: bool,
    pub sculpt_core22_layout_migrated: bool, // SCULPT_CORE22_LAYOUT_FLAG''','SCULPT_CORE22_LAYOUT_FLAG')
transform('crates/ui-egui/src/lib.rs',
'''            show_io_setup: false,''',
'''            show_io_setup: false,
            sculpt_core22_layout_migrated: false, // SCULPT_CORE22_LAYOUT_INIT''','SCULPT_CORE22_LAYOUT_INIT')
transform(dock,
'''    let before_sync = workspace.clone();''',
'''    // SCULPT_CORE22_LAYOUT_MIGRATION: old persisted sidebars should not survive first launch.
    if edit && !app.ui.sculpt_core22_layout_migrated {
        workspace=Workspace::default();
        app.ui.show_tracks_list=false;
        app.ui.show_clip_list=false;
        app.ui.show_midi_editor=false;
        app.ui.show_memory_locations=true;
        app.ui.sculpt_core22_layout_migrated=true;
    }
    let before_sync = workspace.clone();''','SCULPT_CORE22_LAYOUT_MIGRATION')
# Default flags of new sessions.
transform('crates/ui-egui/src/lib.rs',
'''            show_tracks_list: true,
            show_clip_list: true,''',
'''            show_tracks_list: false, // SCULPT_CORE22_NO_TRACKS_SIDEBAR
            show_clip_list: false,''','SCULPT_CORE22_NO_TRACKS_SIDEBAR')
transform('crates/ui-egui/src/lib.rs',
'''            show_memory_locations: false,''',
'''            show_memory_locations: true, // SCULPT_CORE22_MARKERS_VISIBLE''','SCULPT_CORE22_MARKERS_VISIBLE')
# Separate Groups panel; reuse exact group interactions from existing panel.
p=ROOT/'crates/ui-egui/src/panels.rs';s=p.read_text(encoding='utf-8')
if 'SCULPT_CORE22_GROUPS_ONLY' not in s:
    begin=s.find('    panel_header(ui, "GROUPS");')
    end=s.find('\n}\n\npub fn clip_list(',begin)
    if begin<0 or end<0:raise RuntimeError('groups block not found')
    group=s[begin:end]
    s+='''
/// SCULPT_CORE22_GROUPS_ONLY: separate dedicated right-hand Groups panel.
pub fn groups_only(app: &mut SoundApp, ui: &mut Ui) {
    let t=Tokens::current();
'''+group+'\n}\n'
    p.write_text(s,encoding='utf-8')

# Audio Settings replaces the previous read-only Playback Engine; the same config file
# is also consumed by Show and by the active player on reconnect.
settings_ui=r'''/// SCULPT_CORE22_AUDIO_SETTINGS: CoreAudio setup MUST precede I/O routing.
pub fn playback_engine(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open=app.ui.show_playback_engine;
    win(ctx,&mut open,"Sculpt · Audio Settings",vec2(580.0,435.0),|ui|{
        #[cfg(not(target_arch="wasm32"))]
        {
            use soundcraft_playback::sculpt_audio_settings::{AudioSettings,available_output_devices};
            let id=egui::Id::new("sculpt_core22_audio_settings_draft");
            let mut cfg:AudioSettings=ui.ctx().data(|d|d.get_temp(id)).unwrap_or_else(AudioSettings::read);
            ui.heading("CoreAudio interface");
            ui.label("A shared device profile is used by Sculpt Show and Sculpt DAW.");
            let devices=available_output_devices();
            ui.horizontal(|ui|{
                ui.label("Output device");
                egui::ComboBox::from_id_salt("sculpt_core22_audio_output")
                    .selected_text(if cfg.output_name.is_empty(){"System default"}else{&cfg.output_name})
                    .show_ui(ui,|ui|{
                        ui.selectable_value(&mut cfg.output_name,String::new(),"System default");
                        for d in &devices {ui.selectable_value(&mut cfg.output_name,d.name.clone(),format!("{} · {} out",d.name,d.channels));}
                    });
            });
            ui.horizontal(|ui|{
                ui.label("Input device");
                ui.text_edit_singleline(&mut cfg.input_name)
                    .on_hover_text("Optional exact CoreAudio input device name. Leave blank for default input.");
            });
            ui.horizontal(|ui|{
                ui.label("Sample rate");
                egui::ComboBox::from_id_salt("sculpt_core22_rate")
                    .selected_text(if cfg.sample_rate==0{"Session rate".into()}else{format!("{} Hz",cfg.sample_rate)})
                    .show_ui(ui,|ui|{
                        for rate in [0,44100,48000,88200,96000,176400,192000] {
                            ui.selectable_value(&mut cfg.sample_rate,rate,
                                if rate==0{"Session rate".into()}else{format!("{} Hz",rate)});
                        }
                    });
            });
            ui.horizontal(|ui|{
                ui.label("Buffer");
                egui::ComboBox::from_id_salt("sculpt_core22_buffer")
                    .selected_text(if cfg.buffer_frames==0{"Device default".into()}else{format!("{} samples",cfg.buffer_frames)})
                    .show_ui(ui,|ui|{
                        for frames in [0,64,128,256,512,1024,2048]{
                            ui.selectable_value(&mut cfg.buffer_frames,frames,
                                if frames==0{"Device default".into()}else{format!("{} samples",frames)});
                        }
                    });
            });
            if let Some(device)=&app.player {
                ui.separator();ui.label(format!("Active: {} · {} Hz · {} output channels",
                    device.device_name,device.device_rate,device.device_channels));
            }
            ui.label("A device change is rejected during playback or recording. Stop transport first.");
            let running=app.engine.transport.playing||app.engine.transport.recording;
            if ui.add_enabled(!running,egui::Button::new("Apply and reconnect audio")).clicked(){
                let previous=AudioSettings::read();
                // A new choice by name must be resolved to an UID by Show when next opened.
                if previous.output_name!=cfg.output_name {cfg.output_uid.clear();}
                match cfg.save(){
                    Ok(())=>{
                        let trial=soundcraft_playback::Player::new(app.engine.session_arc());
                        if trial.silent {
                            let _=previous.save();
                            app.ui.status="Audio setup failed; previous device configuration restored".into();
                        }else{
                            app.player=Some(trial);
                            app.ui.status="Audio interface reconnected; reopen Sculpt Show when idle to share it".into();
                        }
                    },
                    Err(e)=> app.ui.status=format!("Cannot save audio settings: {e}"),
                }
            }
            ui.ctx().data_mut(|d|d.insert_temp(id,cfg));
        }
        #[cfg(target_arch="wasm32")]
        ui.label("Native CoreAudio settings require the macOS desktop app.");
    });
    app.ui.show_playback_engine=open;
}
'''
rewrite_function('crates/ui-egui/src/windows.rs','pub fn playback_engine(app: &mut SoundApp, ctx: &egui::Context) {',settings_ui,'SCULPT_CORE22_AUDIO_SETTINGS')

# No hardware matrix UI: only physical outputs and one-click track assignment.
io_ui=r'''/// SCULPT_CORE22_SIMPLE_IO: no user-editable matrix, just physical destinations.
pub fn io_setup(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open=app.ui.show_io_setup;
    win(ctx,&mut open,"Sculpt · I/O Setup",vec2(540.0,490.0),|ui|{
        let count=app.player.as_ref().map(|p|usize::from(p.device_channels)).unwrap_or(0).min(64);
        ui.horizontal(|ui|{
            ui.heading("Physical outputs");
            if ui.button("Audio Settings…").clicked(){app.ui.show_playback_engine=true;}
        });
        ui.label(app.player.as_ref().map_or("No connected audio output".into(),|p|format!("{} · {} active outputs",p.device_name,count)));
        let selected=app.engine.session().edit.selected_tracks.first().copied();
        let Some(track)=selected else {ui.separator();ui.label("Select a track in Edit or Mix to assign its output.");return};
        let current=app.engine.session().track(track).map(|t|format!("{:?}",t.mixer.output)).unwrap_or_default();
        ui.label(format!("Track output: {current}"));
        if ui.button("Main (monitor)").clicked(){let _=app.run("track.output",json!({"tracks":[track.0],"output":"main","create_bus":false}));}
        ui.separator();
        ui.label("Assign channel(s), with the same numbering used in Sculpt Show:");
        egui::ScrollArea::vertical().max_height(275.0).show(ui,|ui|{
            if count==0 {ui.label("Connect an interface in Audio Settings first.");return;}
            for first in 0..count {
                ui.horizontal(|ui|{
                    let mono=format!("SCULPT HW MONO {:02}",first+1);
                    if ui.button(format!("Out {}",first+1)).clicked(){
                        let exists=app.engine.session().outputs.iter().any(|p|p.name==mono);
                        if !exists{let _=app.run("setup.io",json!({"action":"create_output","name":mono,"first_channel":first,"format":"mono"}));}
                        let _=app.run("track.output",json!({"tracks":[track.0],"output":mono,"create_bus":false}));
                    }
                    if first+1<count {
                        let stereo=format!("SCULPT HW STEREO {:02}-{:02}",first+1,first+2);
                        if ui.button(format!("Out {}–{}",first+1,first+2)).clicked(){
                            let exists=app.engine.session().outputs.iter().any(|p|p.name==stereo);
                            if !exists{let _=app.run("setup.io",json!({"action":"create_output","name":stereo,"first_channel":first,"format":"stereo"}));}
                            let _=app.run("track.output",json!({"tracks":[track.0],"output":stereo,"create_bus":false}));
                        }
                    }
                });
            }
        });
    });
    app.ui.show_io_setup=open;
}
'''
rewrite_function('crates/ui-egui/src/windows.rs','pub fn io_setup(app: &mut SoundApp, ctx: &egui::Context) {',io_ui,'SCULPT_CORE22_SIMPLE_IO')

# Ensure tests fail if the old matrix UI or bottom strip remains.
checks={
'crates/ui-egui/src/windows.rs':['SCULPT_CORE22_AUDIO_SETTINGS','SCULPT_CORE22_SIMPLE_IO'],
'crates/ui-egui/src/edit_window.rs':['SCULPT_CORE22_EDIT_LEFT_STRIP'],
'crates/ui-egui/src/docking.rs':['SCULPT_CORE22_GROUP_RENDER','SCULPT_CORE22_EDIT_DOCK','SCULPT_CORE22_LAYOUT_MIGRATION'],
'crates/playback/src/lib.rs':['SCULPT_CORE22_EXPLICIT_DEVICE','SCULPT_CORE22_BUFFER_APPLIED'],
'crates/ui-egui/src/mix_window.rs':['SCULPT_CORE22_SHARED_STRIP','SCULPT_CORE22_SELECTION_SYNC'],
}
for f,tags in checks.items():
    txt=(ROOT/f).read_text(encoding='utf-8')
    for tag in tags:
        assert tag in txt,f'{f}: missing {tag}'
assert 'sculpt_core21_output_matrix' not in (ROOT/'crates/ui-egui/src/windows.rs').read_text(), 'old matrix survived'
print('SCULPT CORE22: Audio Settings, direct I/O, Edit right stack, shared selected strip source guards OK')
