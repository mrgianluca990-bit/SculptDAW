//! Application panel identities and persistence around the shared docking renderer.
//! Document contents remain owned by their existing view implementations.

use craft_ui::docking::{Action, DockArea, DockStyle, Layout, Location, Node, PanelLimits, Permissions, Placement, Zone};
use craft_ui::layout::{SplitAxis, SplitSize};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Panel {
    Canvas,
    Tracks,
    Clips,
    Midi,
    Memory,
    Groups, // SCULPT_CORE22_GROUP_PANEL
    History,
}

impl Panel {
    pub const ALL: &[Self] = &[Self::Canvas, Self::Tracks, Self::Clips, Self::Midi, Self::Memory, Self::Groups, Self::History]; // SCULPT_CORE22_GROUP_ALL
    pub fn id(self) -> &'static str {
        match self {
            Self::Canvas => "canvas",
            Self::Tracks => "tracks",
            Self::Clips => "clips",
            Self::Midi => "midi",
            Self::Memory => "memory",
            Self::Groups => "groups", // SCULPT_CORE22_GROUP_ID
            Self::History => "history",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Canvas => "Session",
            Self::Tracks => "Groups / Selected Channel", // SCULPT_CORE19_PANEL_NAME
            Self::Clips => "Clips",
            Self::Midi => "MIDI Editor",
            Self::Memory => "Memory Locations",
            Self::Groups => "Groups", // SCULPT_CORE22_GROUP_LABEL
            Self::History => "Undo History",
        }
    }
    fn default_side(self) -> Zone {
        match self {
            Self::Canvas => Zone::Right,
            Self::Tracks => Zone::Left,
            Self::Clips => Zone::Right,
            Self::Midi => Zone::Bottom,
            Self::Memory => Zone::Right,
            Self::Groups => Zone::Right, // SCULPT_CORE22_MEMORY_RIGHT
            Self::History => Zone::Right,
        }
    }
    fn parse(id: &str) -> Result<Self, String> {
        Self::ALL.iter().copied().find(|panel| panel.id() == id).ok_or_else(|| format!("Unknown panel: {id}"))
    }
}

const MIDI_TAB_HEIGHT: f32 = 26.0;

fn permissions(panel: &Panel) -> Permissions {
    if *panel == Panel::Canvas { Permissions::PROTECTED } else { Permissions::default() }
}

fn tabs(panel: Panel) -> Node<Panel> {
    Node::Tabs { panels: vec![panel], active: 0 }
}

fn split(axis: SplitAxis, size: SplitSize, first: Node<Panel>, second: Node<Panel>) -> Node<Panel> {
    Node::Split { axis, size, first: Box::new(first), second: Box::new(second) }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub layout: Layout<Panel>,
    /// Closed panels retain their most recent group, side, size or floating rectangle.
    pub hidden: Vec<(Panel, Location<Panel>)>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            layout: Layout {
                // SCULPT_CORE22_EDIT_DOCK: session center, Memory and Groups on right.
                root: Some(split(
                    SplitAxis::Horizontal,
                    SplitSize::FixedSecond(295.0),
                    tabs(Panel::Canvas),
                    split(SplitAxis::Vertical, SplitSize::FixedFirst(240.0), tabs(Panel::Memory), tabs(Panel::Groups)),
                )),
                floating: vec![],
            },
            hidden: Vec::new(),
        }
    }
}

