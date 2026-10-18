#!/usr/bin/env python3
"""SCULPT CORE25: selected, guarded SoundCraft v0.7-v0.8 backports.

Upstream credits: storytold/soundcraft (MIT OR Apache-2.0), PR #400 and #409.
Runs after Core24 against the pinned Sculpt base; this is NOT a wholesale v0.8 rebase.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def patch(relative, old, new, tag):
    file = ROOT / relative
    source = file.read_text(encoding="utf-8")
    if tag in source:
        print("ALREADY", tag)
        return
    count = source.count(old)
    if count != 1:
        raise RuntimeError(f"{relative}: {tag} expected one anchor, got {count}")
    file.write_text(source.replace(old, new, 1), encoding="utf-8")
    print("PATCH", tag)

patch("crates/vst3-host/src/plugin.rs", "    pending: Vec<(u32, f64)>,\n", "    pending: Vec<(u32, f64)>,\n    /// Host parameters pending synchronization with the VST3 controller.\n    unsynced: Vec<bool>, // SCULPT_CORE25_VST3_FLAGS\n", "SCULPT_CORE25_VST3_FLAGS")
patch("crates/vst3-host/src/plugin.rs", "            pending: Vec::with_capacity(ids.len().min(PARAM_QUEUE_CAP)),\n", "            pending: Vec::with_capacity(ids.len().min(PARAM_QUEUE_CAP)),\n            unsynced: vec![false; ids.len()], // SCULPT_CORE25_VST3_ALLOC\n", "SCULPT_CORE25_VST3_ALLOC")
patch("crates/vst3-host/src/plugin.rs", "        self.pending.clear();\n    }\n\n    /// Inserts keeping the queue time-sorted;", "        self.pending.clear();\n        self.unsynced.fill(false); // SCULPT_CORE25_VST3_RESET\n    }\n\n    /// Inserts keeping the queue time-sorted;", "SCULPT_CORE25_VST3_RESET")
patch("crates/vst3-host/src/plugin.rs", "        if let Some(slot) = self.values.get_mut(i) {\n            *slot = v;\n        }\n        let Some(n) = self.normalized_from_plain(i, v) else { return true };", "        if let Some(slot) = self.values.get_mut(i) {\n            *slot = v;\n        }\n        if let Some(unsynced) = self.unsynced.get_mut(i) {\n            *unsynced = true; // SCULPT_CORE25_VST3_SET\n        }\n        let Some(n) = self.normalized_from_plain(i, v) else { return true };", "SCULPT_CORE25_VST3_SET")
patch("crates/vst3-host/src/plugin.rs", "    fn save_state(&mut self) -> Option<Vec<u8>> {\n        self.inst.save_state()\n    }", "    fn save_state(&mut self) -> Option<Vec<u8>> {\n        // SCULPT_CORE25_VST3_SAVE: controller-only parameters must survive a session save.\n        // set_param can run on the real-time thread; only do COM work during save_state.\n        for i in 0..self.ids.len() {\n            if !self.unsynced.get(i).copied().unwrap_or(false) {\n                continue;\n            }\n            if let (Some(&id), Some(n)) = (\n                self.ids.get(i),\n                self.values.get(i).and_then(|&v| self.normalized_from_plain(i, v)),\n            ) {\n                self.inst.set_normalized(id, n);\n            }\n        }\n        self.unsynced.fill(false);\n        self.inst.save_state()\n    }", "SCULPT_CORE25_VST3_SAVE")
patch("crates/vst3-host/src/ffi.rs", "    pub fn value_text(&self, id: u32, normalized: f64) -> Option<String> {", "    /// SCULPT_CORE25_VST3_CONTROLLER: sync a host-set value to IEditController.\n    pub fn set_normalized(&self, id: u32, normalized: f64) {\n        let Some(controller) = &self.controller else { return };\n        if normalized.is_finite() {\n            // SAFETY: controller was validated during instance creation.\n            unsafe { controller.setParamNormalized(id, normalized.clamp(0.0, 1.0)) };\n        }\n    }\n\n    pub fn value_text(&self, id: u32, normalized: f64) -> Option<String> {", "SCULPT_CORE25_VST3_CONTROLLER")
patch("crates/mix/src/lib.rs", "        let lmax_t = if pdc { work.iter().map(|(_, st)| st.latency).max().unwrap_or(0) } else { 0 };", "        // SCULPT_CORE25_MASTER_LATENCY: master insert lookahead delays the entire main bus.\n        let mut lmax_master = 0usize;\n        if pdc {\n            for t in s.tracks.iter().filter(|t| t.kind == TrackKind::Master && !t.inactive) {\n                if let Some(st) = self.strips.get_mut(&t.id) {\n                    note_insert_latency(t, st, pos);\n                    lmax_master = lmax_master.saturating_add(st.latency);\n                }\n            }\n        }\n        let lmax_t = if pdc { work.iter().map(|(_, st)| st.latency).max().unwrap_or(0) } else { 0 };", "SCULPT_CORE25_MASTER_LATENCY")
patch("crates/mix/src/lib.rs", "        self.latency = lmax_t + lmax_a;", "        self.latency = lmax_t.saturating_add(lmax_a).saturating_add(lmax_master); // SCULPT_CORE25_MASTER_TOTAL", "SCULPT_CORE25_MASTER_TOTAL")
patch("crates/vst3-host/tests/fixture_e2e.rs", "#[test]\nfn editor_view_is_probed_and_edits_come_back_as_plain_values() {", "// SCULPT_CORE25_VST3_REGRESSION: adapted from SoundCraft PR #400.\n#[test]\nfn host_set_controller_param_survives_state_round_trip() {\n    let id = id_of(\"Fixture Gain\");\n    let mut p = soundcraft_vst3_host::create(&id).unwrap();\n    p.prepare(48_000.0, 64, 2);\n    assert!(p.set_param(\"7\", 1.5));\n    let mut block = vec![vec![0.0f32; 64]; 2];\n    p.process(&mut block, 64);\n    assert!(p.set_param(\"12\", 200.0));\n    let blob = p.save_state().expect(\"plugin state\");\n    let mut q = soundcraft_vst3_host::create(&id).unwrap();\n    assert!(q.load_state(&blob));\n    assert!((q.param(\"7\").unwrap() - 1.5).abs() < 1e-4);\n    assert!((q.param(\"12\").unwrap() - 200.0).abs() < 0.01);\n}\n\n#[test]\nfn editor_view_is_probed_and_edits_come_back_as_plain_values() {", "SCULPT_CORE25_VST3_REGRESSION")
# Keep the master lookahead regression in the actual Rust test tree.
test_source = ROOT / "QA/SculptDAW_Core25_MasterLatency_Test.rs"
test_output = ROOT / "crates/mix/tests/master_insert_latency.rs"
expected = test_source.read_bytes()
if test_output.exists():
    if test_output.read_bytes() != expected:
        raise RuntimeError("Existing master_insert_latency.rs differs from the pinned upstream fixture")
else:
    test_output.parent.mkdir(parents=True, exist_ok=True)
    test_output.write_bytes(expected)
    print("PATCH SCULPT_CORE25_MASTER_TEST")

for relative, tag in [
    ("crates/vst3-host/src/plugin.rs", "SCULPT_CORE25_VST3_SAVE"),
    ("crates/vst3-host/src/ffi.rs", "SCULPT_CORE25_VST3_CONTROLLER"),
    ("crates/vst3-host/tests/fixture_e2e.rs", "SCULPT_CORE25_VST3_REGRESSION"),
    ("crates/mix/src/lib.rs", "SCULPT_CORE25_MASTER_TOTAL"),
]:
    if tag not in (ROOT / relative).read_text(encoding="utf-8"):
        raise RuntimeError(f"Missing patch tag {tag}")
print("SCULPT CORE25: two upstream fixes and two regressions staged")
