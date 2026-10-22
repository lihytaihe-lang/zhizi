#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT_DIR/script/sync_version.sh" --check
CONFIGURATION="${1:-release}"
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
INPUT_GUARD="$ROOT_DIR/script/rc_gate_support.sh"
CORE_TARGET_DIR="$ROOT_DIR/native/core/target"
CORE_LIBRARY_DIR="$CORE_TARGET_DIR/release"
SWIFT_BUILD_ARGS=()
if [[ "${AIM_SWIFTPM_SANDBOX_WORKAROUND:-0}" == "1" ]]; then
  SWIFT_BUILD_ARGS=(--disable-sandbox)
  SWIFT_MODULE_CACHE="${TMPDIR:-/tmp}/agent-ime-native-swift-module-cache"
  mkdir -p "$SWIFT_MODULE_CACHE"
  export CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE"
  export SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE"
fi

case "$CONFIGURATION" in
  debug|release) ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 2 ;;
esac

[[ -x "$INPUT_GUARD" ]] || { echo "missing RC input guard: $INPUT_GUARD" >&2; exit 3; }
PRODUCT_COMMIT="$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" HEAD)"
PRODUCT_INPUTS_SHA256="$("$INPUT_GUARD" fingerprint "$ROOT_DIR" "$PRODUCT_COMMIT")"
if [[ -z "${BUILD_NUMBER:-}" ]]; then
  BUILD_NUMBER="$((1000 + $(git -C "$ROOT_DIR" rev-list --count "$PRODUCT_COMMIT")))"
fi
[[ "$BUILD_NUMBER" =~ ^[1-9][0-9]{0,8}$ ]] || { echo "invalid BUILD_NUMBER: $BUILD_NUMBER" >&2; exit 2; }

cd "$ROOT_DIR"
"$ROOT_DIR/script/bootstrap_native.sh"
node "$ROOT_DIR/scripts/freeze_candidate_corpus.mjs"
node "$ROOT_DIR/scripts/validate_agent_daily_dictionary.mjs"
"$ROOT_DIR/script/check_idiom_dictionary.sh"
"$ROOT_DIR/script/check_modern_dictionary.sh"
if "$INPUT_GUARD" is-public-build "$ROOT_DIR"; then
  # Public source export: these two checks read internal documents and a target list mined from private typing records, none of
  # which are published. The files they guard (the project-term layer and the disambiguation table) are checked by hash elsewhere.
  echo "public build: skipping generate_project_terms --check and check_context_disambig (they need internal-only inputs)"
else
  node "$ROOT_DIR/scripts/generate_project_terms.mjs" --check
fi
node "$ROOT_DIR/scripts/generate_exact_reading_index.mjs" --check
if ! "$INPUT_GUARD" is-public-build "$ROOT_DIR"; then
  node "$ROOT_DIR/scripts/check_context_disambig.mjs"
fi
node "$ROOT_DIR/scripts/validate_native_license_assets.mjs"
CARGO_NET_OFFLINE=true node "$ROOT_DIR/scripts/generate_locked_dependency_registry.mjs" --check

CARGO_TARGET_DIR="$CORE_TARGET_DIR" cargo build --locked --offline \
  --manifest-path "$ROOT_DIR/native/core/Cargo.toml" --release --features librime
touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$CORE_LIBRARY_DIR" swift build --package-path "$ROOT_DIR/native/macos" \
  -c "$CONFIGURATION" --disable-automatic-resolution ${SWIFT_BUILD_ARGS[@]+"${SWIFT_BUILD_ARGS[@]}"}

SWIFT_BIN_DIR="$(AIM_CORE_LIBRARY_DIR="$CORE_LIBRARY_DIR" swift build \
  --package-path "$ROOT_DIR/native/macos" -c "$CONFIGURATION" \
  --disable-automatic-resolution ${SWIFT_BUILD_ARGS[@]+"${SWIFT_BUILD_ARGS[@]}"} --show-bin-path)"