impl Workspace {
    pub fn validate(&self) -> Result<(), String> {
        self.layout.validate().map_err(|error| error.to_string())?;
        if !self.layout.contains(&Panel::Canvas) || self.hidden.len() > Panel::ALL.len() {
            return Err("The panel layout must contain its document view and bounded hidden panels".into());
        }
        // Saved settings cannot turn the protected document view into a floating or closable tab.
        if self.layout.floating.iter().any(|group| group.panels.contains(&Panel::Canvas)) {
            return Err("The document view cannot float".into());
        }
        let mut nodes: Vec<_> = self.layout.root.iter().collect();
        while let Some(node) = nodes.pop() {
            match node {
                Node::Split { first, second, .. } => {
                    nodes.push(first);
                    nodes.push(second);
                }
                Node::Tabs { panels, .. } if panels.contains(&Panel::Canvas) && panels.len() != 1 => {
                    return Err("The document view cannot share a tab group".into());
                }
                Node::Stack { entries } if entries.iter().any(|entry| entry.panel == Panel::Canvas) => {
                    return Err("The document view cannot be collapsed".into());
                }
                _ => {}
            }
        }
        let mut seen = std::collections::HashSet::new();
        for (panel, location) in &self.hidden {
            if *panel == Panel::Canvas || self.layout.contains(panel) || !seen.insert(*panel) {
                return Err("A hidden panel has an invalid or duplicate identity".into());
            }
            if location.anchor == Some(Panel::Canvas) && matches!(location.placement, Placement::Tab { .. } | Placement::Split(Zone::Center)) {
                return Err("A hidden panel cannot restore over the document view".into());
            }
            let mut probe = self.layout.clone();
            if let Err(error) = probe.restore(*panel, location)
                && error != craft_ui::docking::DockError::MissingTarget
            {
                return Err(error.to_string());
            }
        }
        Ok(())
    }

    pub fn validate_mix(&self) -> Result<(), String> {
        self.validate()?;
        if self.layout.contains(&Panel::Clips) || self.layout.contains(&Panel::Midi) {
            return Err("Clips and MIDI panels belong to the Edit window".into());
        }
        Ok(())
    }

