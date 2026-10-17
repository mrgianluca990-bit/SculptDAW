#!/usr/bin/env bash
# Sculpt DAW macOS universal build and packaging (Apple Silicon + Intel)
# Sculpt Bridge: the CLI is nested in the app bundle, not shipped as a sidecar.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$(uname -s)" != Darwin ]]; then
  echo 'This build requires macOS and Xcode Command Line Tools.' >&2
  exit 1
fi
cd "$ROOT"
export MACOSX_DEPLOYMENT_TARGET=11.0
export CARGO_TARGET_DIR="$ROOT/target"
export SOUNDCRAFT_BUILD_SHA="$(git rev-parse HEAD 2>/dev/null || echo source-archive)"
VERSION=0.4.0

# CORE20 and CORE21 must overlay the full Core19 source restored by the GitHub workflow.
# The patch aborts immediately if the upstream file shapes have drifted.
python3 -m py_compile QA/SculptDAW_Core20_Patch.py
python3 QA/SculptDAW_Core20_Patch.py
python3 -m py_compile QA/SculptDAW_Core21_IO_Patch.py
python3 QA/SculptDAW_Core21_IO_Patch.py
# Core22 must run after all legacy overlays, before the universal release build.
python3 -m py_compile QA/SculptDAW_Core22_Patch.py
python3 QA/SculptDAW_Core22_Patch.py
# DAW Core23: tested field corrections after the stable Core22 overlay.
python3 -m py_compile QA/SculptDAW_Core23_Patch.py
python3 QA/SculptDAW_Core23_Patch.py
# We validate the final, actually packaged source (the earlier workflow tests
# run before the patch for historical reasons).
cargo check --locked --target x86_64-apple-darwin -p sculpt-daw
# Real DSP test: channel 64 + no-device Main fallback, not just UI snapshots.
cargo test --locked --target x86_64-apple-darwin -p soundcraft-mix --lib sculpt_core21_hardware_output64

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target add "$target"
  cargo build --release --locked --target "$target" -p sculpt-daw -p soundcraft-cli
done

mkdir -p "$ROOT/target" "$ROOT/dist/sculpt"
WORK="$(mktemp -d "$ROOT/target/sculpt-package.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/Sculpt DAW.app"
CLI="$APP/Contents/Helpers/soundcraft-cli"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers" "$APP/Contents/Resources"

for bin in sculpt-daw soundcraft-cli; do
  if [[ "$bin" == sculpt-daw ]]; then
    dest="$APP/Contents/MacOS/sculpt-daw"
  else
    dest="$CLI"
  fi
  lipo -create \
    "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/$bin" \
    "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/$bin" \
    -output "$dest"
  chmod 755 "$dest"
  lipo "$dest" -verify_arch arm64 x86_64
  otool -L "$dest"
done

ICONSET="$WORK/Sculpt.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/sculpt-daw/sculpt-daw-512.png \
    --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" assets/sculpt-daw/sculpt-daw-512.png \
    --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Sculpt DAW.icns"

export SCULPT_APP="$APP" SCULPT_VERSION="$VERSION"
python3 - <<'PY'
import os
import pathlib
import plistlib
import shutil

app = pathlib.Path(os.environ['SCULPT_APP'])
with open('packaging/macos/Info.plist.in', 'rb') as f:
    info = plistlib.load(f)
info.update(
    CFBundleExecutable='sculpt-daw',
    CFBundleShortVersionString=os.environ['SCULPT_VERSION'],
    CFBundleVersion=os.environ['SCULPT_VERSION'],
)
info['SoundCraftVersion'] = '0.6.1 (Sculpt 0.4.0)'
info['SoundCraftBuildCommit'] = os.environ['SOUNDCRAFT_BUILD_SHA']
with (app / 'Contents' / 'Info.plist').open('wb') as f:
    plistlib.dump(info, f)
for name in ['LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'ATTRIBUTION.md', 'SCULPT-ORIGIN.md']:
    shutil.copy2(name, app / 'Contents' / 'Resources' / name)
PY

plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
xattr -cr "$APP"

# Signing order matters: first sign nested helpers, THEN the enclosing .app.
# Signing the CLI after the app would invalidate the enclosing bundle seal.
codesign --force --sign - --timestamp=none "$CLI"
codesign --verify --strict --verbose=2 "$CLI"
"$CLI" --version

codesign --force --sign - --timestamp=none \
  --entitlements packaging/macos/entitlements.plist "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign --verify --strict --verbose=2 "$APP/Contents/MacOS/sculpt-daw"
codesign --verify --strict --verbose=2 "$CLI"

OUTPUT="$ROOT/dist/sculpt/Sculpt_DAW_${VERSION}_Mac_Universal.zip"
rm -f "$OUTPUT" "$OUTPUT.sha256"
# One deliverable: the signed app including its signed CLI helper.
ditto -c -k --keepParent "$APP" "$OUTPUT"
unzip -t "$OUTPUT" >/dev/null
python3 - "$OUTPUT" <<'PY_ZIP'
import sys
import zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    names = z.namelist()
    cli = [p for p in names if p.endswith('/soundcraft-cli') or p == 'soundcraft-cli']
    expected = 'Sculpt DAW.app/Contents/Helpers/soundcraft-cli'
    assert cli == [expected], f'CLI must be bundled once under Contents/Helpers: {cli}'
    assert 'Sculpt DAW.app/Contents/MacOS/sculpt-daw' in names, 'Missing DAW executable'
    assert 'soundcraft-cli' not in names, 'Unexpected standalone CLI'
    assert {p.split('/')[0] for p in names if p.strip('/')} == {'Sculpt DAW.app'}, 'Archive contains more than Sculpt DAW.app'
    assert not any(p.lower().endswith(('.zip','.tar','.gz')) for p in names), 'Nested archive detected'
print('SCULPT CORE22: one app bundle, embedded signed CLI; no standalone CLI')
PY_ZIP
shasum -a 256 "$OUTPUT" > "$OUTPUT.sha256"
echo "Created: $OUTPUT (CLI integrated; ad-hoc signed; not notarized)."
