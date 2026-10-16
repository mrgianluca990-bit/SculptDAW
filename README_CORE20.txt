SCULPT DAW — CORE 20 CANDIDATE (source patch for existing Core19 workflow)
16 October 2026

SCOPE — DAW ONLY. No Sculpt Show files are changed.

INSTALL ON GITHUB
  1. Extract this ZIP.
  2. Copy the entire sculpt/ and QA/ directories into repository
     https://github.com/mrgianluca990-bit/SculptDAW (retain folder paths).
  3. Commit the two source files, replacing sculpt/build-macos.sh and adding
     QA/SculptDAW_Core20_Patch.py.
  4. The EXISTING Core19 workflow on main runs after the push. Its old patch
     sequence runs first; build-macos.sh then applies the Core20 overlay,
     cargo-checks it and builds the Core20 candidate.
  5. Download the new GitHub Actions artifact. The filename remains
     Sculpt_DAW_0.4.0_Mac_Universal.zip for compatibility.

FEATURES IN CANDIDATE, PENDING MAC HARDWARE ACCEPTANCE
  - Finder/Open With: process Cocoa odoc events and recognize .scdaw as sessions.
  - Markers / Memory Locations: move to right ONCE, keep subsequent user drags.
  - Persistent Edit selected-channel strip including a visible vertical fader,
    pan, I/O and inserts/sends overview; no change to existing Mix view.
  - soundcraft-cli: built into Contents/Helpers/ and signed before the app.
    It is NOT a process that should automatically run at DAW startup.
  - DAW bridge handshake: `Sculpt DAW.app/Contents/MacOS/sculpt-daw
    --sculpt-bridge-capabilities` outputs a versioned JSON capability document.
    The existing `--control PORT` command channel remains opt-in.

IMPORTANT: This is a SOURCE-LEVEL CANDIDATE, NOT A COMPILED APP.
No Apple Silicon or Intel run has been confirmed. An incompatible Core19 source
revision fails during the patch or cargo check; the workflow must succeed
before distributing the resulting application.

OPEN / UNCONFIRMED ISSUES
  - FabFilter VST3 meters and animation: cannot claim solved. SoundCraft
    VST3 host implements IPlugView and IPlugFrame but no IRunLoop, and
    AudioUnit native editor support is not yet present. Next step is an
    isolated VST3 host fix rather than modifying audio DSP blindly.
  - AUX/master meters: track is measured before pan/routing; downstream levels
    may legitimately differ with pan law -3 dB and stereo format. Calibrate
    on a known mono 1kHz signal, same faders and no inserts, then determine if
    there is a real UI or DSP bug before altering calibrated meters.

NON-REGRESSION ACCEPTANCE
  1. Open a saved .scdaw by double-clicking in Finder. Try when app closed
     AND already running. Test Open With > Sculpt DAW.
  2. Marker panel appears on right; can still manually move/dock it.
  3. Fader in Edit changes same channel as Mix and persists on save/reopen.
  4. Groups, track reorder and plugin drag/drop still work.
  5. App ZIP has NO standalone soundcraft-cli; inside app:
     Contents/Helpers/soundcraft-cli. Mac universal arm64+x86_64.
  6. CLI `--version`; bridge `--sculpt-bridge-capabilities` prints JSON.
  7. FabFilter meters/animation, 1kHz calibration of track>AUX>master:
     report observation, not assumed regression.
  8. Show's existing two-stem cue open after any future Show-side changes.
     DO NOT update Show as part of this candidate.
