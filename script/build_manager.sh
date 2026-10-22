#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCT_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
"$ROOT_DIR/script/sync_version.sh" --check
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
INPUT_GUARD="$ROOT_DIR/script/rc_gate_support.sh"
[[ -x "$INPUT_GUARD" ]] || { echo "missing RC input guard: $INPUT_GUARD" >&2; exit 3; }
PRODUCT_COMMIT="$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" HEAD)"
PRODUCT_INPUTS_SHA256="$("$INPUT_GUARD" fingerprint "$ROOT_DIR" "$PRODUCT_COMMIT")"
if [[ -z "${BUILD_NUMBER:-}" ]]; then
  BUILD_NUMBER="$((1000 + $(git -C "$ROOT_DIR" rev-list --count "$PRODUCT_COMMIT")))"
fi
[[ "$BUILD_NUMBER" =~ ^[1-9][0-9]{0,8}$ ]] || { echo "invalid BUILD_NUMBER: $BUILD_NUMBER" >&2; exit 2; }

cd "$ROOT_DIR"
CARGO_NET_OFFLINE=true node "$ROOT_DIR/scripts/generate_locked_dependency_registry.mjs" --check
# Test builds take their invite only from the environment; never put it in source.
if [[ "${ZHIZI_TEST_INVITE:-}" =~ [^[:space:]] ]]; then
  ZHIZI_TEST_INVITE="$(printf '%s' "${ZHIZI_TEST_INVITE:-}" | tr -d '[:space:]')"
  export ZHIZI_TEST_INVITE
else
  unset ZHIZI_TEST_INVITE
fi
# Optional cargo flags for the Tauri build, e.g. ZHIZI_MANAGER_CARGO_FLAGS=--no-default-features
# (public package); unset keeps the default command line byte-for-byte identical.
if [[ -n "${ZHIZI_MANAGER_CARGO_FLAGS:-}" ]]; then
  read -r -a manager_cargo_flags <<< "$ZHIZI_MANAGER_CARGO_FLAGS"
  npm_config_offline=true CARGO_NET_OFFLINE=true npm run tauri build -- --bundles app -- "${manager_cargo_flags[@]}"
else
  npm_config_offline=true CARGO_NET_OFFLINE=true npm run tauri build -- --bundles app
fi
[[ "$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$PRODUCT_COMMIT")" == "$PRODUCT_COMMIT" ]]

BUNDLE="$ROOT_DIR/src-tauri/target/release/bundle/macos/Agent Input Method.app"
CONTENTS="$BUNDLE/Contents"
[[ -d "$BUNDLE" ]] || { echo "missing manager bundle: $BUNDLE" >&2; exit 2; }
[[ -z "$(find "$BUNDLE" -type l -print -quit)" ]] || { echo "manager bundle contains symlinks" >&2; exit 3; }

/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NUMBER" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :AgentIMEProductCommit" "$CONTENTS/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :AgentIMEProductCommit string $PRODUCT_COMMIT" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :AgentIMEProductInputsSHA256" "$CONTENTS/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :AgentIMEProductInputsSHA256 string $PRODUCT_INPUTS_SHA256" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :AgentIMEBuildConfiguration" "$CONTENTS/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :AgentIMEBuildConfiguration string release" "$CONTENTS/Info.plist"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$CONTENTS/Info.plist")" == "$PRODUCT_VERSION" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$CONTENTS/Info.plist")" == "$BUILD_NUMBER" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CONTENTS/Info.plist")" == "com.agentinputlayer.desktop" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEBuildConfiguration' "$CONTENTS/Info.plist")" == "release" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEProductCommit' "$CONTENTS/Info.plist")" == "$PRODUCT_COMMIT" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEProductInputsSHA256' "$CONTENTS/Info.plist")" == "$PRODUCT_INPUTS_SHA256" ]]
cmp -s "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$CONTENTS/Resources/_up_/THIRD_PARTY_NOTICES.md"
cmp -s "$ROOT_DIR/docs/current/licenses/DEPENDENCY_REGISTRY.md" \
  "$CONTENTS/Resources/_up_/docs/current/licenses/DEPENDENCY_REGISTRY.md"
cmp -s "$ROOT_DIR/docs/current/licenses/LOCKED_DEPENDENCIES.json" \
  "$CONTENTS/Resources/_up_/docs/current/licenses/LOCKED_DEPENDENCIES.json"
cmp -s "$ROOT_DIR/docs/current/licenses/NATIVE_LICENSE_ASSETS.json" \
  "$CONTENTS/Resources/_up_/docs/current/licenses/NATIVE_LICENSE_ASSETS.json"

while IFS= read -r executable; do
  file "$executable" | grep -q 'Mach-O 64-bit executable arm64'
  codesign --force --sign "$SIGN_IDENTITY" --timestamp=none "$executable"
done < <(find "$CONTENTS/MacOS" -type f -perm -111 | LC_ALL=C sort)
codesign --force --sign "$SIGN_IDENTITY" --timestamp=none "$BUNDLE"

plutil -lint "$CONTENTS/Info.plist"
codesign --verify --deep --strict --verbose=2 "$BUNDLE"
[[ "$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$PRODUCT_COMMIT")" == "$PRODUCT_COMMIT" ]]
echo "$BUNDLE"
