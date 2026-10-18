# Sculpt Development Infrastructure — Bridge milestone 1

Two products remain independent: Swift/CoreAudio Show and Rust DAW. Each repository
uses `feature/*` → `develop` → `main`; macOS compilation and regression tests run
on PRs and branch pushes. A green build is required before manual promotion.
Do not regenerate Rust code or replay historical source patches during builds.
`QA/*Patch.py` is historical migration material, not an active build dependency.

## First acceptance workflow

1. In Show select a stereo or multitrack cue and choose **Edit in Sculpt DAW**.
2. CLI embedded in DAW.app creates a native `.scdaw` with aligned tracks,
   physical output metadata, markers and `sculpt-cue-bridge.json` beside it.
3. Edit regions/fades, insert plugins, change levels and markers, then save DAW.
4. File → Publish in Show → **Publish in Show** updates the original cue.
   **Publish in Show as a New Cue** leaves the original and adds a new cue.
5. A standalone DAW session uses **Publish in Show as a New Cue**, targeting
   Show's selected cue list. Import from DAW accepts both `.scdaw` and `.scraft`;
   only an unprocessed, aligned native session can be imported without a render.
6. Each publication is a fresh directory with `Audio/` and `publication.sculptbridge`.
   macOS delivers that document to the existing Show. If unavailable, open Show's
   **Receive DAW Publication…** command and select it manually.
7. Show validates media and destination IDs, checks the original cue revision,
   prepares waveforms, then commits one undoable change. A playing/paused/fading/
   waiting destination cue is queued until fully stopped; other playback continues.
8. `receipt.json` means applied **in memory**, not saved. Save Show. DAW's status
   only says delivered, never assumes application. Reopen the cue from Show after
   undoing an applied publication or reopening an earlier saved Show revision.

## Contract and scope

`bridge/publication-v1.schema.json` is shared verbatim. Legacy outbound manifests
remain v1; new returns add publicationID, publishMode and originCue (optimistic
concurrency snapshot). Unknown versions/modes are rejected. Updates retain IDs,
cue-only controls, LTC and existing marker rules; new cues get fresh IDs and no
copied event rules. Every packet uses new media files; source audio is never replaced.
Repeated delivery of an acknowledged packet is ignored. Conflicting Show media or
marker edits require reopening from Show or publishing a new cue.

One rendered file per active mono/stereo audio/instrument track, aligned from 0
through content end; edits, fades, inserts and track gain automation are baked.
Manual mute remains a cue-element flag. Filenames use track IDs, so duplicate names
cannot overwrite each other. Mono keeps one hardware channel, including OUT 64.
Missing/invalid loaded audio and unavailable required plugin/instrument instances
abort publication instead of silently exporting an incomplete mix. Bypassed plugins
do not prevent publication.

Shared buses/sends and processed master/VCA chains cannot be represented as independent
track stems by this renderer and are explicitly rejected. Consolidate those returns
before publishing. Offline render allocation limit is checked before rendering;
longer material is rejected rather than truncated. Saving Show copies published
audio into its portable project. Byte-identical media relocation preserves the
bridge revision; changed audio still conflicts. Retain the DAW session and its
publication folders/receipts for subsequent edits and acknowledgements.

## Validation status

Unit tests exercise routing/identity, stale updates, malformed/unsafe packets,
mono/stereo audio, duplicate names, muted stems, atomic model updates and new cues.
macOS app/CLI build and existing regression tests run in GitHub Actions. A real
Mac audio device + UI round-trip remains a separate user acceptance gate; a green
compiler does not prove hardware playback or third-party plugin fidelity.

## Development rules

- Work on a feature branch; never replace main with an untested source overlay.
- Change the protocol additively within v1; breaking changes require a new version.
- Keep audio engine, document model, bridge and UI responsibilities separate.
- Never interrupt playback to receive a publication; do not call stopAll/openShow.
- Compile and test the exact source tree that will be packaged, both architectures.
- PR template records observed results and remaining manual checks. Release only
  from an explicitly reviewed green main commit/tag; no automatic public release.
