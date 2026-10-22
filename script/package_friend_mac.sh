#!/usr/bin/env bash
# Friends' Mac test package: one 知字.dmg holding the manager with the input method
# inside (the manager installs it on first launch, src-tauri/src/friend_install.rs).
# Uses the bundles built by test_all.sh (input method) and build_manager.sh (manager).
#
#   SIGN_IDENTITY  Developer ID Application identity (default: the owner's; "-" for a dry run)
#   NOTARIZE       1 (default) submits to Apple from the Mac mini that holds the API key
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SIGN_IDENTITY="${SIGN_IDENTITY:-Developer ID Application: Li Hongye (79YH2GYTHN)}"
NOTARIZE="${NOTARIZE:-1}"
NOTARY_HOST="${NOTARY_HOST:-minihermes}"
NOTARY_KEY_ID="${NOTARY_KEY_ID:-4H38PYHZA5}"
NOTARY_ISSUER="${NOTARY_ISSUER:-49d5d59f-44c5-4fab-a0ae-b7212776b2fe}"

IME="$ROOT_DIR/build/native/release/Agent Input Method.inputmethod"
MANAGER="$ROOT_DIR/src-tauri/target/release/bundle/macos/Agent Input Method.app"
[[ -d "$IME" && -d "$MANAGER" ]] || { echo "build the input method and manager first" >&2; exit 2; }
BUILD="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$MANAGER/Contents/Info.plist")"
OUT="$ROOT_DIR/releases/friend-mac/$BUILD"
STAGE="$OUT/stage"
APP="$STAGE/知字.app"
RELEASE_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
DMG="$OUT/知字-$RELEASE_VERSION.dmg"   # named by release version (0.1.1), not the build number
rm -rf "$OUT"
mkdir -p "$STAGE"

ditto "$MANAGER" "$APP"
mkdir -p "$APP/Contents/Resources/InputMethod"
ditto "$IME" "$APP/Contents/Resources/InputMethod/Agent Input Method.app"

sign() {
  if [[ "$SIGN_IDENTITY" == "-" ]]; then
    codesign --force --sign - --timestamp=none "$@"
  else
    codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$@"
  fi
}

# Inside out: libraries, then executables, then each bundle.
NESTED="$APP/Contents/Resources/InputMethod/Agent Input Method.app"
while IFS= read -r library; do sign "$library"; done \
  < <(find "$NESTED" -type f -name '*.dylib' | LC_ALL=C sort)
while IFS= read -r executable; do sign "$executable"; done \
  < <(find "$NESTED/Contents/MacOS" -type f -perm -111 | LC_ALL=C sort)
sign "$NESTED"
while IFS= read -r executable; do sign "$executable"; done \
  < <(find "$APP/Contents/MacOS" -type f -perm -111 | LC_ALL=C sort)
sign "$APP"
codesign --verify --deep --strict --verbose=1 "$APP"

cp "$ROOT_DIR/script/friend-mac-readme.txt" "$STAGE/使用说明.txt"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "知字" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
[[ "$SIGN_IDENTITY" == "-" ]] || codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"

if [[ "$NOTARIZE" == "1" && "$SIGN_IDENTITY" != "-" ]]; then
  # The App Store Connect key stays on the Mac mini; only the dmg travels.
  REMOTE="/tmp/zhizi-notarize-$BUILD.dmg"
  scp -q "$DMG" "$NOTARY_HOST:$REMOTE"
  ssh -o BatchMode=yes "$NOTARY_HOST" \
    "xcrun notarytool submit '$REMOTE' --key ~/.appstoreconnect/private_keys/AuthKey_$NOTARY_KEY_ID.p8 \
       --key-id $NOTARY_KEY_ID --issuer $NOTARY_ISSUER --wait; result=\$?; rm -f '$REMOTE'; exit \$result"
  xcrun stapler staple "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
fi

shasum -a 256 "$DMG" | tee "$DMG.sha256"
echo "friend_mac_package=$DMG"