    pub fn apply(&mut self, action: Action<Panel>) -> Result<(), String> {
        self.validate()?;
        let mut next = self.clone();
        if let Action::Close { panel } = &action {
            let location = next.layout.location(panel).map_err(|error| error.to_string())?;
            next.hidden.retain(|(id, _)| id != panel);
            next.hidden.push((*panel, location));
        } else if let Action::Open { panel, .. } | Action::OpenAt { panel, .. } = &action {
            next.hidden.retain(|(id, _)| id != panel);
        }
        next.layout.apply_with_permissions(action, permissions).map_err(|error| error.to_string())?;
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn set_visible(&mut self, panel: Panel, visible: bool) -> Result<(), String> {
        if self.layout.contains(&panel) == visible {
            return Ok(());
        }
        if !visible {
            return self.apply(Action::Close { panel });
        }
        self.validate()?;
        let mut next = self.clone();
        let mut location = next.hidden.iter().find(|(id, _)| *id == panel).map(|(_, location)| location.clone());
        // A newly opened MIDI editor has a predictable body height, rather than half
        // the current canvas. Closed editors still restore their operator-chosen size.
        if location.is_none() && panel == Panel::Midi {
            location = Some(Location {
                floating: None,
                anchor: Some(Panel::Canvas),
                placement: Placement::Split(Zone::Bottom),
                size: Some(SplitSize::FixedSecond(320.0 + MIDI_TAB_HEIGHT)),
                stack: None,
                sibling: Vec::new(),
            });
        }
        next.hidden.retain(|(id, _)| *id != panel);
        if let Some(saved) = &mut location
            && saved.floating.is_some()
            && saved.anchor.as_ref().is_some_and(|anchor| !next.layout.floating.iter().any(|group| group.panels.contains(anchor)))
        {
            // The other members of a floating group may have been closed too.
            saved.anchor = None;
            saved.placement = Placement::Tab { before: None };
        }
        let restored = location.as_ref().is_some_and(|location| next.layout.restore(panel, location).is_ok());
        if !restored {
            next.layout
                .apply_with_permissions(
                    Action::OpenAt { panel, anchor: Panel::Canvas, placement: Placement::Split(panel.default_side()) },
                    permissions,
                )
                .map_err(|error| error.to_string())?;
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    fn command(&mut self, params: &Value) -> Result<(), String> {
        self.validate()?;
        if let Some(action) = params.get("action") {
            let action: Action<Panel> = serde_json::from_value(action.clone()).map_err(|error| error.to_string())?;
            return self.apply(action);
        }
        let operation = params.get("operation").and_then(Value::as_str).ok_or("operation is required")?;
        if operation == "resizeSplit" {
            let path: Vec<bool> =
                serde_json::from_value(params.get("path").cloned().ok_or("path is required")?).map_err(|error| error.to_string())?;
            let size: SplitSize =
                serde_json::from_value(params.get("size").cloned().ok_or("size is required")?).map_err(|error| error.to_string())?;
            return self.apply(Action::ResizeSplit { path, size });
        }
        let panel = Panel::parse(params.get("panel").and_then(Value::as_str).ok_or("panel is required")?)?;
        let action = match operation {
            "open" => return self.set_visible(panel, true),
            "close" => Action::Close { panel },
            "activate" => Action::Activate { panel },
            "setStackOpen" => Action::SetStackOpen { panel, open: params.get("open").and_then(Value::as_bool).ok_or("open must be a boolean")? },
            "resizeStack" => {
                let value = params.get("height").ok_or("height is required")?;
                let height = if value.is_null() {
                    None
                } else {
                    Some(
                        value
                            .as_f64()
                            .filter(|height| height.is_finite() && (0.0..=1_000_000.0).contains(height))
                            .ok_or("height must be finite and bounded")? as f32,
                    )
                };
                Action::ResizeStack { panel, height }
            }
            "float" | "moveFloating" => {
                let rect = params.get("rect").and_then(Value::as_array).filter(|rect| rect.len() == 4).ok_or("rect must be [x,y,width,height]")?;
                let mut values = [0.0f32; 4];
                for (output, value) in values.iter_mut().zip(rect) {
                    *output = value
                        .as_f64()
                        .filter(|number| number.is_finite() && number.abs() <= 1_000_000.0)
                        .ok_or("rect values must be finite and bounded")? as f32;
                }
                if operation == "float" { Action::Float { panel, rect: values } } else { Action::MoveFloating { panel, rect: values } }
            }
            "move" => {
                let anchor = Panel::parse(params.get("target").and_then(Value::as_str).ok_or("target is required")?)?;
                let placement = match params.get("zone").and_then(Value::as_str).unwrap_or("center") {
                    "center" => Placement::Split(Zone::Center),
                    "left" => Placement::Split(Zone::Left),
                    "right" => Placement::Split(Zone::Right),
                    "top" => Placement::Split(Zone::Top),
                    "bottom" => Placement::Split(Zone::Bottom),
                    _ => return Err("zone must be center, left, right, top or bottom".into()),
                };
                let placement = if let Some(before) = params.get("before") {
                    if !matches!(placement, Placement::Split(Zone::Center)) {
                        return Err("before only applies to a center move".into());
                    }
                    Placement::Tab {
                        before: if before.is_null() {
                            None
                        } else {
                            Some(Panel::parse(before.as_str().ok_or("before must be a panel ID or null")?)?)
                        },
                    }
                } else {
                    placement
                };
                Action::Move { panel, anchor, placement }
            }
            _ => return Err("operation must be open, close, activate, float, move, moveFloating, resizeSplit, setStackOpen or resizeStack".into()),
        };
        self.apply(action)
    }
}

/// Mix retains its own arrangement; the timeline-only Clips and MIDI panels stay in Edit.
pub fn mix_default() -> Workspace {
    let mut workspace = Workspace::default();
    // SCULPT_CORE22_MIX_DOCK: main Mix canvas uses the full display.
    let _ = workspace.set_visible(Panel::Groups, false);
    let _ = workspace.set_visible(Panel::Memory, false);
    let _ = workspace.set_visible(Panel::Clips, false);
    let _ = workspace.set_visible(Panel::Midi, false);
    let _ = workspace.set_visible(Panel::Tracks, false); // SCULPT_CORE19_MIX_NO_TRACKS_SIDEBAR
    workspace
}

fn sync(workspace: &mut Workspace, app: &crate::SoundApp, edit: bool) -> Result<(), String> {
    // Edit uses its sidebar preference. Mix keeps panels explicitly opened in its own layout.
    if edit {
        workspace.set_visible(Panel::Tracks, app.ui.show_tracks_list)?;
    }
    workspace.set_visible(Panel::Memory, app.ui.show_memory_locations)?;
    workspace.set_visible(Panel::History, app.ui.show_undo_history)?;
    if edit {
        workspace.set_visible(Panel::Clips, app.ui.show_clip_list)?;
        workspace.set_visible(Panel::Midi, app.ui.show_midi_editor)?;
    }
    Ok(())
}

fn publish_visibility(app: &mut crate::SoundApp, workspace: &Workspace, edit: bool) {
    if edit {
        app.ui.show_tracks_list = workspace.layout.contains(&Panel::Tracks);
    }
    app.ui.show_memory_locations = workspace.layout.contains(&Panel::Memory); // SCULPT_CORE19_SHARED_MEMORY_VISIBILITY
    app.ui.show_undo_history = workspace.layout.contains(&Panel::History);
    if edit {
        app.ui.show_clip_list = workspace.layout.contains(&Panel::Clips);
        app.ui.show_midi_editor = workspace.layout.contains(&Panel::Midi);
    }
}

fn store(app: &mut crate::SoundApp, workspace: Workspace, edit: bool) {
    if edit {
        app.ui.docking = workspace;
    } else {
        app.ui.mix_docking = workspace;
    }
}

pub fn command(app: &mut crate::SoundApp, reset: bool, params: &Value) -> Result<Value, String> {
    let edit = app.ui.window == crate::MainWindow::Edit;
    let previous = if edit { app.ui.docking.clone() } else { app.ui.mix_docking.clone() };
    let result = command_for(app, reset, params, edit)?;
    let next = if edit { &app.ui.docking } else { &app.ui.mix_docking };
    cancel_replaced_workspace(app, &previous, next, edit);
    Ok(result)
}

fn cancel_replaced_workspace(app: &crate::SoundApp, previous: &Workspace, next: &Workspace, edit: bool) {
    if (previous.layout != next.layout || previous.hidden != next.hidden)
        && let Some(ctx) = &app.docking_context
    {
        craft_ui::docking::cancel_drag::<Panel>(ctx, egui::Id::new(("soundcraft-docking", edit)));
    }
}

/// Restoring complete preferences/configurations uses the same ownership boundary as commands.
pub(crate) fn cancel_replaced_state(app: &crate::SoundApp, next: &crate::UiState) {
    cancel_replaced_workspace(app, &app.ui.docking, &next.docking, true);
    cancel_replaced_workspace(app, &app.ui.mix_docking, &next.mix_docking, false);
}

fn command_for(app: &mut crate::SoundApp, reset: bool, params: &Value, edit: bool) -> Result<Value, String> {
    if !edit && ["panel", "target"].iter().any(|field| matches!(params.get(*field).and_then(Value::as_str), Some("clips" | "midi"))) {
        return Err("Clips and MIDI panels belong to the Edit window".into());
    }
    let mut workspace = if edit { app.ui.docking.clone() } else { app.ui.mix_docking.clone() };
    if !reset && !edit {
        workspace.validate_mix()?;
    }
    if reset {
        workspace = if edit { Workspace::default() } else { mix_default() };
        sync(&mut workspace, app, edit)?;
    } else {
        sync(&mut workspace, app, edit)?;
        workspace.command(params)?;
        if !edit {
            workspace.validate_mix()?;
        }
    }
    publish_visibility(app, &workspace, edit);
    let result = json!({"docking": workspace});
    store(app, workspace, edit);
    Ok(result)
}

/// Existing Arrange commands reposition shared floating panels along with the legacy windows.
pub(crate) fn arrange(app: &mut crate::SoundApp) -> Result<(), String> {
    let edit = app.ui.window == crate::MainWindow::Edit;
    let mut workspace = if edit { app.ui.docking.clone() } else { app.ui.mix_docking.clone() };
    if edit {
        workspace.validate()?;
    } else {
        workspace.validate_mix()?;
    }
    let floating: Vec<_> = workspace.layout.floating.iter().filter_map(|group| group.panels.first().map(|panel| (*panel, group.rect))).collect();
    for (index, (panel, mut rect)) in floating.into_iter().enumerate() {
        rect[0] = 32.0 + index as f32 * 28.0;
        rect[1] = 80.0 + index as f32 * 28.0;
        workspace.apply(Action::MoveFloating { panel, rect })?;
    }
    // Arrange cancels a held divider even when every panel was already docked.
    if let Some(ctx) = &app.docking_context {
        craft_ui::docking::cancel_drag::<Panel>(ctx, egui::Id::new(("soundcraft-docking", edit)));
    }
    store(app, workspace, edit);
    Ok(())
}

pub fn show(app: &mut crate::SoundApp, ui: &mut egui::Ui, window: crate::MainWindow) {
    app.docking_context = Some(ui.ctx().clone());
    let t = crate::theme::Tokens::current();
    let edit = window == crate::MainWindow::Edit;
    let mut workspace = if edit { std::mem::take(&mut app.ui.docking) } else { std::mem::replace(&mut app.ui.mix_docking, mix_default()) };
    // SCULPT_CORE20_MARKER_DEFAULT_RIGHT: Core18's right default is preserved.
    // SCULPT_CORE20_MARKER_RIGHT_MIGRATION: move a pre-Core20 Marker/Memory
    // panel to the right once, while leaving subsequent operator panel moves intact.
    if (edit && !app.ui.sculpt_core20_marker_placed) || (!edit && !app.ui.sculpt_core20_marker_placed_mix) {
        if workspace.layout.contains(&Panel::Memory) {
            let _ = workspace.apply(Action::Move { panel: Panel::Memory, anchor: Panel::Canvas, placement: Placement::Split(Zone::Right) });
        } else {
            workspace.hidden.retain(|(panel, _)| *panel != Panel::Memory);
        }
        if edit {
            app.ui.sculpt_core20_marker_placed = true;
        } else {
            app.ui.sculpt_core20_marker_placed_mix = true;
        }
    }
    // SCULPT_CORE22_LAYOUT_MIGRATION: old persisted sidebars should not survive first launch.
    if edit && !app.ui.sculpt_core22_layout_migrated {
        workspace = Workspace::default();
        app.ui.show_tracks_list = false;
        app.ui.show_clip_list = false;
        app.ui.show_midi_editor = false;
        app.ui.show_memory_locations = true;
        app.ui.sculpt_core22_layout_migrated = true;
    }
    // SCULPT_CORE23_RIGHT_DOCK_MIGRATION: a single intentional layout reset.
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
    let before_sync = workspace.clone();
    if let Err(error) = (if edit { workspace.validate() } else { workspace.validate_mix() }).and_then(|()| sync(&mut workspace, app, edit)) {
        app.ui.status = format!("Panel layout could not be restored: {error}");
        workspace = if edit { Workspace::default() } else { mix_default() };
        let _ = sync(&mut workspace, app, edit);
    }
    cancel_replaced_workspace(app, &before_sync, &workspace, edit);
    let mut style = DockStyle::from_ui(ui);
    style.min_pane = 96.0;
    style.tab_height = MIDI_TAB_HEIGHT;
    style.background = t.panel_bg;
    style.tab_background = t.panel_bg2;
    style.active_background = t.row_selected;
    style.text = t.text;
    style.inactive_text = t.text_dim;
    style.border = egui::Stroke::new(1.0, t.border);
    style.accent = t.accent;
    style.font = crate::theme::regular(11.5);
    let output = DockArea::new(egui::Id::new(("soundcraft-docking", edit))).show_with_limits(
        ui,
        &workspace.layout,
        &style,
        |panel| panel.label().into(),
        permissions,
        panel_limits,
        |ui, panel| match panel {
            Panel::Canvas => {
                ui.painter().rect_filled(ui.max_rect(), 0.0, t.window_bg);
                if edit {
                    crate::edit_window::main_area(app, ui);
                } else {
                    crate::mix_window::content(app, ui);
                }
            }
            Panel::Tracks => crate::panels::groups_and_selected_channel(app, ui), // SCULPT_CORE19_GROUP_CHANNEL_PANEL
            Panel::Clips => crate::panels::clip_list(app, ui),
            Panel::Midi => {
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("soundcraft-midi-body"), ui.max_rect()));
                minimum_body(ui, egui::vec2(420.0, 260.0), |ui| crate::midi_editor::show(app, ui));
            }
            Panel::Memory => crate::panels::memory_locations(app, ui),
            Panel::Groups => crate::panels::groups_only(app, ui), // SCULPT_CORE22_GROUP_RENDER
            Panel::History => crate::panels::undo_history(app, ui),
        },
    );
    let _ = sync(&mut workspace, app, edit);
    publish_visibility(app, &workspace, edit);
    store(app, workspace, edit);
    for action in output.actions {
        if let Err(error) = serde_json::to_value(action)
            .map_err(|error| error.to_string())
            .and_then(|action| command_for(app, false, &json!({"action": action}), edit))
        {
            app.ui.status = format!("Panel move was not applied: {error}");
        }
    }
}

/// Docking may make a specialist view smaller than its fixed tool/ruler geometry. Keep that
/// geometry scrollable inside its clipped panel instead of producing inverted rectangles.
pub(crate) fn minimum_body(ui: &mut egui::Ui, minimum: egui::Vec2, body: impl FnOnce(&mut egui::Ui)) {
    let available = ui.available_size();
    if available.x >= minimum.x && available.y >= minimum.y {
        body(ui);
        return;
    }
    let extent = available.max(minimum);
    egui::ScrollArea::both().id_salt("minimum-body").auto_shrink([false, false]).show(ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(extent, egui::Sense::hover());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        body(&mut child);
    });
}

fn panel_limits(panel: &Panel) -> PanelLimits {
    let (min, max_width) = match panel {
        Panel::Canvas => (egui::vec2(300.0, 240.0), 1_000_000.0),
        Panel::Tracks => (egui::vec2(140.0, 120.0), 520.0),
        Panel::Clips => (egui::vec2(180.0, 120.0), 720.0),
        Panel::Midi => (egui::vec2(420.0, 260.0), 1_000_000.0),
        Panel::Memory => (egui::vec2(220.0, 130.0), 470.0),
        Panel::Groups => (egui::vec2(220.0, 130.0), 470.0), // SCULPT_CORE22_GROUP_LIMIT
        Panel::History => (egui::vec2(200.0, 160.0), 720.0),
    };
    PanelLimits { min, max: egui::vec2(max_width, 1_000_000.0) }
}

#[cfg(test)]
mod sculpt_core18_tests {
    use super::*;
    #[test]
    fn sculpt_core18_memory_is_right_docked_by_default() {
        let mut w = Workspace::default();
        w.set_visible(Panel::Memory, true).unwrap();
        assert!(w.layout.contains(&Panel::Memory));
        assert!(!w.layout.floating.iter().any(|f| f.panels.contains(&Panel::Memory)));
    }
}
