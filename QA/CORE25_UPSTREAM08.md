# Sculpt DAW Core25 — single workspace, Edit strip, SoundCraft backports

18 ottobre 2026. Baseline: bridge integration `057c6e6` with Core24 features.
This is a selective update from SoundCraft 0.8.0, not a complete core migration.
The full merge was inspected: editing, groups, fades, routing and publication need additional compatibility work before a complete 0.8 migration is safe.

Included upstream corrections (MIT OR Apache-2.0):
- VST3 host-set controller parameters survive state save/restore (upstream #400).
- Master insert lookahead is included in offline delay compensation (upstream #409).

Sculpt corrections:
- macOS process lock and document forwarding reuse one running workspace before starting audio.
- Existing unsaved-project confirmation remains in the normal document-open flow.
- Edit selected-channel strip reserves its fader and readouts after every visible section, including eight inserts/sends; it scrolls inside its pane.
- Show bridge, native .scdaw projects, routing, groups, fades and plugin hosting remain based on the committed integration sources.

Validation: CI must pass the complete workspace gate, the two upstream regressions, single-instance forwarding and Edit strip geometry. Actual Core Audio devices and audible playback still require the Mac acceptance test.
