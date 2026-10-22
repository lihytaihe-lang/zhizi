#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
PRODUCT_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
BUNDLE="${1:-}"
POINTER="${2:-$ROOT_DIR/docs/current/evidence/v0.2-v0.3/RC_ARTIFACT.sha256}"
MINIMUM_BUILD=1020

usage() {
  echo "usage: $0 <frozen-input-bundle> [frozen-rc-pointer]" >&2
  exit 2
}

[[ $# -ge 1 && $# -le 2 ]] || usage
[[ "$BUNDLE" == /* && "$POINTER" == /* ]] || {
  echo "official bundle validation requires absolute bundle and pointer paths" >&2
  exit 2
}
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing or symlinked official input bundle: $BUNDLE" >&2; exit 3; }
[[ -f "$POINTER" && ! -L "$POINTER" ]] || { echo "missing or symlinked frozen RC pointer: $POINTER" >&2; exit 3; }
node "$ROOT_DIR/scripts/validate_native_license_assets.mjs" --bundle "$BUNDLE" >/dev/null

BUNDLE="$(cd "$BUNDLE" && pwd -P)"
POINTER="$(cd "$(dirname "$POINTER")" && pwd -P)/$(basename "$POINTER")"
POINTER_FILE_SHA_BEFORE="$(shasum -a 256 "$POINTER" | awk '{ print $1 }')"

plist_value() {
  /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null
}

signature_value() {
  local code="$1" key="$2"
  codesign -dv --verbose=4 "$code" 2>&1 \
    | awk -F= -v key="$key" '
        $1 == key && !found { value=substr($0, length(key) + 2); found=1 }
        END { if (found) print value; else exit 3 }
      '
}

text_checksum() {
  shasum -a 256 | awk '{ print $1 }'
}

verify_exact_inventory() {
  local expected_count="$#" actual_count path
  if [[ -n "$(find "$BUNDLE" ! -type d ! -type f -print -quit)" ]]; then
    echo "official input bundle contains a symlink or special filesystem entry" >&2
    return 3
  fi
  for path in "$@"; do
    [[ -f "$BUNDLE/$path" && ! -L "$BUNDLE/$path" ]] || {
      echo "official input bundle is missing required payload: $path" >&2
      return 3
    }
    [[ "$(stat -f '%l' "$BUNDLE/$path")" -eq 1 ]] || {
      echo "hard-linked payload is forbidden in official input bundle: $path" >&2
      return 3
    }
  done
  actual_count="$(find "$BUNDLE" -type f | wc -l | tr -d ' ')"
  [[ "$actual_count" -eq "$expected_count" ]] || {
    echo "official input bundle has undeclared payloads: expected=$expected_count actual=$actual_count" >&2
    (cd "$BUNDLE" && find . -type f -print | LC_ALL=C sort) >&2
    return 3
  }
}

verify_arm64_and_dependencies() {
  local file_path relative_path architectures dependency_path rpaths expected_rpaths mach_count=0
  while IFS= read -r file_path; do
    file "$file_path" | grep -q 'Mach-O' || continue
    mach_count=$((mach_count + 1))
    relative_path="${file_path#"$BUNDLE"/}"
    architectures="$(lipo -archs "$file_path")"
    [[ "$architectures" == "arm64" ]] || {
      echo "official input payload is not arm64-only: $relative_path ($architectures)" >&2
      return 3
    }
    if [[ "$relative_path" == Contents/MacOS/* ]]; then
      [[ -x "$file_path" ]] || { echo "non-executable Mach-O in Contents/MacOS: $relative_path" >&2; return 3; }
    fi
    while IFS= read -r dependency_path; do
      [[ -z "$dependency_path" ]] && continue
      case "$dependency_path" in
        */../*|*/./*|*/..|*/.)
          echo "path traversal in Mach-O dependency: $relative_path ($dependency_path)" >&2
          return 3
          ;;
      esac
      case "$dependency_path" in
        /System/Library/*|/usr/lib/*) ;;
        @rpath/librime.1.dylib)
          [[ "$relative_path" == "Contents/MacOS/AgentInputMethod" || \
             "$relative_path" == "Contents/MacOS/AgentIMEPersonalReplayHarness" || \
             "$relative_path" == "Contents/MacOS/AgentIMEUserDBCtl" || \
             "$relative_path" == "Contents/Frameworks/librime.1.dylib" ]] || {
            echo "undeclared librime dependency: $relative_path" >&2
            return 3
          }
          ;;
        *)
          echo "undeclared Mach-O dependency: $relative_path ($dependency_path)" >&2
          return 3
          ;;
      esac
    done < <(otool -L "$file_path" | tail -n +2 | awk '{print $1}')
    rpaths="$(otool -l "$file_path" | awk '
      $1 == "cmd" && $2 == "LC_RPATH" { in_rpath=1; next }
      in_rpath && $1 == "path" { print $2; in_rpath=0 }
    ' | LC_ALL=C sort)"
    if [[ "$relative_path" == "Contents/MacOS/AgentInputMethod" || \
          "$relative_path" == "Contents/MacOS/AgentIMEPersonalReplayHarness" || \
          "$relative_path" == "Contents/MacOS/AgentIMEUserDBCtl" ]]; then
      expected_rpaths=$'@executable_path/../Frameworks\n@loader_path'
      [[ "$rpaths" == "$expected_rpaths" ]] || {
        echo "official input host has a missing or undeclared RPATH: $rpaths" >&2
        return 3
      }
    elif [[ -n "$rpaths" ]]; then
      echo "undeclared RPATH in official input payload: $relative_path ($rpaths)" >&2
      return 3
    fi
  done < <(find "$BUNDLE/Contents" -type f -print | LC_ALL=C sort)
  [[ "$mach_count" -gt 0 ]] || { echo "official input bundle contains no Mach-O payload" >&2; return 3; }
}

verify_signing_identity() {
  local expected_team="$1" expected_authority="$2" file_path team authority
  while IFS= read -r file_path; do
    file "$file_path" | grep -q 'Mach-O' || continue
    codesign --verify --strict "$file_path"
    team="$(signature_value "$file_path" TeamIdentifier)"
    authority="$(signature_value "$file_path" Authority)"
    [[ "$team" == "$expected_team" && "$authority" == "$expected_authority" ]] || {
      echo "nested Mach-O signing identity mismatch: $file_path" >&2
      return 3
    }
  done < <(find "$BUNDLE/Contents" -type f -print | LC_ALL=C sort)
}

# Reuse the strict pointer parser used by dogfood. Its validation is deliberately
# invoked before any installer directory, lock, process, or service operation.
/usr/bin/sandbox-exec -p '(version 1) (allow default) (deny network*)' \
  node --input-type=module - "$ROOT_DIR/scripts/dogfood_checkpoint.mjs" "$POINTER" <<'NODE'
import { pathToFileURL } from "node:url";
const modulePath = pathToFileURL(process.argv[2]).href;
const { inspectFrozenReleasePointer } = await import(modulePath);
inspectFrozenReleasePointer(process.argv[3]);
NODE

POINTER_SHA="$(awk '/^[0-9a-f]{64}  / { print $1; exit }' "$POINTER")"
POINTER_PATH="$(awk '/^[0-9a-f]{64}  / { print substr($0, 67); exit }' "$POINTER")"
[[ "$POINTER_PATH" == /* && -d "$POINTER_PATH" && ! -L "$POINTER_PATH" ]] || {
  echo "frozen RC pointer contains an invalid input artifact path" >&2
  exit 3
}
POINTER_PATH="$(cd "$POINTER_PATH" && pwd -P)"
[[ "$POINTER_PATH" == "$BUNDLE" ]] || { echo "input bundle is not the exact frozen RC pointer artifact" >&2; exit 3; }
for plugin_path in Contents/PlugIns Contents/Resources/Rime/plugins Contents/Resources/Rime/rime-plugins; do
  [[ ! -e "$BUNDLE/$plugin_path" && ! -L "$BUNDLE/$plugin_path" ]] || {
    echo "official input bundle must not contain a plugin directory: $plugin_path" >&2
    exit 3
  }
done

verify_exact_inventory \
  Contents/Frameworks/librime.1.dylib \
  Contents/Info.plist \
  Contents/MacOS/AgentIMEInputSourceCtl \
  Contents/MacOS/AgentIMEPersonalReplayHarness \
  Contents/MacOS/AgentIMEUserDBCtl \
  Contents/MacOS/AgentInputMethod \
  Contents/PkgInfo \
  Contents/Resources/AgentInputMethod.icns \
  Contents/Resources/AgentInputMethodMenu.pdf \
  Contents/Resources/ContextDisambig.json \
  Contents/Resources/Licenses/OpenCC-Apache-2.0.txt \
  Contents/Resources/Licenses/RapidJSON-LICENSE.txt \
  Contents/Resources/Licenses/Rust-standard-library-Apache-2.0.txt \
  Contents/Resources/Licenses/Rust-standard-library-COPYRIGHT.txt \
  Contents/Resources/Licenses/Rust-standard-library-MIT.txt \
  Contents/Resources/Licenses/boost-BSL-1.0.txt \
  Contents/Resources/Licenses/darts-clone-COPYING.txt \
  Contents/Resources/Licenses/glog-BSD-3-Clause.txt \
  Contents/Resources/Licenses/leveldb-BSD-3-Clause.txt \
  Contents/Resources/Licenses/librime-BSD-3-Clause.txt \
  Contents/Resources/Licenses/marisa-trie-COPYING.txt \
  Contents/Resources/Licenses/rime-pinyin-simp-Apache-2.0.txt \
  Contents/Resources/Licenses/rime-pinyin-simp-AUTHORS.txt \
  Contents/Resources/Licenses/THUOCL-MIT.txt \
  Contents/Resources/Licenses/jieba-MIT.txt \
  Contents/Resources/Licenses/pypinyin-MIT.txt \
  Contents/Resources/Licenses/yaml-cpp-MIT.txt \
  Contents/Resources/Rime/agent_common_fix.dict.yaml \
  Contents/Resources/Rime/agent_daily.dict.yaml \
  Contents/Resources/Rime/agent_english.dict.yaml \
  Contents/Resources/Rime/agent_project_terms.dict.yaml \
  Contents/Resources/Rime/agent_tech_terms.dict.yaml \
  Contents/Resources/Rime/agent_ai_terms.dict.yaml \
  Contents/Resources/Rime/agent_pack_terms.dict.yaml \
  Contents/Resources/Rime/agent_idioms.dict.yaml \
  Contents/Resources/Rime/agent_thuocl_it.dict.yaml \
  Contents/Resources/Rime/agent_thuocl_caijing.dict.yaml \
  Contents/Resources/Rime/agent_modern.dict.yaml \
  Contents/Resources/Rime/agent_general.dict.yaml \
  Contents/Resources/Rime/agent_newwords.dict.yaml \
  Contents/Resources/Rime/agent_spoken.dict.yaml \
  Contents/Resources/Rime/agent_ai_names.dict.yaml \
  Contents/Resources/Rime/agent_pack_poetry.dict.yaml \
  Contents/Resources/Rime/agent_pack_cad.dict.yaml \
  Contents/Resources/Rime/agent_pack_industry.dict.yaml \
  Contents/Resources/Rime/agent_exact_readings.tsv \
  Contents/Resources/Rime/agent_pinyin.dict.yaml \
  Contents/Resources/Rime/agent_pinyin.schema.yaml \
  Contents/Resources/Rime/default.yaml \
  Contents/Resources/Rime/pinyin_simp.dict.yaml \
  Contents/Resources/THIRD_PARTY_NOTICES.md \
  Contents/Resources/en.lproj/InfoPlist.strings \
  Contents/Resources/zh-Hans.lproj/InfoPlist.strings \
  Contents/_CodeSignature/CodeResources
verify_arm64_and_dependencies

VERSION="$(plist_value "$BUNDLE" CFBundleShortVersionString)"
BUILD="$(plist_value "$BUNDLE" CFBundleVersion)"
BUNDLE_ID="$(plist_value "$BUNDLE" CFBundleIdentifier)"
PRODUCT_COMMIT="$(plist_value "$BUNDLE" AgentIMEProductCommit)"
PRODUCT_INPUTS="$(plist_value "$BUNDLE" AgentIMEProductInputsSHA256)"
CONFIGURATION="$(plist_value "$BUNDLE" AgentIMEBuildConfiguration)"
CONNECTION_NAME="$(plist_value "$BUNDLE" InputMethodConnectionName)"
CONTROLLER_CLASS="$(plist_value "$BUNDLE" InputMethodServerControllerClass)"
DELEGATE_CLASS="$(plist_value "$BUNDLE" InputMethodServerDelegateClass)"
BACKGROUND_ONLY="$(plist_value "$BUNDLE" LSBackgroundOnly)"
MINIMUM_SYSTEM="$(plist_value "$BUNDLE" LSMinimumSystemVersion)"
TIS_VISIBLE_MODE_ID="$(plist_value "$BUNDLE" 'ComponentInputModeDict:tsVisibleInputModeOrderedArrayKey:0')"
TIS_MODE_DEFAULT="$(plist_value "$BUNDLE" 'ComponentInputModeDict:tsInputModeListKey:com.hongye.facai.inputmethod.pinyin:tsInputModeDefaultStateKey')"
TIS_MODE_VISIBLE="$(plist_value "$BUNDLE" 'ComponentInputModeDict:tsInputModeListKey:com.hongye.facai.inputmethod.pinyin:tsInputModeIsVisibleKey')"
# Like Sogou and Squirrel, the mode names its input source explicitly (2026-10-13: without it the source was only visible to
# the system for a short time after being registered and the menu list lost it).
TIS_SOURCE_ID="$(plist_value "$BUNDLE" 'ComponentInputModeDict:tsInputModeListKey:com.hongye.facai.inputmethod.pinyin:TISInputSourceID')"
[[ "$TIS_SOURCE_ID" == "com.hongye.facai.inputmethod.zhizi.pinyin" ]] || { echo "input mode lacks the explicit TISInputSourceID" >&2; exit 3; }
TIS_MODE_INVENTORY="$({
  plutil -convert json -o - "$BUNDLE/Contents/Info.plist" \
    | node --input-type=module -e '
        import fs from "node:fs";
        const plist = JSON.parse(fs.readFileSync(0, "utf8"));
        const modes = plist?.ComponentInputModeDict?.tsInputModeListKey;
        const visible = plist?.ComponentInputModeDict?.tsVisibleInputModeOrderedArrayKey;
        if (!modes || Array.isArray(modes) || typeof modes !== "object" || !Array.isArray(visible)) {
          process.exit(3);
        }
        process.stdout.write(`${Object.keys(modes).sort().join(",")}|${visible.join(",")}`);
      '
} 2>/dev/null)" || { echo "official input mode inventory is invalid" >&2; exit 3; }
TEAM_ID="$(signature_value "$BUNDLE" TeamIdentifier)"
AUTHORITY="$(signature_value "$BUNDLE" Authority)"
TEAM_ID_SHA256="$(printf '%s' "$TEAM_ID" | text_checksum)"
AUTHORITY_SHA256="$(printf '%s' "$AUTHORITY" | text_checksum)"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"

[[ "$VERSION" == "$PRODUCT_VERSION" ]] || { echo "official input version is not $PRODUCT_VERSION" >&2; exit 3; }
[[ "$BUILD" =~ ^[1-9][0-9]{0,8}$ ]] && (( 10#$BUILD >= MINIMUM_BUILD )) || {
  echo "official input build must be >= $MINIMUM_BUILD: $BUILD" >&2
  exit 3
}
[[ "$BUNDLE_ID" == "com.hongye.facai.inputmethod.zhizi" ]] || { echo "official input bundle ID is invalid" >&2; exit 3; }
[[ "$PRODUCT_COMMIT" =~ ^[0-9a-f]{40}$ ]] || { echo "official input ProductCommit is invalid" >&2; exit 3; }
[[ "$PRODUCT_INPUTS" =~ ^[0-9a-f]{64}$ ]] || { echo "official input ProductInputsSHA256 is invalid" >&2; exit 3; }
[[ "$CONFIGURATION" == "release" ]] || { echo "official input is not a release build" >&2; exit 3; }
[[ "$CONNECTION_NAME" == "com.hongye.facai.inputmethod.zhizi_Connection" ]] \
  || { echo "official input connection name is invalid" >&2; exit 3; }
[[ "$CONTROLLER_CLASS" == "AgentInputController" && "$DELEGATE_CLASS" == "AgentInputController" ]] \
  || { echo "official InputMethodKit controller metadata is invalid" >&2; exit 3; }
[[ "$BACKGROUND_ONLY" == "true" && "$MINIMUM_SYSTEM" == "14.0" ]] \
  || { echo "official InputMethodKit process metadata is invalid" >&2; exit 3; }
[[ "$TIS_MODE_INVENTORY" == \
   "com.hongye.facai.inputmethod.pinyin|com.hongye.facai.inputmethod.pinyin" ]] \
  || { echo "official input must declare exactly one visible pinyin mode" >&2; exit 3; }
[[ "$TIS_VISIBLE_MODE_ID" == "com.hongye.facai.inputmethod.pinyin" && \
   "$TIS_MODE_DEFAULT" == "true" && \
   "$TIS_MODE_VISIBLE" == "true" ]] \
  || { echo "official input pinyin mode metadata is invalid" >&2; exit 3; }
plutil -convert json -o - "$BUNDLE/Contents/Info.plist" \
  | node --input-type=module -e '
      import fs from "node:fs";
      const plist = JSON.parse(fs.readFileSync(0, "utf8"));
      const mode = plist.ComponentInputModeDict.tsInputModeListKey["com.hongye.facai.inputmethod.pinyin"];
      const forbiddenTopLevel = ["TISInputSourceID", "TISInputSourceLanguages"];
      const forbiddenMode = ["TISInputSourceID", "tsInputModeCharacterRepertoireKey"];
      if (forbiddenTopLevel.some((key) => Object.hasOwn(plist, key)) ||
          forbiddenMode.some((key) => Object.hasOwn(mode, key))) {
        process.exit(3);
      }
    ' \
  || { echo "official input must let TIS derive parent and mode identifiers" >&2; exit 3; }
[[ "$TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || { echo "official input TeamIdentifier is invalid" >&2; exit 3; }
if [[ -n "${AIM_AUTHORIZED_TEAM_ID-}" ]]; then
  [[ "${AIM_AUTHORIZED_TEAM_ID}" =~ ^[A-Z0-9]{10}$ && "${AIM_AUTHORIZED_TEAM_ID}" == "$TEAM_ID" ]] \
    || { echo "AIM_AUTHORIZED_TEAM_ID does not match the frozen RC TeamIdentifier" >&2; exit 3; }
fi
[[ "$AUTHORITY" == "Apple Development:"* ]] || { echo "official input is missing an Apple Development leaf authority" >&2; exit 3; }
[[ "$ARTIFACT_SHA" == "$POINTER_SHA" ]] || { echo "official input checksum differs from frozen RC pointer" >&2; exit 3; }
[[ "$(awk -F= '$1 == "# artifact_version" { print $2; exit }' "$POINTER")" == "$VERSION" ]]
[[ "$(awk -F= '$1 == "# artifact_build" { print $2; exit }' "$POINTER")" == "$BUILD" ]]
[[ "$(awk -F= '$1 == "# product_commit" { print $2; exit }' "$POINTER")" == "$PRODUCT_COMMIT" ]]
[[ "$(awk -F= '$1 == "# product_inputs_sha256" { print $2; exit }' "$POINTER")" == "$PRODUCT_INPUTS" ]]
[[ "$(awk -F= '$1 == "# team_identifier_sha256" { print $2; exit }' "$POINTER")" == "$TEAM_ID_SHA256" ]]
[[ "$(awk -F= '$1 == "# leaf_authority_sha256" { print $2; exit }' "$POINTER")" == "$AUTHORITY_SHA256" ]]
codesign --verify --deep --strict "$BUNDLE"
verify_signing_identity "$TEAM_ID" "$AUTHORITY"
[[ -z "$(find "$BUNDLE" \
  \( -perm -0200 -o -perm -0020 -o -perm -0002 \) -print -quit)" ]] || {
  echo "official frozen input bundle contains writable mode bits" >&2
  exit 3
}
while IFS= read -r -d '' frozen_entry; do
  [[ ! -w "$frozen_entry" ]] || {
    echo "official frozen input bundle is writable through effective permissions" >&2
    exit 3
  }
done < <(find "$BUNDLE" -print0)
[[ "$(shasum -a 256 "$POINTER" | awk '{ print $1 }')" == "$POINTER_FILE_SHA_BEFORE" ]] || {
  echo "frozen RC pointer changed during validation" >&2
  exit 3
}

printf 'official_input_validation=PASS checksum=%s build=%s product_commit=%s product_inputs_sha256=%s team_identifier_sha256=%s leaf_authority_sha256=%s\n' \
  "$ARTIFACT_SHA" "$BUILD" "$PRODUCT_COMMIT" "$PRODUCT_INPUTS" "$TEAM_ID_SHA256" "$AUTHORITY_SHA256"
