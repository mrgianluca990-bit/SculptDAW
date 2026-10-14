# Sculpt compatibility companion

`import_reaper.py` parses REAPER `.rpp` and writes actual native SoundCraft CLI
commands (`.scscript`) that save a `.scraft` session when run through the
upstream `soundcraft-cli` binary. This is a **limited external importer**, not
in-app opening of `.rpp` or a guarantee of full session fidelity.

It requires `ffmpeg` for media-region extraction, so source trims and
positions are preserved at the audio level. The original RPP/media are not
modified. The generated compatibility report explicitly identifies omissions.

Example (from the SoundCraft/Sculpt source directory):

```sh
python3 sculpt/compatibility/import_reaper.py /path/to/session.rpp --output /tmp/sculpt-import
cargo run --release -p soundcraft-cli -- script /tmp/sculpt-import/import.scscript --save /tmp/sculpt-import/session.scraft
cargo run --release -p sculpt-daw -- /tmp/sculpt-import/session.scraft
```

`scan_plugins.py` inventories plugin bundles in VST3, VST2, AU, AAX and CLAP
folders on macOS and offers conservative **name-based** alternatives. No
plugin is loaded; presets are not converted.

```sh
python3 sculpt/compatibility/scan_plugins.py --output /tmp/sculpt-plugins.json
```
