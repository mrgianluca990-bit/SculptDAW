#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ "$(uname -s)" = Darwin ]] || { echo 'This build requires macOS and Xcode Command Line Tools.' >&2; exit 1; }
cd "$ROOT"
export MACOSX_DEPLOYMENT_TARGET=11.0
export CARGO_TARGET_DIR="$ROOT/target"
export SOUNDCRAFT_BUILD_SHA="$(git rev-parse HEAD 2>/dev/null || echo source-archive)"
VERSION=0.4.0
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target add "$target"
  cargo build --release --locked --target "$target" -p sculpt-daw -p soundcraft-cli
done
WORK="$(mktemp -d "$ROOT/target/sculpt-package.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/Sculpt DAW.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$ROOT/dist/sculpt"
for bin in sculpt-daw soundcraft-cli; do
  lipo -create "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/$bin" "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/$bin" -output "$APP/Contents/MacOS/$bin"
  lipo "$APP/Contents/MacOS/$bin" -verify_arch arm64 x86_64
  otool -L "$APP/Contents/MacOS/$bin"
done
ICONSET="$WORK/Sculpt.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/sculpt-daw/sculpt-daw-512.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" assets/sculpt-daw/sculpt-daw-512.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Sculpt DAW.icns"
export SCULPT_APP="$APP" SCULPT_VERSION="$VERSION"
python3 - <<'PY'
import os, pathlib, plistlib, shutil
app = pathlib.Path(os.environ['SCULPT_APP'])
with open('packaging/macos/Info.plist.in', 'rb') as f:
    info = plistlib.load(f)
info.update(CFBundleExecutable='sculpt-daw', CFBundleShortVersionString=os.environ['SCULPT_VERSION'], CFBundleVersion=os.environ['SCULPT_VERSION'])
info['SoundCraftVersion'] = '0.6.1 (Sculpt 0.4.0)'
info['SoundCraftBuildCommit'] = os.environ['SOUNDCRAFT_BUILD_SHA']
with (app/'Contents/Info.plist').open('wb') as f:
    plistlib.dump(info, f)
for name in ['LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'ATTRIBUTION.md', 'SCULPT-ORIGIN.md']:
    shutil.copy2(name, app/'Contents/Resources'/name)
PY
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
xattr -cr "$APP"
for bin in sculpt-daw soundcraft-cli; do
  codesign --force --sign - --timestamp=none --entitlements packaging/macos/entitlements.plist "$APP/Contents/MacOS/$bin"
done
codesign --force --sign - --timestamp=none --entitlements packaging/macos/entitlements.plist "$APP"
codesign --verify --deep --strict "$APP"
"$APP/Contents/MacOS/soundcraft-cli" --version
OUTPUT="$ROOT/dist/sculpt/Sculpt_DAW_${VERSION}_Mac_Universal.zip"
ditto -c -k --keepParent "$APP" "$OUTPUT"
shasum -a 256 "$OUTPUT" > "$OUTPUT.sha256"
echo "Created: $OUTPUT (ad-hoc signed; not notarized)."