[[ "$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$PRODUCT_COMMIT")" == "$PRODUCT_COMMIT" ]]
mkdir -p "$ROOT_DIR/build/swift-module-cache"
/usr/bin/xcrun swiftc -O -framework Carbon -framework Foundation \
  -module-cache-path "$ROOT_DIR/build/swift-module-cache" \
  "$ROOT_DIR/script/input_source_ctl.swift" -o "$SWIFT_BIN_DIR/AgentIMEInputSourceCtl"
RIME_DIR="$ROOT_DIR/.build-deps/librime/1.16.1"
PINYIN_DIR="$ROOT_DIR/.build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0"
NATIVE_LICENSE_DIR="$ROOT_DIR/third_party/licenses"
RUST_LICENSE_DIR="$ROOT_DIR/.build-deps/rust/1.94.1/licenses"
OUTPUT_DIR="$ROOT_DIR/build/native/$CONFIGURATION"
BUNDLE="$OUTPUT_DIR/Agent Input Method.inputmethod"
CONTENTS="$BUNDLE/Contents"

rm -rf "$BUNDLE"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Frameworks" "$CONTENTS/Resources/Rime" "$CONTENTS/Resources/Licenses" "$CONTENTS/Resources/Skins"
cp "$SWIFT_BIN_DIR/AgentInputMethod" "$CONTENTS/MacOS/AgentInputMethod"
cp "$SWIFT_BIN_DIR/AgentIMEInputSourceCtl" "$CONTENTS/MacOS/AgentIMEInputSourceCtl"
cp "$SWIFT_BIN_DIR/AgentIMEPersonalReplayHarness" "$CONTENTS/MacOS/AgentIMEPersonalReplayHarness"
cp "$SWIFT_BIN_DIR/AgentIMEUserDBCtl" "$CONTENTS/MacOS/AgentIMEUserDBCtl"
cp "$RIME_DIR/lib/librime.1.dylib" "$CONTENTS/Frameworks/librime.1.dylib"
cp "$ROOT_DIR/native/macos/Resources/InputMethod-Info.plist" "$CONTENTS/Info.plist"
printf 'APPL????' > "$CONTENTS/PkgInfo"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NUMBER" "$CONTENTS/Info.plist"
# The release version testers and friends see (VERSION at the repo root: 0.1.0, 0.1.1, ...); the menu shows it next to the build number.
RELEASE_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
[[ "$RELEASE_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "bad VERSION file: $RELEASE_VERSION" >&2; exit 2; }
/usr/libexec/PlistBuddy -c "Delete :ZhiziReleaseVersion" "$CONTENTS/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :ZhiziReleaseVersion string $RELEASE_VERSION" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :AgentIMEProductCommit" "$CONTENTS/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :AgentIMEProductCommit string $PRODUCT_COMMIT" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :AgentIMEProductInputsSHA256" "$CONTENTS/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :AgentIMEProductInputsSHA256 string $PRODUCT_INPUTS_SHA256" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :AgentIMEBuildConfiguration" "$CONTENTS/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :AgentIMEBuildConfiguration string $CONFIGURATION" "$CONTENTS/Info.plist"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$CONTENTS/Info.plist")" == "$RELEASE_VERSION" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$CONTENTS/Info.plist")" == "$BUILD_NUMBER" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CONTENTS/Info.plist")" == "com.hongye.facai.inputmethod.zhizi" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEBuildConfiguration' "$CONTENTS/Info.plist")" == "$CONFIGURATION" ]]
cp "$ROOT_DIR/native/macos/Resources/AgentInputMethod.icns" "$CONTENTS/Resources/AgentInputMethod.icns"
cp "$ROOT_DIR/native/macos/Resources/AgentInputMethodMenu.pdf" "$CONTENTS/Resources/AgentInputMethodMenu.pdf"
mkdir -p "$CONTENTS/Resources/zh-Hans.lproj" "$CONTENTS/Resources/en.lproj"
cp "$ROOT_DIR/native/macos/Resources/zh-Hans.lproj/InfoPlist.strings" "$CONTENTS/Resources/zh-Hans.lproj/InfoPlist.strings"
cp "$ROOT_DIR/native/macos/Resources/en.lproj/InfoPlist.strings" "$CONTENTS/Resources/en.lproj/InfoPlist.strings"
mkdir -p "$CONTENTS/Resources/Lexicon"
cp "$ROOT_DIR/native/resources/Lexicon/lexicon_catalog.json" "$CONTENTS/Resources/Lexicon/lexicon_catalog.json"
cp -R "$ROOT_DIR/native/resources/Skins/Builtin" "$CONTENTS/Resources/Skins/Builtin"
cp "$ROOT_DIR/native/resources/Skins/skin.schema.json" "$CONTENTS/Resources/Skins/skin.schema.json"
python3 "$ROOT_DIR/scripts/context_model/copy_bundle_model.py" "$CONTENTS/Resources/ContextModel"
cp "$ROOT_DIR/third_party/context-disambig/disambig-table-v1.json" "$CONTENTS/Resources/ContextDisambig.json"
cp "$ROOT_DIR/native/resources/Rime/default.yaml" "$CONTENTS/Resources/Rime/default.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_pinyin.schema.yaml" "$CONTENTS/Resources/Rime/agent_pinyin.schema.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_pinyin.dict.yaml" "$CONTENTS/Resources/Rime/agent_pinyin.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_common_fix.dict.yaml" "$CONTENTS/Resources/Rime/agent_common_fix.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_daily.dict.yaml" "$CONTENTS/Resources/Rime/agent_daily.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_english.dict.yaml" "$CONTENTS/Resources/Rime/agent_english.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_project_terms.dict.yaml" "$CONTENTS/Resources/Rime/agent_project_terms.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_tech_terms.dict.yaml" "$CONTENTS/Resources/Rime/agent_tech_terms.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_ai_terms.dict.yaml" "$CONTENTS/Resources/Rime/agent_ai_terms.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_pack_terms.dict.yaml" "$CONTENTS/Resources/Rime/agent_pack_terms.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_idioms.dict.yaml" "$CONTENTS/Resources/Rime/agent_idioms.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_thuocl_it.dict.yaml" "$CONTENTS/Resources/Rime/agent_thuocl_it.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_thuocl_caijing.dict.yaml" "$CONTENTS/Resources/Rime/agent_thuocl_caijing.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_modern.dict.yaml" "$CONTENTS/Resources/Rime/agent_modern.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_general.dict.yaml" "$CONTENTS/Resources/Rime/agent_general.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_newwords.dict.yaml" "$CONTENTS/Resources/Rime/agent_newwords.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_spoken.dict.yaml" "$CONTENTS/Resources/Rime/agent_spoken.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_ai_names.dict.yaml" "$CONTENTS/Resources/Rime/agent_ai_names.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_pack_poetry.dict.yaml" "$CONTENTS/Resources/Rime/agent_pack_poetry.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_pack_cad.dict.yaml" "$CONTENTS/Resources/Rime/agent_pack_cad.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_pack_industry.dict.yaml" "$CONTENTS/Resources/Rime/agent_pack_industry.dict.yaml"
cp "$ROOT_DIR/native/resources/Rime/agent_exact_readings.tsv" "$CONTENTS/Resources/Rime/agent_exact_readings.tsv"
cp "$PINYIN_DIR/pinyin_simp.dict.yaml" "$CONTENTS/Resources/Rime/pinyin_simp.dict.yaml"
cp "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$CONTENTS/Resources/THIRD_PARTY_NOTICES.md"
cp "$RIME_DIR/licenses/LICENSE.librime" "$CONTENTS/Resources/Licenses/librime-BSD-3-Clause.txt"
cp "$PINYIN_DIR/LICENSE" "$CONTENTS/Resources/Licenses/rime-pinyin-simp-Apache-2.0.txt"
cp "$PINYIN_DIR/AUTHORS" "$CONTENTS/Resources/Licenses/rime-pinyin-simp-AUTHORS.txt"
cp "$NATIVE_LICENSE_DIR/THUOCL-MIT.txt" "$CONTENTS/Resources/Licenses/THUOCL-MIT.txt"
cp "$NATIVE_LICENSE_DIR/jieba-MIT.txt" "$CONTENTS/Resources/Licenses/jieba-MIT.txt"
cp "$NATIVE_LICENSE_DIR/pypinyin-MIT.txt" "$CONTENTS/Resources/Licenses/pypinyin-MIT.txt"
cp "$RUST_LICENSE_DIR/LICENSE-APACHE" "$CONTENTS/Resources/Licenses/Rust-standard-library-Apache-2.0.txt"
cp "$RUST_LICENSE_DIR/LICENSE-MIT" "$CONTENTS/Resources/Licenses/Rust-standard-library-MIT.txt"
cp "$RUST_LICENSE_DIR/COPYRIGHT-library.html" "$CONTENTS/Resources/Licenses/Rust-standard-library-COPYRIGHT.txt"
cp "$NATIVE_LICENSE_DIR/boost-BSL-1.0.txt" "$CONTENTS/Resources/Licenses/boost-BSL-1.0.txt"
cp "$NATIVE_LICENSE_DIR/glog-BSD-3-Clause.txt" "$CONTENTS/Resources/Licenses/glog-BSD-3-Clause.txt"
cp "$NATIVE_LICENSE_DIR/leveldb-BSD-3-Clause.txt" "$CONTENTS/Resources/Licenses/leveldb-BSD-3-Clause.txt"
cp "$NATIVE_LICENSE_DIR/marisa-trie-COPYING.txt" "$CONTENTS/Resources/Licenses/marisa-trie-COPYING.txt"
cp "$NATIVE_LICENSE_DIR/OpenCC-Apache-2.0.txt" "$CONTENTS/Resources/Licenses/OpenCC-Apache-2.0.txt"
cp "$NATIVE_LICENSE_DIR/darts-clone-COPYING.txt" "$CONTENTS/Resources/Licenses/darts-clone-COPYING.txt"
cp "$NATIVE_LICENSE_DIR/RapidJSON-LICENSE.txt" "$CONTENTS/Resources/Licenses/RapidJSON-LICENSE.txt"
cp "$NATIVE_LICENSE_DIR/yaml-cpp-MIT.txt" "$CONTENTS/Resources/Licenses/yaml-cpp-MIT.txt"
node "$ROOT_DIR/scripts/validate_native_license_assets.mjs" --bundle "$BUNDLE"

