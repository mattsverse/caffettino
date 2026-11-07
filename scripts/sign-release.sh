#!/bin/bash
# Signs an already-built app. Credentials are supplied only to this CI step.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/keychain.sh
umask 077

for name in APPLE_CERTIFICATE_P12 APPLE_CERTIFICATE_PASSWORD \
  APPLE_ID APPLE_TEAM_ID APPLE_APP_SPECIFIC_PASSWORD; do
  if [[ -z "${!name:-}" ]]; then
    echo "Missing GitHub Actions secret: $name" >&2
    exit 1
  fi
done
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || {
  echo 'Release signing requires an Apple Silicon Mac.' >&2
  exit 1
}
version="$(python3 scripts/release.py version --tag "${RELEASE_TAG:?}")"
app="$PWD/dist/Caffettino.app"
[[ -x "$app/Contents/MacOS/caffettino" ]] || { echo 'Build the app first.' >&2; exit 1; }
[[ "$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$app/Contents/Info.plist")" == "$version" ]]
[[ "$(lipo -archs "$app/Contents/MacOS/caffettino")" == arm64 ]]

private_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/caffettino-signing.XXXXXX")"
keychain="$private_dir/signing.keychain-db"
mount_point="$private_dir/mounted"
image_mounted=false
cleanup() {
  if "$image_mounted"; then hdiutil detach "$mount_point" || true; fi
  restore_keychain_search_list || true
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -rf "$private_dir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
keychain_password="$(openssl rand -hex 32)"
if [[ "${GITHUB_ACTIONS:-}" == true ]]; then
  echo "::add-mask::$keychain_password"
fi
export CAFFETTINO_SIGNING_DIR="$private_dir"
python3 - <<'PY'
import base64
import os
from pathlib import Path

private = Path(os.environ["CAFFETTINO_SIGNING_DIR"])
# Accept wrapped base64 while rejecting malformed input.
encoded = "".join(os.environ["APPLE_CERTIFICATE_P12"].split())
(private / "certificate.p12").write_bytes(base64.b64decode(encoded, validate=True))
PY
security create-keychain -p "$keychain_password" "$keychain"
use_signing_keychain "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$private_dir/certificate.p12" -k "$keychain" \
  -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security
security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
  -k "$keychain_password" "$keychain" >/dev/null
rm "$private_dir/certificate.p12"
identity="$(security find-identity -v -p codesigning "$keychain" | python3 -c '
import os, re, sys
team = re.escape(os.environ["APPLE_TEAM_ID"])
matches = re.findall(rf"\b([A-Fa-f0-9]{{40}}) \"Developer ID Application: [^\"]+ \({team}\)\"", sys.stdin.read())
if len(matches) != 1:
    sys.exit("The P12 must contain exactly one valid Developer ID Application identity and private key for APPLE_TEAM_ID")
print(matches[0])
')"
xcrun notarytool store-credentials caffettino --keychain "$keychain" \
  --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD"
codesign --force --sign "$identity" --keychain "$keychain" \
  --options runtime --timestamp "$app"
codesign --verify --deep --strict --verbose=2 "$app"

notarize() {
  local artifact="$1" label="$2" result submission_id submit_status=0
  result="$private_dir/$label-result.json"
  xcrun notarytool submit "$artifact" \
    --keychain "$keychain" --keychain-profile caffettino \
    --wait --timeout 30m --output-format json >"$result" || submit_status=$?
  cat "$result"
  submission_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id", ""))' "$result")"
  if [[ -n "$submission_id" ]]; then
    xcrun notarytool log "$submission_id" \
      --keychain "$keychain" --keychain-profile caffettino \
      "$private_dir/$label-log.json"
    cat "$private_dir/$label-log.json"
  fi
  [[ "$submit_status" == 0 ]] || return "$submit_status"
  python3 -c '
import json, sys
if json.load(open(sys.argv[1])).get("status") != "Accepted":
    sys.exit("Apple did not accept this notarization; refusing to publish")
' "$result"
}

# Stapling fetches the ticket from Apple's CloudKit CDN, which can drop
# connections or lag behind a just-accepted notarization. Retry with backoff.
staple() {
  local target="$1" attempt delay=15
  for attempt in 1 2 3 4 5; do
    if xcrun stapler staple "$target"; then
      return 0
    fi
    if [[ "$attempt" == 5 ]]; then break; fi
    echo "Stapling failed (attempt $attempt/5); retrying in ${delay}s" >&2
    sleep "$delay"
    delay=$((delay * 2))
  done
  return 1
}

# A temporary transport archive lets us staple the app before making the DMG.
ditto -c -k --keepParent "$app" "$private_dir/app.zip"
notarize "$private_dir/app.zip" app
staple "$app"
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=2 "$app"

mkdir "$private_dir/image"
chmod 755 "$private_dir/image"
ditto "$app" "$private_dir/image/Caffettino.app"
ln -s /Applications "$private_dir/image/Applications"
dmg="$PWD/dist/Caffettino-$version-arm64.dmg"
rm -f "$dmg"
# Match Octowatch: HFS+ UDZO images have been corrupt on macOS 15 runners.
hdiutil create -volname Caffettino -srcfolder "$private_dir/image" -fs APFS -format UDZO "$dmg"
hdiutil verify "$dmg"
codesign --sign "$identity" --keychain "$keychain" --timestamp "$dmg"
notarize "$dmg" dmg
staple "$dmg"
xcrun stapler validate "$dmg"
codesign --verify --strict --verbose=2 "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount_point"
image_mounted=true
codesign --verify --deep --strict --verbose=2 "$mount_point/Caffettino.app"
xcrun stapler validate "$mount_point/Caffettino.app"
spctl --assess --type execute --verbose=2 "$mount_point/Caffettino.app"
hdiutil detach "$mount_point"
image_mounted=false
echo "Signed and notarized: $dmg"
