# Sculpt DAW Core17 QA gate

- Baseline pinned: Core15 commit `6d0b50cef84523e44c9e6ad1af13b0570574c933` (Mac passed).
- Replays Core16 + Spot modifications (Mac release binaries compiled in run #16, but artifact blocked by incorrect packaging validation).
- Conditionally applies new patch, guards source state and amber icon hash.
- Cargo target `x86_64-apple-darwin`, universal package includes arm64/x86_64.
- New test modules: sculpt_core17_project_tests, sculpt_core17_marker_tests, sculpt_core17_group_tests.
- Existing tests: Core15 crossfade/fades, Core16 Spot/QA, VST3/AU hosts and audio playback.
- Validates zip checksum and UTI using its embedded Info.plist.
- Does not automatically commit source changes from CI; durable Rust source consolidation into repository should follow only after user acceptance, instead of keeping source fixes embedded in workflow.
- Show interoperability is not yet added; markers remain within Sculpt DAW, preserving session metadata for later bridge.
