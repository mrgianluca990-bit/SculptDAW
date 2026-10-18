//! I/O Setup uses a draft: Cancel is side-effect free; Apply is one undoable engine command.
use crate::SoundApp;
use egui::{Color32, RichText};
use serde_json::{Value, json};
use soundcraft_model::{Bus, BusId, ChannelFormat, OutputPath, Session};
use soundcraft_playback::sculpt_audio_settings::{AudioDeviceInfo, AudioSettings, available_devices};

#[derive(Clone)]
struct Draft {
    base: Value,
    inputs: Vec<OutputPath>,
    outputs: Vec<OutputPath>,
    busses: Vec<Bus>,
    tab: usize,
    selected: Option<usize>,
    new_name: String,
    stereo: bool,
    error: Option<String>,
    devices: (Vec<AudioDeviceInfo>, Vec<AudioDeviceInfo>),
    settings: AudioSettings,
    next_bus: u64,
    session_rate: u32,
}
impl Draft {
    fn new(s: &Session) -> Self {
        Self {
            base: json!({"inputs":s.inputs,"outputs":s.outputs,"busses":s.busses}),
            inputs: s.inputs.clone(),
            outputs: s.outputs.clone(),
            busses: s.busses.clone(),
            tab: 0,
            selected: None,
            new_name: String::new(),
            stereo: true,
            error: None,
            devices: (available_devices(false), available_devices(true)),
            settings: AudioSettings::read(),
            next_bus: s.next_id,
            session_rate: s.sample_rate.hz(),
        }
    }
    fn request(&self) -> Value {
        json!({"expected":self.base,"inputs":self.inputs,"outputs":self.outputs,"busses":self.busses})
    }
    fn count(&self, input: bool) -> usize {
        let (devices, name) = if input { (&self.devices.0, &self.settings.input_name) } else { (&self.devices.1, &self.settings.output_name) };
        devices.iter().find(|d| if name.is_empty() { d.is_default } else { d.name == *name }).map_or(0, |d| {
            let rate = if self.settings.sample_rate == 0 { self.session_rate } else { self.settings.sample_rate };
            d.configs.iter().filter(|c| c.supports(rate, self.settings.buffer_frames)).map(|c| usize::from(c.channels)).max().unwrap_or(0).min(64)
        })
    }
    fn device_name(&self, input: bool) -> &str {
        let (devices, name) = if input { (&self.devices.0, &self.settings.input_name) } else { (&self.devices.1, &self.settings.output_name) };
        devices.iter().find(|d| if name.is_empty() { d.is_default } else { d.name == *name }).map_or("Not connected", |d| d.name.as_str())
    }
    fn new_path(&mut self) {
        let format = if self.stereo { ChannelFormat::Stereo } else { ChannelFormat::Mono };
        if self.tab == 2 {
            if self.busses.len() >= 256 {
                return;
            }
            let mut n = self.busses.len() + 1;
            let name = if self.new_name.trim().is_empty() {
                while self.busses.iter().any(|p| p.name == format!("Bus {n}")) {
                    n += 1;
                }
                format!("Bus {n}")
            } else {
                self.new_name.trim().to_string()
            };
            self.busses.push(Bus { id: BusId(self.next_bus), name, format, output: None });
            self.next_bus = self.next_bus.saturating_add(1);
            self.selected = Some(self.busses.len() - 1);
        } else {
            let input = self.tab == 0;
            let paths = if input { &mut self.inputs } else { &mut self.outputs };
            if paths.len() >= 128 {
                return;
            }
            let id = paths.iter().map(|p| p.id).max().unwrap_or(0).saturating_add(1);
            let mut n = paths.len() + 1;
            let prefix = if input { "Input" } else { "Output" };
            let name = if self.new_name.trim().is_empty() {
                while paths.iter().any(|p| p.name == format!("{prefix} {n}")) {
                    n += 1;
                }
                format!("{prefix} {n}")
            } else {
                self.new_name.trim().to_string()
            };
            paths.push(OutputPath { id, active: true, monitor_on_main: false, name, first_channel: 0, format });
            self.selected = Some(paths.len() - 1);
        }
        self.new_name.clear();
    }
    fn delete_selected(&mut self) {
        let Some(index) = self.selected.take() else { return };
        if self.tab == 2 {
            if index < self.busses.len() {
                self.busses.remove(index);
            }
        } else {
            if self.tab == 1 && index == 0 {
                return;
            }
            let paths = if self.tab == 0 { &mut self.inputs } else { &mut self.outputs };
            if index >= paths.len() {
                return;
            }
            let removed = paths.remove(index);
            if self.tab == 1 {
                for bus in &mut self.busses {
                    if bus.output.as_deref().is_some_and(|key| key == removed.route_key(false) || key == removed.name) {
                        bus.output = None;
                    }
                }
            }
        }
    }
    fn defaults(&mut self) {
        if self.tab == 2 {
            self.busses.clear();
            for n in 0..8 {
                self.busses.push(Bus {
                    id: BusId(self.next_bus),
                    name: format!("Bus {}–{}", n * 2 + 1, n * 2 + 2),
                    format: ChannelFormat::Stereo,
                    output: None,
                });
                self.next_bus = self.next_bus.saturating_add(1);
            }
        } else {
            let input = self.tab == 0;
            let channels = self.count(input);
            if channels == 0 {
                self.error = Some("Connect an interface and refresh devices before creating default paths".into());
                return;
            }
            let paths = if input { &mut self.inputs } else { &mut self.outputs };
            let mut next = paths.iter().map(|p| p.id).max().unwrap_or(0).saturating_add(1);
            let main = if input {
                None
            } else {
                Some(paths.first().cloned().unwrap_or_else(|| {
                    let id = next;
                    next = next.saturating_add(1);
                    OutputPath { id, active: true, monitor_on_main: false, name: "Out 1-2".into(), first_channel: 0, format: ChannelFormat::Stereo }
                }))
            };
            let mut defaults = Vec::new();
            if let Some(mut main) = main {
                main.first_channel = 0;
                main.active = true;
                main.format = ChannelFormat::Stereo;
                defaults.push(main);
            }
            let prefix = if input { "In" } else { "Out" };
            for width in [1usize, 2] {
                for first in (0..channels).step_by(width) {
                    if first + width > channels || (!input && width == 2 && first == 0) {
                        continue;
                    }
                    let name = if width == 1 { format!("{prefix} {}", first + 1) } else { format!("{prefix} {}-{}", first + 1, first + 2) };
                    if defaults.iter().any(|p: &OutputPath| p.name == name) {
                        continue;
                    }
                    let id = paths.iter().find(|p| p.name == name && !defaults.iter().any(|d| d.id == p.id)).map_or_else(
                        || {
                            let id = next;
                            next = next.saturating_add(1);
                            id
                        },
                        |p| p.id,
                    );
                    defaults.push(OutputPath {
                        id,
                        active: true,
                        monitor_on_main: false,
                        name,
                        first_channel: first as u16,
                        format: if width == 1 { ChannelFormat::Mono } else { ChannelFormat::Stereo },
                    });
                }
            }
            *paths = defaults;
            if !input {
                for bus in &mut self.busses {
                    if bus.output.as_deref().is_some_and(|key| !self.outputs.iter().any(|p| p.route_key(false) == key || p.name == key)) {
                        bus.output = None;
                    }
                }
            }
        }
        self.selected = None;
    }
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let key = egui::Id::new("sculpt_core27_io_draft");
    if !app.ui.show_io_setup {
        ctx.data_mut(|d| d.remove::<Draft>(key));
        return;
    }
    let mut draft = ctx.data(|d| d.get_temp::<Draft>(key)).unwrap_or_else(|| Draft::new(app.engine.session()));
    draft.tab = app.ui.io_setup_tab.min(2);
    let mut open = true;
    let mut close = false;
    egui::Window::new("I/O Setup").open(&mut open).default_size(egui::vec2(1020.0,580.0)).min_size(egui::vec2(680.0,400.0)).show(ctx,|ui| {
        ui.horizontal(|ui| {
            for (tab,label) in ["Input","Output","Bus"].iter().enumerate() {
                if ui.selectable_label(draft.tab==tab,*label).clicked() {draft.tab=tab;app.ui.io_setup_tab=tab;draft.selected=None;}
            }
            ui.separator();
            if ui.button("Playback Engine…").clicked() {app.ui.show_playback_engine=true;}
            if ui.button("Refresh devices").clicked() {draft.devices=(available_devices(false),available_devices(true));draft.settings=AudioSettings::read();}
        });
        ui.separator();
        if draft.tab==2 {
            ui.label("Internal buses collect track outputs and sends. Map a bus to an output path to hear it directly, or use it as an aux input.");
        } else {
            let input=draft.tab==0;
            let active_count=if input {app.recorder.as_ref().map(|r|r.channels)}else{app.player.as_ref().filter(|p|!p.silent).map(|p|usize::from(p.device_channels))};
            ui.label(format!("{} · {} channels{}",draft.device_name(input),draft.count(input),active_count.map_or(String::new(),|c|format!(" · {c} currently open"))));
            ui.label("Click a matrix cell to choose the first hardware channel. Stereo paths occupy adjacent channels. Grey columns are unavailable.");
        }
        ui.add_space(8.0);
        egui::ScrollArea::both().id_salt("core27_io_matrix_scroll").max_height(340.0).auto_shrink([false,false]).show(ui,|ui| {
            if draft.tab==2 {bus_matrix(ui,&mut draft);} else {path_matrix(ui,&mut draft);}
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.new_name).hint_text("New path name").desired_width(200.0));
            egui::ComboBox::from_id_salt("core27_new_format").selected_text(if draft.stereo {"Stereo"}else{"Mono"}).show_ui(ui,|ui| {
                ui.selectable_value(&mut draft.stereo,false,"Mono");ui.selectable_value(&mut draft.stereo,true,"Stereo");
            });
            if ui.button("New Path").clicked() {draft.new_path();}
            let can_delete=draft.selected.is_some_and(|i|draft.tab!=1 || i!=0);
            if ui.add_enabled(can_delete,egui::Button::new("Delete Path")).clicked() {draft.delete_selected();}
            if ui.button("Default").clicked() {draft.defaults();}
        });
        if let Some(error)=&draft.error {ui.colored_label(Color32::from_rgb(255,160,120),error);}
        ui.label("Names and assignments are saved with the project. Deleting a path disconnects its assignments. Apply is undoable; Cancel discards this draft.");
        if app.engine.transport.recording {ui.colored_label(Color32::YELLOW,"Stop recording before applying I/O changes.");}
        ui.horizontal(|ui| {
            if ui.button("Revert").clicked() {let tab=draft.tab;draft=Draft::new(app.engine.session());draft.tab=tab;}
            if ui.button("Cancel").clicked() {close=true;}
            let changed=draft.request().get("expected").is_some_and(|expected|*expected!=json!({"inputs":draft.inputs,"outputs":draft.outputs,"busses":draft.busses}));
            let can_apply=!app.engine.transport.recording;
            let apply=ui.add_enabled(can_apply && changed,egui::Button::new("Apply")).clicked();
            let ok=ui.add_enabled(can_apply,egui::Button::new("OK")).clicked();
            if apply || (ok && changed) {
                match app.run("setup.io.apply",draft.request()) {
                    Ok(_)=>{let tab=draft.tab;draft=Draft::new(app.engine.session());draft.tab=tab;if ok {close=true;}},
                    Err(error)=>draft.error=Some(error),
                }
            } else if ok {close=true;}
        });
    });
    app.ui.show_io_setup = open && !close;
    if app.ui.show_io_setup {
        ctx.data_mut(|d| d.insert_temp(key, draft));
    } else {
        ctx.data_mut(|d| d.remove::<Draft>(key));
    }
}

