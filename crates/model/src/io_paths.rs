use crate::{OutputPath, Route, Session};

impl Session {
    pub fn needs_hardware_input(&self) -> bool {
        self.tracks.iter().any(|t| {
            !t.inactive
                && matches!(t.mixer.input, Route::Hardware(_))
                && (t.kind == crate::TrackKind::Aux || (t.kind == crate::TrackKind::Audio && (t.mixer.input_monitor || t.mixer.record_arm)))
        })
    }
    pub fn output_path(&self, key: &str) -> Option<&OutputPath> {
        find(&self.outputs, key, false)
    }
    pub fn input_path(&self, key: &str) -> Option<&OutputPath> {
        find(&self.inputs, key, true)
    }

    /// Resolve a hardware input without wrapping an unavailable channel back to input one.
    /// Names remain accepted for legacy sessions; new assignments use stable route keys.
    pub fn input_channel(&self, route: &Route, offset: usize, available: usize) -> Option<usize> {
        let Route::Hardware(key) = route else { return None };
        let first = if let Some(path) = self.input_path(key) {
            if !path.available(available) || offset >= path.format.channels() {
                return None;
            }
            usize::from(path.first_channel)
        } else {
            let raw = key.strip_prefix("In ")?;
            let number = raw.split(['-', ' ']).next()?.parse::<usize>().ok()?;
            number.checked_sub(1)?
        };
        first.checked_add(offset).filter(|channel| *channel < available.min(64))
    }

    pub(crate) fn sanitize_io_paths(&mut self) {
        for path in &mut self.outputs {
            if path.name.starts_with("SCULPT HW") {
                path.monitor_on_main = true;
            }
        }
        for paths in [&mut self.inputs, &mut self.outputs] {
            let mut next = paths.iter().map(|p| p.id).filter(|id| *id < 1_000_000_000).max().unwrap_or(0).saturating_add(1);
            let mut used = std::collections::HashSet::new();
            for path in paths.iter_mut() {
                if path.id == 0 || path.id >= 1_000_000_000 || !used.insert(path.id) {
                    path.id = next;
                    next = next.saturating_add(1);
                    used.insert(path.id);
                }
                if usize::from(path.first_channel).saturating_add(path.format.channels()) > 64 {
                    path.active = false;
                }
            }
        }
    }
}

fn find<'a>(paths: &'a [OutputPath], key: &str, input: bool) -> Option<&'a OutputPath> {
    let prefix = if input { "sculpt.io.in:" } else { "sculpt.io.out:" };
    if let Some(id) = key.strip_prefix(prefix) {
        let id = id.parse::<u64>().ok()?;
        paths.iter().find(|p| p.id == id && id != 0)
    } else {
        paths.iter().find(|p| p.name.eq_ignore_ascii_case(key))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::ChannelFormat;
    #[test]
    fn renamed_input_keeps_its_identity_and_unavailable_channels_are_silent() {
        let mut s = Session::default();
        s.inputs.push(OutputPath {
            id: 7,
            active: true,
            monitor_on_main: false,
            name: "Microphone".into(),
            first_channel: 2,
            format: ChannelFormat::Mono,
        });
        let route = Route::Hardware(s.inputs[0].route_key(true));
        s.inputs[0].name = "Voice".into();
        assert_eq!(s.input_channel(&route, 0, 4), Some(2));
        assert_eq!(s.input_channel(&route, 0, 2), None);
        assert_eq!(s.input_channel(&route, 1, 4), None);
        assert_eq!(s.input_channel(&Route::None, 0, 4), None);
        assert_eq!(s.input_channel(&Route::Hardware("In 65".into()), 0, 64), None);
        s.inputs[0].active = false;
        assert_eq!(s.input_channel(&route, 0, 4), None);
    }
    #[test]
    fn legacy_files_gain_path_ids_without_changing_track_ids_or_losing_preview() {
        let mut s = Session::default();
        let track = s.add_track(crate::TrackKind::Audio, ChannelFormat::Mono, None);
        let mut legacy = serde_json::to_value(&s).unwrap();
        legacy.as_object_mut().unwrap().remove("inputs");
        let output = legacy["outputs"][0].as_object_mut().unwrap();
        output.remove("id");
        output.remove("active");
        output.remove("monitor_on_main");
        output.insert("name".into(), serde_json::json!("SCULPT HW STEREO 05-06"));
        let loaded = Session::from_json(&legacy.to_string()).unwrap();
        assert_eq!(loaded.tracks[0].id, track);
        assert!(loaded.outputs[0].id > 0 && loaded.outputs[0].active && loaded.outputs[0].monitor_on_main);
    }
}
