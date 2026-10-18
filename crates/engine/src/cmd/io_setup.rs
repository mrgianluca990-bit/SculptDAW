//! Atomic, undoable I/O editing; hardware paths have identities independent of their labels.
use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Bus, OutputPath, Route, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "setup.io.apply",
        "Apply I/O Setup",
        [],
        None,
        "{expected: {inputs, outputs, busses}, inputs: [paths], outputs: [paths], busses: [buses]} — atomic I/O edit; zero IDs allocate new identities",
        always,
        apply
    )]
}

pub(crate) fn state(s: &Session) -> Value {
    json!({"inputs":s.inputs,"outputs":s.outputs,"busses":s.busses})
}

fn name_ok(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(char::is_control) && !name.starts_with("sculpt.io.")
}
fn paths(p: &Value, key: &str) -> Result<Vec<OutputPath>> {
    let list = p
        .get(key)
        .and_then(Value::as_array)
        .filter(|v| v.len() <= 128)
        .ok_or_else(|| bad("setup.io.apply", format!("{key} must contain at most 128 paths")))?;
    let mut paths: Vec<OutputPath> = serde_json::from_value(json!(list)).map_err(|e| bad("setup.io.apply", e.to_string()))?;
    let mut next = paths.iter().map(|p| p.id).max().unwrap_or(0).saturating_add(1);
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for path in &mut paths {
        if path.id == 0 {
            path.id = next;
            next = next.saturating_add(1);
        }
        if path.id >= 1_000_000_000
            || !ids.insert(path.id)
            || !name_ok(&path.name)
            || !names.insert(path.name.to_lowercase())
            || usize::from(path.first_channel).saturating_add(path.format.channels()) > 64
            || path.format.channels() > 16
        {
            return Err(bad("setup.io.apply", format!("Invalid or duplicate {key} path: {}", path.name)));
        }
    }
    Ok(paths)
}

fn rewrite(key: &str, before: &[OutputPath], after: &[OutputPath], input: bool) -> Option<String> {
    let prefix = if input { "sculpt.io.in:" } else { "sculpt.io.out:" };
    let old = key
        .strip_prefix(prefix)
        .and_then(|id| id.parse::<u64>().ok())
        .and_then(|id| before.iter().find(|p| p.id == id))
        .or_else(|| before.iter().find(|p| p.name.eq_ignore_ascii_case(key)));
    if let Some(old) = old {
        after.iter().find(|p| (old.id != 0 && p.id == old.id) || (old.id == 0 && p.name == old.name)).map(|p| p.route_key(input))
    } else {
        Some(key.into())
    }
}