chmod 755 "$CONTENTS/MacOS/AgentInputMethod" "$CONTENTS/MacOS/AgentIMEInputSourceCtl" "$CONTENTS/MacOS/AgentIMEPersonalReplayHarness" "$CONTENTS/MacOS/AgentIMEUserDBCtl" "$CONTENTS/Frameworks/librime.1.dylib"
codesign --force --sign "$SIGN_IDENTITY" --timestamp=none "$CONTENTS/Frameworks/librime.1.dylib"
codesign --force --sign "$SIGN_IDENTITY" --timestamp=none "$CONTENTS/MacOS/AgentIMEInputSourceCtl"
codesign --force --sign "$SIGN_IDENTITY" --timestamp=none "$CONTENTS/MacOS/AgentIMEPersonalReplayHarness"
codesign --force --sign "$SIGN_IDENTITY" --timestamp=none "$CONTENTS/MacOS/AgentIMEUserDBCtl"
codesign --force --deep --sign "$SIGN_IDENTITY" --timestamp=none "$BUNDLE"

plutil -lint "$CONTENTS/Info.plist"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEProductCommit' "$CONTENTS/Info.plist")" == "$PRODUCT_COMMIT" ]]
[[ "$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEProductInputsSHA256' "$CONTENTS/Info.plist")" == "$PRODUCT_INPUTS_SHA256" ]]
codesign --verify --deep --strict --verbose=2 "$BUNDLE"
file "$CONTENTS/MacOS/AgentInputMethod" "$CONTENTS/MacOS/AgentIMEInputSourceCtl" "$CONTENTS/MacOS/AgentIMEPersonalReplayHarness" "$CONTENTS/MacOS/AgentIMEUserDBCtl" "$CONTENTS/Frameworks/librime.1.dylib"
otool -L "$CONTENTS/MacOS/AgentInputMethod"
otool -L "$CONTENTS/Frameworks/librime.1.dylib"
shasum -a 256 \
  "$CONTENTS/MacOS/AgentInputMethod" \
  "$CONTENTS/MacOS/AgentIMEPersonalReplayHarness" \
  "$CONTENTS/MacOS/AgentIMEUserDBCtl" \
  "$CONTENTS/Frameworks/librime.1.dylib" \
  "$CONTENTS/Resources/Rime/agent_pinyin.schema.yaml" \
  "$CONTENTS/Resources/Rime/agent_pinyin.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_common_fix.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_daily.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_english.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_project_terms.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_tech_terms.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_ai_terms.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_pack_terms.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_idioms.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_thuocl_it.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_thuocl_caijing.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_modern.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_general.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_newwords.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_spoken.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_ai_names.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_pack_poetry.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_pack_cad.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_pack_industry.dict.yaml" \
  "$CONTENTS/Resources/Rime/agent_exact_readings.tsv" \
  "$CONTENTS/Resources/Rime/pinyin_simp.dict.yaml" \
  "$CONTENTS/Resources/Lexicon/lexicon_catalog.json" \
  "$CONTENTS/Resources/Skins/skin.schema.json" \
  "$CONTENTS/Resources/Skins/Builtin/system/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/xuanmo/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/shuitun/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/youzi/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/juzimao/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/juzimao/cat-actions.png" \
  "$CONTENTS/Resources/Skins/Builtin/chaiquan/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/chaiquan/shiba-actions.png" \
  "$CONTENTS/Resources/Skins/Builtin/baitu/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/baitu/rabbit-actions.png" \
  "$CONTENTS/Resources/Skins/Builtin/yueye/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/luoying/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/fenghong/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/denglong/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/songxue/skin.json" \
  "$CONTENTS/Resources/Skins/Builtin/nihong/skin.json" \
  "$CONTENTS/Resources/ContextDisambig.json" \
  "$CONTENTS/Resources/Licenses/librime-BSD-3-Clause.txt" \
  "$CONTENTS/Resources/Licenses/boost-BSL-1.0.txt" \
  "$CONTENTS/Resources/Licenses/glog-BSD-3-Clause.txt" \
  "$CONTENTS/Resources/Licenses/leveldb-BSD-3-Clause.txt" \
  "$CONTENTS/Resources/Licenses/marisa-trie-COPYING.txt" \
  "$CONTENTS/Resources/Licenses/OpenCC-Apache-2.0.txt" \
  "$CONTENTS/Resources/Licenses/darts-clone-COPYING.txt" \
  "$CONTENTS/Resources/Licenses/RapidJSON-LICENSE.txt" \
  "$CONTENTS/Resources/Licenses/rime-pinyin-simp-Apache-2.0.txt" \
  "$CONTENTS/Resources/Licenses/rime-pinyin-simp-AUTHORS.txt" \
  "$CONTENTS/Resources/Licenses/THUOCL-MIT.txt" \
  "$CONTENTS/Resources/Licenses/jieba-MIT.txt" \
  "$CONTENTS/Resources/Licenses/pypinyin-MIT.txt" \
  "$CONTENTS/Resources/Licenses/Rust-standard-library-Apache-2.0.txt" \
  "$CONTENTS/Resources/Licenses/Rust-standard-library-MIT.txt" \
  "$CONTENTS/Resources/Licenses/Rust-standard-library-COPYRIGHT.txt" \
  "$CONTENTS/Resources/Licenses/yaml-cpp-MIT.txt" \
  "$CONTENTS/Resources/THIRD_PARTY_NOTICES.md"
[[ "$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$PRODUCT_COMMIT")" == "$PRODUCT_COMMIT" ]]
"$ROOT_DIR/script/unregister_dev_bundles.sh" >/dev/null 2>&1 || true
echo "$BUNDLE"
