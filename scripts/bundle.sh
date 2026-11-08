#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != Darwin ]]; then
  echo 'Caffettino needs macOS.' >&2
  exit 1
fi

# By default this builds an app for the current Mac. `--universal` builds one for
# both Apple Silicon and Intel Macs, as releases ship.
case "${1:-}" in
  '') targets=("$(rustc -vV | sed -n 's/^host: //p')") ;;
  --universal) targets=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  *) echo "Usage: $0 [--universal]" >&2; exit 2 ;;
esac

# Keep the artifact path deterministic even with a user-configured target directory
# or cross-compilation target.
export MACOSX_DEPLOYMENT_TARGET=13.0
binaries=()
for target in "${targets[@]}"; do
  cargo build --release --locked --target "$target" --target-dir "$PWD/target"
  binaries+=("target/$target/release/caffettino")
done
bundle="$PWD/dist/Caffettino.app"
iconset="$PWD/target/Caffettino.iconset"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources" "$iconset"
lipo -create "${binaries[@]}" -output "$bundle/Contents/MacOS/caffettino"
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