fn apply(e: &mut Engine, p: &Value) -> Result<Value> {
    if e.transport.recording {
        return Err(bad("setup.io.apply", "Stop recording before changing I/O"));
    }
    if p.get("expected") != Some(&state(e.session())) {
        return Err(bad("setup.io.apply", "I/O changed while this window was open. Revert and edit the current configuration."));
    }
    let inputs = paths(p, "inputs")?;
    let outputs = paths(p, "outputs")?;
    if outputs.first().is_none_or(|p| !(2..=16).contains(&p.format.channels())) {
        return Err(bad("setup.io.apply", "Keep a primary output path with at least two channels"));
    }
    if let Some(old) = e.session().outputs.first().filter(|p| p.id != 0)
        && outputs.first().is_none_or(|p| p.id != old.id)
    {
        return Err(bad("setup.io.apply", "The primary output path cannot be deleted or replaced"));
    }
    let list = p
        .get("busses")
        .and_then(Value::as_array)
        .filter(|v| v.len() <= 256)
        .ok_or_else(|| bad("setup.io.apply", "At most 256 buses are supported"))?;
    let mut busses: Vec<Bus> = serde_json::from_value(json!(list)).map_err(|error| bad("setup.io.apply", error.to_string()))?;
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for bus in &busses {
        let lower = bus.name.to_ascii_lowercase();
        if !name_ok(&bus.name)
            || !names.insert(lower.clone())
            || (bus.id.0 != 0 && !ids.insert(bus.id))
            || matches!(lower.as_str(), "main" | "out 1-2" | "master" | "none" | "no input" | "no output")
        {
            return Err(bad("setup.io.apply", "Bus names and identities must be unique; reserved routing names cannot be used"));
        }
        if let Some(target) = &bus.output {
            let target_id = target.strip_prefix("sculpt.io.out:").and_then(|id| id.parse::<u64>().ok());
            if !outputs.iter().any(|o| target_id.is_some_and(|id| o.id == id) || o.name == *target) {
                return Err(bad("setup.io.apply", "A mapped bus must refer to an existing output path"));
            }
        }
    }
    let s = e.session_mut();
    // New buses are allocated by the engine, avoiding collisions with tracks added during editing.
    for bus in &mut busses {
        if !s.busses.iter().any(|old| old.id == bus.id) {
            bus.id = soundcraft_model::BusId(s.alloc());
        }
        if let Some(target) = &bus.output {
            let target_id = target.strip_prefix("sculpt.io.out:").and_then(|id| id.parse::<u64>().ok());
            if let Some(path) = outputs.iter().find(|o| target_id.is_some_and(|id| o.id == id) || o.name == *target) {
                bus.output = Some(path.route_key(false));
            }
        }
    }
    for track in &mut s.tracks {
        for (route, before, after, input) in
            [(&mut track.mixer.input, &s.inputs, &inputs, true), (&mut track.mixer.output, &s.outputs, &outputs, false)]
        {
            match route {
                Route::Hardware(key) => {
                    *route = rewrite(key, before, after, input).map_or(Route::None, Route::Hardware);
                }
                Route::Bus(id) if !busses.iter().any(|b| b.id == *id) => {
                    *route = Route::None;
                }
                _ => (),
            }
        }
        for send in &mut track.mixer.sends {
            if send.as_ref().is_some_and(|send| matches!(send.target, Route::Bus(id) if !busses.iter().any(|b| b.id == id))) {
                *send = None;
            }
        }
    }
    s.inputs = inputs;
    s.outputs = outputs;
    s.busses = busses;
    Ok(state(s))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};
    #[test]
    fn named_inputs_remap_and_round_trip_without_changing_track_identity() {
        let mut e = Engine::default();
        let track = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Voice"));
        let original = state(e.session());
        let mut request = original.clone();
        request["expected"] = original;
        request["inputs"] = json!([{"id":9,"active":true,"name":"Microphone","first_channel":2,"format":"Mono"}]);
        // Use the model's actual serialized format rather than relying on a spelling.
        request["inputs"][0]["format"] = serde_json::to_value(ChannelFormat::Mono).unwrap();
        e.execute("setup.io.apply", &request).unwrap();
        e.execute("track.input", &json!({"track":track.0,"input":"sculpt.io.in:9","create_bus":false})).unwrap();
        let mut edit = state(e.session());
        edit["expected"] = edit.clone();
        edit["inputs"][0]["name"] = json!("Vocal");
        edit["inputs"][0]["first_channel"] = json!(3);
        e.execute("setup.io.apply", &edit).unwrap();
        let route = e.session().track(track).unwrap().mixer.input.clone();
        assert_eq!(e.session().input_channel(&route, 0, 8), Some(3));
        let reopened = Session::from_json(&e.session().to_json().unwrap()).unwrap();
        assert_eq!(reopened.input_channel(&reopened.track(track).unwrap().mixer.input, 0, 8), Some(3));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().input_channel(&route, 0, 8), Some(2));
        assert!(e.execute("setup.io.apply", &edit).is_ok());
        assert!(e.execute("setup.io.apply", &edit).is_err(), "a stale draft must not overwrite newer changes");
    }
    #[test]
    fn invalid_or_recording_io_edits_are_atomic_and_deleted_paths_disconnect() {
        let mut e = Engine::default();
        let original = state(e.session());
        let mut request = original.clone();
        request["expected"] = original.clone();
        request["inputs"] = json!([{"id":2,"active":true,"name":"Invalid","first_channel":63,"format":"Stereo"}]);
        assert!(e.execute("setup.io.apply", &request).is_err());
        assert_eq!(state(e.session()), original);
        request["inputs"][0]["first_channel"] = json!(2);
        e.transport.recording = true;
        assert!(e.execute("setup.io.apply", &request).is_err());
        assert_eq!(state(e.session()), original);
        e.transport.recording = false;
        e.execute("setup.io.apply", &request).unwrap();
        let track = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Voice"));
        e.execute("track.input", &json!({"track":track.0,"input":"sculpt.io.in:2","create_bus":false})).unwrap();
        let mut remove = state(e.session());
        remove["expected"] = remove.clone();
        remove["inputs"] = json!([]);
        e.execute("setup.io.apply", &remove).unwrap();
        assert_eq!(e.session().track(track).unwrap().mixer.input, Route::None);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().input_channel(&e.session().track(track).unwrap().mixer.input, 0, 4), Some(2));
    }
}
