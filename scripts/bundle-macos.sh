#!/bin/sh
# Build Promptly.app (Apple silicon). Signing/notarization run only when the
# identity env vars are set, so local builds stay unsigned.
#   APPLE_SIGN_IDENTITY="Developer ID Application: ..."  NOTARY_PROFILE=promptly ./scripts/bundle-macos.sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --workspace
VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys;print([p for p in json.load(sys.stdin)["packages"] if p["name"]=="promptly"][0]["version"])')
APP=target/release/Promptly.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/promptly target/release/promptly-ctl "$APP/Contents/MacOS/"
sed "s/__VERSION__/$VERSION/g" packaging/Info.plist > "$APP/Contents/Info.plist"
if [ -n "${APPLE_SIGN_IDENTITY:-}" ]; then
  codesign --force --options runtime --timestamp --sign "$APPLE_SIGN_IDENTITY" "$APP/Contents/MacOS/promptly-ctl"
  codesign --force --options runtime --timestamp --sign "$APPLE_SIGN_IDENTITY" "$APP"
  if [ -n "${NOTARY_PROFILE:-}" ]; then
    ditto -c -k --keepParent "$APP" target/release/Promptly.zip
    xcrun notarytool submit target/release/Promptly.zip --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$APP"
  fi
fi
echo "built $APP"