fn path_matrix(ui: &mut egui::Ui, draft: &mut Draft) {
    let input = draft.tab == 0;
    let available = draft.count(input);
    let paths = if input { &mut draft.inputs } else { &mut draft.outputs };
    let columns =
        available.max(paths.iter().map(|p| usize::from(p.first_channel).saturating_add(p.format.channels())).max().unwrap_or(0)).clamp(2, 64);
    egui::Grid::new(("core27_paths", input)).striped(true).spacing(egui::vec2(5.0, 5.0)).show(ui, |ui| {
        ui.label("Active");
        ui.label("Path Name");
        ui.label("Format");
        ui.label("Status");
        for c in 0..columns {
            ui.add_sized(
                [26.0, 22.0],
                egui::Label::new(RichText::new((c + 1).to_string()).color(if c < available { Color32::WHITE } else { Color32::GRAY })),
            );
        }
        ui.end_row();
        for (row, path) in paths.iter_mut().enumerate() {
            {
                if ui.checkbox(&mut path.active, "").clicked() {
                    draft.selected = Some(row);
                }
                if ui
                    .add_sized([220.0, 24.0], egui::TextEdit::singleline(&mut path.name))
                    .on_hover_text("Click the name to select this path for deletion")
                    .has_focus()
                {
                    draft.selected = Some(row);
                }
                egui::ComboBox::from_id_salt(("format", input, row)).width(62.0).selected_text(path.format.label()).show_ui(ui, |ui| {
                    if input || row != 0 {
                        ui.selectable_value(&mut path.format, ChannelFormat::Mono, "Mono");
                    }
                    ui.selectable_value(&mut path.format, ChannelFormat::Stereo, "Stereo");
                });
                let ok = path.available(available);
                ui.label(
                    RichText::new(if !path.active {
                        "Inactive"
                    } else if ok {
                        "Available"
                    } else {
                        "Unavailable"
                    })
                    .color(if ok { Color32::from_rgb(120, 210, 150) } else { Color32::GRAY }),
                );
                for c in 0..columns {
                    let width = path.format.channels();
                    let selected = (usize::from(path.first_channel)..usize::from(path.first_channel) + width).contains(&c);
                    let valid = (selected && c < available) || c.saturating_add(width) <= available;
                    let label = if selected {
                        if width == 1 {
                            "●"
                        } else if c == usize::from(path.first_channel) {
                            "L"
                        } else {
                            "R"
                        }
                    } else {
                        "·"
                    };
                    if ui.add_enabled(valid, egui::Button::new(label).selected(selected).min_size(egui::vec2(26.0, 22.0))).clicked() {
                        if c.saturating_add(width) <= available {
                            path.first_channel = c as u16;
                        }
                        draft.selected = Some(row);
                    }
                }
                ui.end_row();
            }
        }
    });
    if paths.is_empty() {
        ui.label("No input paths yet. Use Default for the selected interface or create a named path.");
    }
}
fn bus_matrix(ui: &mut egui::Ui, draft: &mut Draft) {
    let available = draft.count(false);
    egui::Grid::new("core27_bus_matrix").striped(true).spacing(egui::vec2(8.0, 6.0)).show(ui, |ui| {
        ui.label("");
        ui.label("Bus Name");
        ui.label("Format");
        ui.label("Internal");
        for path in &draft.outputs {
            ui.label(&path.name);
        }
        ui.end_row();
        for (row, bus) in draft.busses.iter_mut().enumerate() {
            {
                if ui.selectable_label(draft.selected == Some(row), "●").clicked() {
                    draft.selected = Some(row);
                }
                if ui.add_sized([220.0, 24.0], egui::TextEdit::singleline(&mut bus.name)).has_focus() {
                    draft.selected = Some(row);
                }
                egui::ComboBox::from_id_salt(("bus_format", row)).selected_text(bus.format.label()).show_ui(ui, |ui| {
                    ui.selectable_value(&mut bus.format, ChannelFormat::Mono, "Mono");
                    ui.selectable_value(&mut bus.format, ChannelFormat::Stereo, "Stereo");
                });
                if ui.selectable_label(bus.output.is_none(), "Internal").clicked() {
                    bus.output = None;
                }
                for path in &draft.outputs {
                    let selected = bus.output.as_deref().is_some_and(|key| key == path.route_key(false) || key == path.name);
                    if ui
                        .selectable_label(selected, if selected { "●" } else { "·" })
                        .on_hover_text(if path.available(available) {
                            "Map this bus to this output"
                        } else {
                            "Output currently unavailable; assignment is retained"
                        })
                        .clicked()
                    {
                        bus.output = Some(path.route_key(false));
                    }
                }
                ui.end_row();
            }
        }
    });
    if draft.busses.is_empty() {
        ui.label("No internal buses. Create a mono or stereo bus with New Path.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    fn harness() -> Harness<'static, SoundApp> {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.ui.show_io_setup = true;
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(|ui, app| show(app, ui.ctx()), app);
        h.run_steps(4);
        h
    }
    #[test]
    fn cancel_discards_new_paths_without_dirtying_the_document() {
        let mut h = harness();
        h.get_by_label("New Path").click();
        h.run_steps(3);
        assert!(h.state().engine.session().inputs.is_empty());
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert!(!h.state().ui.show_io_setup);
        assert!(h.state().engine.session().inputs.is_empty());
        assert!(!h.state().engine.is_dirty());
    }
    #[test]
    fn apply_commits_the_draft_and_one_undo_restores_the_previous_io() {
        let mut h = harness();
        h.get_by_label("New Path").click();
        h.run_steps(3);
        h.get_by_label("Apply").click();
        h.run_steps(3);
        assert_eq!(h.state().engine.session().inputs.len(), 1);
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert!(h.state_mut().engine.undo());
        assert!(h.state().engine.session().inputs.is_empty());
    }
}
