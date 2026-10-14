#!/usr/bin/env bash
# Sculpt DAW macOS universal build and packaging (Apple Silicon + Intel)
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

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target add "$target"
  cargo build --release --locked --target "$target" -p sculpt-daw -p soundcraft-cli
done

mkdir -p "$ROOT/target" "$ROOT/dist/sculpt"
WORK="$(mktemp -d "$ROOT/target/sculpt-package.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/Sculpt DAW.app"
CLI="$WORK/soundcraft-cli"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# A macOS application bundle contains only its own primary executable in Contents/MacOS.
# The independent command-line tool is distributed alongside the app, not inside it.
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

# Sign the application bundle as a single unit: codesign signs CFBundleExecutable.
# Signing a second independent executable in Contents/MacOS confuses deep validation.
codesign --force --sign - --timestamp=none \
  --entitlements packaging/macos/entitlements.plist "$APP"
codesign --verify --strict --verbose=2 "$APP"
codesign --verify --strict --verbose=2 "$APP/Contents/MacOS/sculpt-daw"

# The CLI is a separate, independently signed binary.
codesign --force --sign - --timestamp=none "$CLI"
codesign --verify --strict --verbose=2 "$CLI"
"$CLI" --version

OUTPUT="$ROOT/dist/sculpt/Sculpt_DAW_${VERSION}_Mac_Universal.zip"
rm -f "$OUTPUT" "$OUTPUT.sha256"
# Include Sculpt DAW.app and the CLI as siblings in the downloadable ZIP.
ditto -c -k --keepParent "$APP" "$OUTPUT"
zip -q -j "$OUTPUT" "$CLI"
unzip -t "$OUTPUT" >/dev/null
shasum -a 256 "$OUTPUT" > "$OUTPUT.sha256"
echo "Created: $OUTPUT (ad-hoc signed; not notarized)."
