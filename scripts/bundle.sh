#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != Darwin ]]; then
  echo 'Caffettino needs macOS.' >&2
  exit 1
fi

# Keep the artifact path deterministic even with a user-configured target directory
# or cross-compilation target. This script produces an app for the current Mac.
build_target="$(rustc -vV | sed -n 's/^host: //p')"
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo build --release --locked --target "$build_target" --target-dir "$PWD/target"
bundle="$PWD/dist/Caffettino.app"
iconset="$PWD/target/Caffettino.iconset"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources" "$iconset"
cp "target/$build_target/release/caffettino" "$bundle/Contents/MacOS/caffettino"
python3 scripts/release.py plist --output "$bundle/Contents/Info.plist"
swift scripts/draw-icon.swift "$PWD/target/caffettino-icon.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" target/caffettino-icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" target/caffettino-icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$bundle/Contents/Resources/Caffettino.icns"
codesign --force --sign - "$bundle"
echo "Ready: $bundle"
