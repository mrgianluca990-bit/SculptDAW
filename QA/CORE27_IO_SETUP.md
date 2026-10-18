# Sculpt DAW Core27 — I/O Setup

19 October 2026. Baseline: Core26 `e6333abe6a36c3fe106b56f1c941d3f61e99c1c4`.

The I/O Setup window now has Input, Output and Bus pages, named mono/stereo paths,
hardware-channel matrices, output-bus mapping, active/unavailable states, New Path,
Delete Path and Default. Apply commits one undoable edit; Cancel discards the draft.
A stale draft cannot overwrite I/O changed elsewhere, and recording blocks Apply.

Input and output identities are scoped to their respective namespaces. Hardware
route keys store the identity, while the displayed name can change freely. Existing
name-based sessions remain readable; their paths acquire IDs when loaded. Existing
track, marker, source and bus IDs do not change. The native document format remains
version 1 with additive fields. Use the updated DAW for projects using output-bus
mapping; older DAW versions do not implement that feature.

Named hardware inputs drive both input monitoring and take-file channel selection.
Unavailable paths are silent, never wrapped onto input one. Recording is refused
if any armed track lacks a complete available hardware input. No Input now means
silence instead of secretly reading input one. Hardware inputs can also feed aux
tracks. Internal buses continue to feed auxes, sends and track outputs; an optional
output mapping makes a bus audible directly on a physical output path.

The primary output remains the main/master mix and can be renamed or remapped,
but cannot be deleted or made mono. Its hardware mapping does not move channels
inside an offline bounce. New stable hardware outputs stay silent when unavailable;
legacy Show monitor fallback remains compatible. The legacy Show preview flag is
retained independently of a path's label so renaming does not change preview audio.

## Bridge boundary

Publication schema, stem ordering, mono/stereo files, marker identities, optimistic
revision checks and one-project behavior are preserved. Publication resolves stable
output identities, including renamed mono output 64. SHOW retains its own cyclic
assignment of incoming stems; this matrix does not replicate SHOW routing.

Shared bus/aux/master processing still requires consolidation before independent
stem publication. The renderer continues to reject unsupported shared routing,
rather than silently publishing different audio. No new Show build is included.

## Explicit scope

This is an independent implementation of the basic I/O workflow documented by Avid:
https://kb.avid.com/pkb/articles/en_US/how_to/How-do-I-default-the-IO-setup-in-Pro-Tools
https://apps.avid.com/proToolsFirstHelp/version12.3/ENU/Pro%20Tools%20First%20Help/mix1.basic.54.24.html

Native Pro Tools UI observation was attempted but the app did not respond to the UI
controller. No proprietary application resources, code or artwork were accessed.
The UI is implemented with Sculpt's existing widgets and styles.

Hardware inserts, insert delay calibration, preset import/export, hierarchical
subpaths and recording an internal bus directly onto an audio track are not added.
Mono and stereo paths are independent rows; stereo hardware channels are adjacent.
The previous DAW playback SRC behavior is unchanged by this I/O update.

## Validation

Required: complete repository gate in an isolated test process that cannot read the
installed third-party VST3 directories, bridge tests using the actual Swift receiver,
audio assertions for named inputs and bus/primary-output mapping, save/reopen and
Undo, invalid/stale/recording edits, UI Cancel/Apply interactions, rendered previews,
both Mac architectures, embedded CLI, archive CRC/content and signature verification.
Actual interface recording, audible latency, third-party plugins and project-specific
Show → DAW → Show acceptance remain tests on the user's Mac.
