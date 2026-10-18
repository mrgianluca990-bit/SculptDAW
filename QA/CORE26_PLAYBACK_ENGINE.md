# Sculpt DAW Core26 — Playback Engine

Baseline: Core25 `3a30d185e42f39af915b03371f0b02d7ad374a4c`.

Selected capability-driven configuration approach after inspecting SoundCraft
`2e9ad2f4215bd36e7edf30579e8ce520d5ce67c7`. This is not a full upstream merge.

- Playback Engine menu/window; cached device enumeration with explicit Refresh.
- Input selector, rate-specific hardware buffer options, requested/observed callback diagnostics.
- Strict named device, rate and buffer selection; no silent fallback to another device or rate.
- Input capture now honors the selected rate and buffer, using the session rate by default.
- Stream-start errors report failure rather than claiming an active output.
- New streams are tried before saving the existing SCULPT_AUDIO_V1 device profile format.
  The current Show source does not consume this profile; automatic device synchronization
  with Show is not included or claimed.
- Reconnection is rejected while playing/recording; plugin states and transport position are retained.
  Native plugin editor windows are closed on the UI thread before replacing the player.
- I/O retains existing physical output assignment, adjacent stereo paths and 64-channel limits.

Bridge renderer, publication protocol, document model and single-project behavior are unchanged.
Existing bridge audio/receiver regressions remain mandatory, alongside capability/transaction tests.

This phase does not implement offline project-rate migration, eliminate all playback SRC,
add a full Pro Tools I/O matrix or merge all upstream features. A different explicitly selected
hardware rate still uses the existing resampler and is shown as such. Default output selection
now fails visibly if the session rate is unsupported rather than choosing a different rate.

Manual acceptance: real interface input/output at 44.1/48/96 kHz and supported buffers;
monitoring, recording, VST3 state across reconnect; missing device and unsupported settings;
Show → DAW → Show with mono/stereo stems, markers, duplicate track names and reordered tracks.
Automatic checks and archive verification must be recorded separately from audible Mac acceptance.
