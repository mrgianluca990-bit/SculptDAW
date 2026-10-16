SCULPT DAW — CORE 21 COMBINED BUILD PATCH / 16 OCT 2026
================================================================
SOURCE UPDATE FOR YOUR EXISTING GITHUB ACTIONS BUILD.
THIS IS NOT A COMPILED OR MAC-TESTED APPLICATION.

EXACT CAUSE OF THE LATEST FAILED BUILD (OCTOBER 16)
GitHub Actions run 38909007749, job 116999789930, failed at
"Build Sculpt DAW Mac app" after the baseline Core19 Rust tests passed.
Error from QA/SculptDAW_Core20_Patch.py, line 313:
  RuntimeError: crates/ui-egui/src/docking.rs:
  could not identify exactly one Memory default side: 0

The previous fix still attempted to text-match `Self::Memory => Zone::Bottom/Right`
after Core19 restored the layout. That check was unnecessary and brittle.
It has NOW BEEN REMOVED ENTIRELY. The marker migration already moves the
panel to the right once per window, without touching the Core19 match arm
or the user's layout thereafter. The expected verification marker is
placed inside this one-time migration and is no longer tied to a Rust
source pattern. This is an overlay-script failure, not a Rust compiler
error. Core21 did not execute in the failed job and its Rust remains unverified.

Additional failures prevented in this combined candidate:
* Core21 playback source has TWO AudioState::new(...) call sites.
  The native CoreAudio output-stream instance is now anchored uniquely.
* Core21 assignment UI lacked its own post-patch verification marker;
  added one, so the source guards now complete successfully.
* Intermediate routing-enum replacements were not idempotent;
  consolidated into one serialization-safe check.
* `track.output` required a selected track even when a caller provided
  explicit track IDs. Its precondition now permits restoring Show routes
  into a session without any UI track selection.
* Auto-migrated Show tracks retain their stem IDs/paths but mark migration
  completed, so a later deliberate operator return-to-Main persists.

UPDATE YOUR PUBLIC REPOSITORY WITH A SINGLE COMMIT
  https://github.com/mrgianluca990-bit/SculptDAW
Overwrite / add exactly these paths from the archive:
  QA/SculptDAW_Core20_Patch.py                 (replace existing)
  QA/SculptDAW_Core21_IO_Patch.py              (new)
  sculpt/build-macos.sh                        (replace existing)

Do not replace any existing Core19/18/17 Rust source files, nor modify
Sculpt Show. Commit all three paths in ONE commit: the existing GitHub
workflow will apply Core19, then the corrected Core20, then Core21, and
compile only the resulting combined candidate.

IMPORTANT: The workflow's existing artifact LABEL may still be
"Sculpt-DAW-Core19-Mac-Universal" because the workflow itself has
not been renamed. It will contain the new Core21-patched candidate
only if its run corresponds to your new commit and every step is green.
The inner package filename remains Sculpt_DAW_0.4.0_Mac_Universal.zip
for compatibility with the existing workflow's ZIP checks.

CHANGES INCLUDED
Core20:
  - Finder / Open With .scdaw handler
  - Marker/Memory one-time migration to RIGHT (Core19 default unchanged)
  - Visible selected channel strip and fader in Edit view
  - Embedded signed CLI in Sculpt DAW.app/Contents/Helpers/
  - Sculpt DAW bridge protocol/capability handshake
Core21:
  - Setup > I/O: 64-output patch matrix and mono/stereo paths
  - Mix output chooser exposes named hardware paths
  - Real Route::Hardware mix path to device channels 1..64
  - Output allocation/negotiation with the default CoreAudio device
  - Fallback/Show preview on Main when outputs are unavailable
  - Named output assignments and Show import metadata restoration
  - Versioned DAW bridge's hardware capabilities
  - DSP regression test on output channel 64 and Main fallback

NON-REGRESSION BASELINE
The Core19 Groups, track reorder, insert drag/drop, project saving,
plugin host and existing Main/Bus sends are NOT intentionally replaced.

LIMITATIONS
* A device cannot provide 64 physical outputs unless CoreAudio
  exposes them. This candidate opens the default system output; select
  the multichannel interface as the default device before testing.
  If changing device/routing from within the app, reconnect through
  Playback Engine / I/O Setup. An unavailable path remains saved and
  falls back to Main. Output overlap warnings are informational.
* No live bidirectional Show round-trip is yet implemented: this is
  the DAW-side hardware bridge and existing Show import restoration.
* FabFilter native meter/animation hosting and AUX/master meter
  calibration remain separate unresolved issues. Do NOT treat this
  release as fixing either problem without focused Mac tests.
* No real Apple Silicon/Intel build, plugin-GUI run, or 64ch hardware
  audio test could be performed here. Trust a green CI run only for
  compilation/tests; then do your hardware acceptance test on Mac.

VALIDATION PERFORMED IN THIS PACKAGE
  - Both overlay Python scripts pass AST/py_compile.
  - Bash package script passes bash -n.
  - Marker layout handling no longer depends on a specific match-arm
    syntax; static guards verify its tag occurs in the migration.
    This is NOT a substitute for compilation of Rust code.
  - The new build script runs a post-overlay cargo check and the
    channel-64 DSP test before expensive release compilation.
  - The build script checks embedded CLI signature and the single-app
    ZIP layout after packaging.

ON YOUR MAC, AFTER A SUCCESSFUL BUILD
  1. Finder: double-click .scdaw with Sculpt closed AND already open.
  2. Marker panel at RIGHT in Edit and Mix; fader visible in Edit.
  3. Groups, track reorder, insert DnD; reopen saved project.
  4. Open Setup > I/O and configure SCULPT HW MONO 64, stereo 63-64.
  5. Play mono/stereo test material routed to these channels via a
     verified 64ch CoreAudio interface, and confirm signals on outputs.
  6. Use a 2ch interface with routing 63-64 saved: no silence, warning,
     preserved route, audible Main monitor.
  7. Open a Show-imported session; inspect named per-stem output paths.
  8. Make an intentional manual route change, save and reopen: it stays.
  9. Inspect FabFilter GUI and AUX/master meter separately (unresolved).
