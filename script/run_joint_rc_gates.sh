#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
PRODUCT_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
INPUT_GUARD="$ROOT_DIR/script/rc_gate_support.sh"
FREEZE_TOOL="$ROOT_DIR/script/freeze_joint_rc.sh"
EVIDENCE="$ROOT_DIR/docs/current/evidence/v0.2-v0.3"
MINIMUM_OFFICIAL_STRESS_SECONDS=1800

usage() {
  echo "usage: $0 <frozen-inputmethod-bundle> <frozen-manager-bundle>" >&2
  echo "       $0 <frozen-inputmethod-bundle> <stress-seconds> <frozen-manager-bundle>" >&2
  exit 2
}

validate_stress_seconds() {
  local source="$1"
  local value="$2"
  [[ "$value" =~ ^[1-9][0-9]{0,8}$ ]] || {
    echo "$source stress duration must be a positive integer of at most 9 digits: $value" >&2
    return 2
  }
  (( 10#$value >= MINIMUM_OFFICIAL_STRESS_SECONDS )) || {
    echo "$source stress duration is below the official ${MINIMUM_OFFICIAL_STRESS_SECONDS}s minimum: $value" >&2
    return 2
  }
}

[[ $# -eq 2 || $# -eq 3 ]] || usage
BUNDLE="$1"
if [[ $# -eq 2 ]]; then
  STRESS_ARG_PRESENT=""
  STRESS_ARG_VALUE=""
  MANAGER_BUNDLE="$2"
else
  STRESS_ARG_PRESENT=1
  STRESS_ARG_VALUE="$2"
  MANAGER_BUNDLE="$3"
fi
STRESS_ENV_PRESENT="${STRESS_SECONDS+x}"
STRESS_ENV_VALUE="${STRESS_SECONDS-}"
if [[ -n "$STRESS_ENV_PRESENT" ]]; then
  validate_stress_seconds "environment" "$STRESS_ENV_VALUE"
fi
if [[ -n "$STRESS_ARG_PRESENT" ]]; then
  validate_stress_seconds "argument" "$STRESS_ARG_VALUE"
fi
if [[ -n "$STRESS_ENV_PRESENT" && -n "$STRESS_ARG_PRESENT" \
  && "$STRESS_ENV_VALUE" != "$STRESS_ARG_VALUE" ]]; then
  echo "ambiguous stress duration: environment=$STRESS_ENV_VALUE argument=$STRESS_ARG_VALUE" >&2
  exit 2
fi
STRESS_SECONDS_REQUESTED="${STRESS_ARG_VALUE:-${STRESS_ENV_VALUE:-$MINIMUM_OFFICIAL_STRESS_SECONDS}}"
validate_stress_seconds "effective" "$STRESS_SECONDS_REQUESTED"
if [[ -n "${AIM_PERFORMANCE_OFFICIAL+x}" && "${AIM_PERFORMANCE_OFFICIAL}" != "1" ]]; then
  echo "AIM_PERFORMANCE_OFFICIAL may only be unset or 1 for a joint RC gate" >&2
  exit 2
fi

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing or symlinked frozen RC bundle: $BUNDLE" >&2; exit 2; }
[[ -d "$MANAGER_BUNDLE" && ! -L "$MANAGER_BUNDLE" ]] || { echo "missing or symlinked frozen RC manager: $MANAGER_BUNDLE" >&2; exit 2; }
BUNDLE="$(cd "$(dirname "$BUNDLE")" && pwd -P)/$(basename "$BUNDLE")"
MANAGER_BUNDLE="$(cd "$(dirname "$MANAGER_BUNDLE")" && pwd -P)/$(basename "$MANAGER_BUNDLE")"
cd "$ROOT_DIR"
RIME="$BUNDLE/Contents/Resources/Rime"
HOST="$BUNDLE/Contents/MacOS/AgentInputMethod"
LIBRARY="$BUNDLE/Contents/Frameworks"
[[ -x "$HOST" && -d "$RIME" && -d "$LIBRARY" ]] || { echo "incomplete frozen RC input bundle" >&2; exit 2; }
[[ -x "$INPUT_GUARD" ]] || { echo "missing RC input guard: $INPUT_GUARD" >&2; exit 2; }
[[ -x "$FREEZE_TOOL" ]] || { echo "missing RC freeze verifier: $FREEZE_TOOL" >&2; exit 2; }
"$FREEZE_TOOL" --verify-frozen "$BUNDLE" "$MANAGER_BUNDLE" >/dev/null

plist_value() {
  /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null
}

required_plist_value() {
  local bundle="$1"
  local key="$2"
  local value
  value="$(plist_value "$bundle" "$key")" || {
    echo "missing required $key in bundle: $bundle" >&2
    return 3
  }
  [[ -n "$value" ]] || {
    echo "empty required $key in bundle: $bundle" >&2
    return 3
  }
  printf '%s\n' "$value"
}

signature_value() {
  local bundle="$1"
  local key="$2"
  codesign -dv --verbose=4 "$bundle" 2>&1 | awk -F= -v key="$key" '
    $1 == key && !found { value=substr($0, length(key) + 2); found=1 }
    END { if (found) print value; else exit 3 }
  '
}

sha256_text() {
  /usr/bin/shasum -a 256 | /usr/bin/awk '{ print $1 }'
}

verify_exact_file_inventory() {
  local bundle="$1"
  local label="$2"
  shift 2
  local expected_count="$#"
  local actual_count relative_path
  if [[ -n "$(find "$bundle" ! -type d ! -type f -print -quit)" ]]; then
    echo "$label RC bundle contains a symlink or special filesystem entry" >&2
    return 3
  fi
  for relative_path in "$@"; do
    [[ -f "$bundle/$relative_path" && ! -L "$bundle/$relative_path" ]] || {
      echo "$label RC bundle is missing required payload: $relative_path" >&2
      return 3
    }
  done
  actual_count="$(find "$bundle" -type f | wc -l | tr -d ' ')"
  [[ "$actual_count" =~ ^[0-9]+$ && "$actual_count" -eq "$expected_count" ]] || {
    echo "$label RC bundle has undeclared payloads: expected=$expected_count actual=$actual_count" >&2
    (cd "$bundle" && find . -type f -print | LC_ALL=C sort) >&2
    return 3
  }
}

verify_arm64_only() {
  local bundle="$1"
  local mach_count=0
  local file_path relative_path architectures dependency_path rpaths expected_rpaths
  while IFS= read -r file_path; do
    if file "$file_path" | grep -q 'Mach-O'; then
      mach_count=$((mach_count + 1))
      relative_path="${file_path#"$bundle"/}"
      if [[ "$file_path" == "$bundle/Contents/MacOS/"* && ! -x "$file_path" ]]; then
        echo "non-executable Mach-O payload in Contents/MacOS: $file_path" >&2
        return 3
      fi
      architectures="$(lipo -archs "$file_path")"
      [[ "$architectures" == "arm64" ]] || {
        echo "non-arm64-only Mach-O in RC bundle: $file_path ($architectures)" >&2
        return 3
      }
      while IFS= read -r dependency_path; do
        [[ -z "$dependency_path" ]] && continue
        case "$dependency_path" in
          */../*|*/./*|*/..|*/.)
            echo "path traversal is forbidden in RC Mach-O dependency: $relative_path ($dependency_path)" >&2
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
              echo "librime dependency is undeclared for RC payload: $relative_path" >&2
              return 3
            }
            ;;
          *)
            echo "undeclared absolute or tokenized dependency in RC Mach-O: $relative_path ($dependency_path)" >&2
            return 3
            ;;
        esac
      done < <(otool -L "$file_path" | tail -n +2 | awk '{ print $1 }')
      rpaths="$(otool -l "$file_path" | awk '
        $1 == "cmd" && $2 == "LC_RPATH" { in_rpath=1; next }
        in_rpath && $1 == "path" { print $2; in_rpath=0 }
      ' | LC_ALL=C sort)"
      if [[ "$relative_path" == "Contents/MacOS/AgentInputMethod" || \
            "$relative_path" == "Contents/MacOS/AgentIMEPersonalReplayHarness" || \
            "$relative_path" == "Contents/MacOS/AgentIMEUserDBCtl" ]]; then
        expected_rpaths=$'@executable_path/../Frameworks\n@loader_path'
        [[ "$rpaths" == "$expected_rpaths" ]] || {
          echo "input host has a missing or undeclared RPATH: $rpaths" >&2
          return 3
        }
      elif [[ -n "$rpaths" ]]; then
        echo "undeclared RPATH in RC Mach-O: $relative_path ($rpaths)" >&2
        return 3
      fi
    fi
  done < <(find "$bundle/Contents" -type f 2>/dev/null | LC_ALL=C sort)
  [[ "$mach_count" -gt 0 ]] || {
    echo "RC bundle contains no Mach-O payload: $bundle" >&2
    return 3
  }
}

verify_nested_identity() {
  local bundle="$1"
  local expected_team="$2"
  local expected_authority="$3"
  local file_path team authority
  while IFS= read -r file_path; do
    file "$file_path" | grep -q 'Mach-O' || continue
    codesign --verify --strict "$file_path"
    team="$(signature_value "$file_path" TeamIdentifier)"
    authority="$(signature_value "$file_path" Authority)"
    [[ "$team" == "$expected_team" && "$authority" == "$expected_authority" ]] || {
      echo "nested Mach-O signing identity mismatch: $file_path" >&2
      return 3
    }
  done < <(find "$bundle/Contents" -type f -print | LC_ALL=C sort)
}

codesign --verify --deep --strict "$BUNDLE"
codesign --verify --deep --strict "$MANAGER_BUNDLE"
for plugin_path in Contents/PlugIns Contents/Resources/Rime/plugins Contents/Resources/Rime/rime-plugins; do
  [[ ! -e "$BUNDLE/$plugin_path" && ! -L "$BUNDLE/$plugin_path" ]] || {
    echo "input RC must not contain a plugin directory: $plugin_path" >&2
    exit 3
  }
done
verify_exact_file_inventory "$BUNDLE" input \
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
verify_exact_file_inventory "$MANAGER_BUNDLE" manager \
  Contents/Info.plist \
  Contents/MacOS/agent-input-layer \
  Contents/MacOS/snapshot-fixture-ctl \
  Contents/Resources/_up_/THIRD_PARTY_NOTICES.md \
  Contents/Resources/_up_/docs/current/licenses/DEPENDENCY_REGISTRY.md \
  Contents/Resources/_up_/docs/current/licenses/LOCKED_DEPENDENCIES.json \
  Contents/Resources/_up_/docs/current/licenses/NATIVE_LICENSE_ASSETS.json \
  Contents/Resources/icon.icns \
  Contents/_CodeSignature/CodeResources
verify_arm64_only "$BUNDLE"
verify_arm64_only "$MANAGER_BUNDLE"

ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
MANAGER_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$MANAGER_BUNDLE")"
[[ "$ARTIFACT_SHA" =~ ^[0-9a-f]{64}$ && "$MANAGER_SHA" =~ ^[0-9a-f]{64}$ ]]
INPUT_VERSION="$(required_plist_value "$BUNDLE" CFBundleShortVersionString)"
MANAGER_VERSION="$(required_plist_value "$MANAGER_BUNDLE" CFBundleShortVersionString)"
INPUT_BUILD="$(required_plist_value "$BUNDLE" CFBundleVersion)"
MANAGER_BUILD="$(required_plist_value "$MANAGER_BUNDLE" CFBundleVersion)"
INPUT_BUNDLE_ID="$(required_plist_value "$BUNDLE" CFBundleIdentifier)"
MANAGER_BUNDLE_ID="$(required_plist_value "$MANAGER_BUNDLE" CFBundleIdentifier)"
INPUT_COMMIT="$(required_plist_value "$BUNDLE" AgentIMEProductCommit)"
MANAGER_COMMIT="$(required_plist_value "$MANAGER_BUNDLE" AgentIMEProductCommit)"
INPUT_TREE_SHA="$(required_plist_value "$BUNDLE" AgentIMEProductInputsSHA256)"
MANAGER_TREE_SHA="$(required_plist_value "$MANAGER_BUNDLE" AgentIMEProductInputsSHA256)"
INPUT_CONFIGURATION="$(required_plist_value "$BUNDLE" AgentIMEBuildConfiguration)"
MANAGER_CONFIGURATION="$(required_plist_value "$MANAGER_BUNDLE" AgentIMEBuildConfiguration)"
INPUT_TEAM="$(signature_value "$BUNDLE" TeamIdentifier)"
MANAGER_TEAM="$(signature_value "$MANAGER_BUNDLE" TeamIdentifier)"
INPUT_AUTHORITY="$(signature_value "$BUNDLE" Authority)"
MANAGER_AUTHORITY="$(signature_value "$MANAGER_BUNDLE" Authority)"
INPUT_TEAM_SHA256="$(printf '%s' "$INPUT_TEAM" | sha256_text)"
INPUT_AUTHORITY_SHA256="$(printf '%s' "$INPUT_AUTHORITY" | sha256_text)"
FROZEN_TEAM_SHA256="$(awk -F= '$1 == "team_identifier_sha256" { count += 1; value = $2 } END { if (count == 1) print value; else exit 3 }' \
  "$(dirname "$BUNDLE")/FROZEN_ARTIFACTS")"
FROZEN_AUTHORITY_SHA256="$(awk -F= '$1 == "leaf_authority_sha256" { count += 1; value = $2 } END { if (count == 1) print value; else exit 3 }' \
  "$(dirname "$BUNDLE")/FROZEN_ARTIFACTS")"

[[ "$INPUT_VERSION" == "$PRODUCT_VERSION" ]] || { echo "unexpected input version: $INPUT_VERSION" >&2; exit 3; }
[[ "$MANAGER_VERSION" == "$INPUT_VERSION" ]] || { echo "manager/input version mismatch" >&2; exit 3; }
[[ "$INPUT_BUILD" =~ ^[1-9][0-9]{0,8}$ && "$MANAGER_BUILD" =~ ^[1-9][0-9]{0,8}$ ]] || {
  echo "invalid RC build number" >&2
  exit 3
}
(( 10#$INPUT_BUILD >= 1020 )) || { echo "joint RC build must be >= 1020: $INPUT_BUILD" >&2; exit 3; }
[[ "$MANAGER_BUILD" == "$INPUT_BUILD" ]] || { echo "manager/input build mismatch" >&2; exit 3; }
[[ "$INPUT_BUNDLE_ID" == "com.hongye.facai.inputmethod.zhizi" ]] || { echo "unexpected input bundle id: $INPUT_BUNDLE_ID" >&2; exit 3; }
[[ "$MANAGER_BUNDLE_ID" == "com.agentinputlayer.desktop" ]] || { echo "unexpected manager bundle id: $MANAGER_BUNDLE_ID" >&2; exit 3; }
[[ "$MANAGER_COMMIT" == "$INPUT_COMMIT" && "$INPUT_COMMIT" =~ ^[0-9a-f]{40}$ ]] || { echo "manager/input ProductCommit mismatch" >&2; exit 3; }
[[ "$MANAGER_TREE_SHA" == "$INPUT_TREE_SHA" && "$INPUT_TREE_SHA" =~ ^[0-9a-f]{64}$ ]] || { echo "manager/input ProductInputsSHA256 mismatch" >&2; exit 3; }
[[ "$INPUT_CONFIGURATION" == "release" && "$MANAGER_CONFIGURATION" == "release" ]] || { echo "joint RC bundles must both be release builds" >&2; exit 3; }
[[ "$INPUT_TEAM" =~ ^[A-Z0-9]{10}$ && "$MANAGER_TEAM" == "$INPUT_TEAM" ]] \
  || { echo "input/manager TeamIdentifier is invalid or mismatched" >&2; exit 3; }
[[ "$FROZEN_TEAM_SHA256" =~ ^[0-9a-f]{64}$ \
  && "$FROZEN_AUTHORITY_SHA256" =~ ^[0-9a-f]{64}$ \
  && "$INPUT_TEAM_SHA256" == "$FROZEN_TEAM_SHA256" \
  && "$INPUT_AUTHORITY_SHA256" == "$FROZEN_AUTHORITY_SHA256" ]] \
  || { echo "frozen metadata signing identity hashes are invalid or mismatched" >&2; exit 3; }
if [[ -n "${AIM_AUTHORIZED_TEAM_ID-}" ]]; then
  [[ "${AIM_AUTHORIZED_TEAM_ID}" =~ ^[A-Z0-9]{10}$ && "${AIM_AUTHORIZED_TEAM_ID}" == "$INPUT_TEAM" ]] \
    || { echo "AIM_AUTHORIZED_TEAM_ID does not match the frozen RC TeamIdentifier" >&2; exit 3; }
fi
[[ "$INPUT_AUTHORITY" == "Apple Development:"* && "$MANAGER_AUTHORITY" == "$INPUT_AUTHORITY" ]] || { echo "missing, ad-hoc, or mismatched Apple Development leaf Authority" >&2; exit 3; }
verify_nested_identity "$BUNDLE" "$INPUT_TEAM" "$INPUT_AUTHORITY"
verify_nested_identity "$MANAGER_BUNDLE" "$INPUT_TEAM" "$INPUT_AUTHORITY"

[[ "$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$INPUT_COMMIT")" == "$INPUT_COMMIT" ]]
EXPECTED_TREE_SHA="$("$INPUT_GUARD" fingerprint "$ROOT_DIR" "$INPUT_COMMIT")"
[[ "$EXPECTED_TREE_SHA" == "$INPUT_TREE_SHA" ]] || { echo "bundle ProductInputsSHA256 does not match ProductCommit" >&2; exit 3; }

[[ ! -L "$EVIDENCE" ]] || { echo "evidence root may not be a symlink" >&2; exit 3; }
[[ ! -e "$EVIDENCE" || -d "$EVIDENCE" ]] || { echo "evidence root must be a directory" >&2; exit 3; }
for evidence_path in \
  "$EVIDENCE/V03-01" "$EVIDENCE/V03-01/artifacts" \
  "$EVIDENCE/V03-02" "$EVIDENCE/V03-02/artifacts" \
  "$EVIDENCE/V03-04" "$EVIDENCE/V03-04/artifacts" \
  "$EVIDENCE/V03-06" "$EVIDENCE/V03-06/artifacts"; do
  [[ ! -L "$evidence_path" ]] || {
    echo "formal evidence path may not be a symlink: $evidence_path" >&2
    exit 3
  }
  [[ ! -e "$evidence_path" || -d "$evidence_path" ]] || {
    echo "formal evidence path must be a directory: $evidence_path" >&2
    exit 3
  }
  if [[ -d "$evidence_path" && -n "$(find "$evidence_path" -type l -print -quit)" ]]; then
    echo "symlink is forbidden in formal evidence: $evidence_path" >&2
    exit 3
  fi
done
existing_pointer="$EVIDENCE/RC_ARTIFACT.sha256"
validate_existing_pointer_monotonic() {
  local existing_build=""
  local existing_bundle_path=""
  local existing_input_sha=""
  local existing_manager_sha=""
  if [[ -e "$existing_pointer" || -L "$existing_pointer" ]]; then
    [[ -f "$existing_pointer" && ! -L "$existing_pointer" ]] || {
      echo "existing RC pointer must be a regular non-symlink file" >&2
      exit 3
    }
    existing_build="$(awk -F= '$1 == "# artifact_build" { print $2; exit }' "$existing_pointer")"
    if [[ -z "$existing_build" ]]; then
      existing_bundle_path="$(awk '/^[0-9a-f]{64}  / { print substr($0, 67); exit }' "$existing_pointer")"
      if [[ -n "$existing_bundle_path" && -f "$existing_bundle_path/Contents/Info.plist" ]]; then
        existing_build="$(plist_value "$existing_bundle_path" CFBundleVersion)"
      fi
    fi
    [[ "$existing_build" =~ ^[1-9][0-9]{0,8}$ ]] || { echo "existing RC pointer has no verifiable build" >&2; exit 3; }
    if (( 10#$existing_build > 10#$INPUT_BUILD )); then
      echo "RC build rollback refused: existing=$existing_build requested=$INPUT_BUILD" >&2
      exit 3
    fi
    if [[ "$existing_build" == "$INPUT_BUILD" ]]; then
      existing_input_sha="$(awk '/^[0-9a-f]{64}  / { print $1; exit }' "$existing_pointer")"
      existing_manager_sha="$(awk '/^[0-9a-f]{64}  / { count += 1; if (count == 2) { print $1; exit } }' "$existing_pointer")"
      [[ "$existing_input_sha" == "$ARTIFACT_SHA" && "$existing_manager_sha" == "$MANAGER_SHA" ]] || {
        echo "same RC build number may only rerun the identical input/manager checksum pair" >&2
        exit 3
      }
    fi
  fi
}
validate_existing_pointer_monotonic

mkdir -p "$EVIDENCE"
GATE_LOCK="$EVIDENCE/.joint-rc.lock"
if [[ -e "$GATE_LOCK" || -L "$GATE_LOCK" ]] || ! mkdir "$GATE_LOCK" 2>/dev/null; then
  echo "another joint RC gate is active, or its lock requires explicit recovery: $GATE_LOCK" >&2
  exit 3
fi
if ! printf '%s\n' "$$" > "$GATE_LOCK/pid"; then
  rmdir "$GATE_LOCK" 2>/dev/null || true
  exit 3
fi
release_gate_lock() {
  local owner=""
  if [[ ! -e "$GATE_LOCK" && ! -L "$GATE_LOCK" ]]; then
    return 0
  fi
  [[ -d "$GATE_LOCK" && ! -L "$GATE_LOCK" && -f "$GATE_LOCK/pid" && ! -L "$GATE_LOCK/pid" ]] || return 1
  IFS= read -r owner < "$GATE_LOCK/pid" || return 1
  [[ "$owner" == "$$" ]] || return 1
  rm -f "$GATE_LOCK/pid" && rmdir "$GATE_LOCK"
}
lock_only_cleanup() {
  local exit_status="$?"
  trap - EXIT
  trap '' HUP INT TERM
  release_gate_lock || exit_status=92
  exit "$exit_status"
}
trap lock_only_cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
# The first check is a fast preflight. This second read occurs while holding the
# publication lease and closes the window where another gate could commit a
# newer pointer between preflight and lock acquisition.
validate_existing_pointer_monotonic

EVIDENCE_TASKS=(V03-01 V03-02 V03-04 V03-06)
EVIDENCE_BASELINE_HASHES=()
for task in "${EVIDENCE_TASKS[@]}"; do
  artifact_dir="$EVIDENCE/$task/artifacts"
  if [[ -d "$artifact_dir" ]]; then
    manifest_path="$artifact_dir/MANIFEST.sha256"
    if [[ -e "$manifest_path" || -L "$manifest_path" ]]; then
      [[ -f "$manifest_path" && ! -L "$manifest_path" ]] || {
        echo "formal evidence manifest must be a regular non-symlink file: $manifest_path" >&2
        exit 3
      }
    fi
    EVIDENCE_BASELINE_HASHES+=("$("$ROOT_DIR/script/artifact_checksum.sh" "$artifact_dir")")
  else
    EVIDENCE_BASELINE_HASHES+=(ABSENT)
  fi
done

RUNTIME_ROOT=""
RUNTIME_ROOT_INODE=""
STAGE_ROOT=""
remove_runtime_root() {
  if [[ -z "$RUNTIME_ROOT" || (! -e "$RUNTIME_ROOT" && ! -L "$RUNTIME_ROOT") ]]; then
    return 0
  fi
  [[ "$RUNTIME_ROOT" == /tmp/agent-ime-joint-rc.* \
    && -d "$RUNTIME_ROOT" && ! -L "$RUNTIME_ROOT" \
    && -n "$RUNTIME_ROOT_INODE" \
    && "$(stat -f %i "$RUNTIME_ROOT" 2>/dev/null || true)" == "$RUNTIME_ROOT_INODE" ]] || return 1
  chmod -R u+w "$RUNTIME_ROOT" || return 1
  rm -rf "$RUNTIME_ROOT"
}
early_cleanup() {
  local exit_status="$?"
  trap - EXIT
  trap '' HUP INT TERM
  remove_runtime_root || exit_status=92
  [[ -n "$STAGE_ROOT" && "$STAGE_ROOT" == "$EVIDENCE"/.staging-joint-rc.* ]] && rm -rf "$STAGE_ROOT"
  release_gate_lock || exit_status=92
  exit "$exit_status"
}
trap early_cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

RUNTIME_ROOT="$(mktemp -d /tmp/agent-ime-joint-rc.XXXXXX)"
RUNTIME_ROOT_INODE="$(stat -f %i "$RUNTIME_ROOT")"
STAGE_ROOT="$(mktemp -d "$EVIDENCE/.staging-joint-rc.XXXXXX")"
[[ "$(stat -f %d "$ROOT_DIR")" == "$(stat -f %d "$EVIDENCE")" && \
   "$(stat -f %d "$EVIDENCE")" == "$(stat -f %d "$STAGE_ROOT")" ]] || {
  echo "formal evidence staging is not on the repository evidence filesystem" >&2
  exit 3
}
RESULTS="$STAGE_ROOT/results"
mkdir -p "$RESULTS/V03-01" "$RESULTS/V03-02" "$RESULTS/V03-04" "$RESULTS/V03-06"

# Running a signed executable directly from the frozen bundle can cause macOS
# to attach a com.apple.macl access-control xattr to the bundle root even when
# every mode bit is read-only. The bytes and code signature do not change, but
# the frozen-tree metadata contract correctly rejects that mutation. Execute
# only from a canonical temporary copy whose checksum and signature are bound
# to the already validated frozen artifact; the frozen source remains a
# read-only evidence input and is reverified before publication and commit.
EXECUTION_BUNDLE="$RUNTIME_ROOT/Agent Input Method.inputmethod"
EXECUTION_METADATA="$RUNTIME_ROOT/FROZEN_ARTIFACTS"
/usr/bin/ditto --norsrc --noextattr --noqtn --noacl "$BUNDLE" "$EXECUTION_BUNDLE"
/bin/cp "$(dirname "$BUNDLE")/FROZEN_ARTIFACTS" "$EXECUTION_METADATA"
/bin/chmod -R u+w "$EXECUTION_BUNDLE"
/bin/chmod u+w "$EXECUTION_METADATA"
/bin/chmod -RN "$EXECUTION_BUNDLE" "$EXECUTION_METADATA"
/usr/bin/xattr -cr "$EXECUTION_BUNDLE" "$EXECUTION_METADATA"
/bin/chmod -R a-w "$EXECUTION_BUNDLE"
/bin/chmod a-w "$EXECUTION_METADATA"
/usr/bin/codesign --verify --deep --strict "$EXECUTION_BUNDLE"
[[ "$("$ROOT_DIR/script/artifact_checksum.sh" "$EXECUTION_BUNDLE")" == "$ARTIFACT_SHA" ]]
/usr/bin/cmp -s "$(dirname "$BUNDLE")/FROZEN_ARTIFACTS" "$EXECUTION_METADATA"
RIME="$EXECUTION_BUNDLE/Contents/Resources/Rime"
HOST="$EXECUTION_BUNDLE/Contents/MacOS/AgentInputMethod"
LIBRARY="$EXECUTION_BUNDLE/Contents/Frameworks"
[[ -x "$HOST" && -d "$RIME" && -d "$LIBRARY" ]] || {
  echo "incomplete frozen RC execution copy" >&2
  exit 3
}

assert_same_file() {
  local label="$1"
  local expected="$2"
  local actual="$3"
  [[ -f "$expected" && ! -L "$expected" && -f "$actual" && ! -L "$actual" ]] || {
    echo "missing or symlinked $label binding input" >&2
    return 3
  }
  cmp -s "$expected" "$actual" || {
    echo "$label in signed RC does not match ProductCommit/pinned input" >&2
    return 3
  }
}

PINNED_RIME_DIR="$ROOT_DIR/.build-deps/librime/1.16.1"
PINNED_PINYIN_DIR="$ROOT_DIR/.build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0"
assert_same_file dictionary "$PINNED_PINYIN_DIR/pinyin_simp.dict.yaml" "$RIME/pinyin_simp.dict.yaml"
assert_same_file default-schema "$ROOT_DIR/native/resources/Rime/default.yaml" "$RIME/default.yaml"
assert_same_file agent-schema "$ROOT_DIR/native/resources/Rime/agent_pinyin.schema.yaml" "$RIME/agent_pinyin.schema.yaml"
assert_same_file agent-dictionary "$ROOT_DIR/native/resources/Rime/agent_pinyin.dict.yaml" "$RIME/agent_pinyin.dict.yaml"
assert_same_file agent-common-fix-dictionary "$ROOT_DIR/native/resources/Rime/agent_common_fix.dict.yaml" "$RIME/agent_common_fix.dict.yaml"
assert_same_file agent-daily-dictionary "$ROOT_DIR/native/resources/Rime/agent_daily.dict.yaml" "$RIME/agent_daily.dict.yaml"
assert_same_file agent-english-dictionary "$ROOT_DIR/native/resources/Rime/agent_english.dict.yaml" "$RIME/agent_english.dict.yaml"
assert_same_file agent-project-terms-dictionary "$ROOT_DIR/native/resources/Rime/agent_project_terms.dict.yaml" "$RIME/agent_project_terms.dict.yaml"
assert_same_file agent-tech-terms-dictionary "$ROOT_DIR/native/resources/Rime/agent_tech_terms.dict.yaml" "$RIME/agent_tech_terms.dict.yaml"
assert_same_file agent-tech-terms-dictionary "$ROOT_DIR/native/resources/Rime/agent_ai_terms.dict.yaml" "$RIME/agent_ai_terms.dict.yaml"
assert_same_file agent-pack-terms-dictionary "$ROOT_DIR/native/resources/Rime/agent_pack_terms.dict.yaml" "$RIME/agent_pack_terms.dict.yaml"
assert_same_file agent-idiom-dictionary "$ROOT_DIR/native/resources/Rime/agent_idioms.dict.yaml" "$RIME/agent_idioms.dict.yaml"
assert_same_file agent-thuocl-it-dictionary "$ROOT_DIR/native/resources/Rime/agent_thuocl_it.dict.yaml" "$RIME/agent_thuocl_it.dict.yaml"
assert_same_file agent-thuocl-caijing-dictionary "$ROOT_DIR/native/resources/Rime/agent_thuocl_caijing.dict.yaml" "$RIME/agent_thuocl_caijing.dict.yaml"
assert_same_file agent-modern-dictionary "$ROOT_DIR/native/resources/Rime/agent_modern.dict.yaml" "$RIME/agent_modern.dict.yaml"
assert_same_file agent-general-dictionary "$ROOT_DIR/native/resources/Rime/agent_general.dict.yaml" "$RIME/agent_general.dict.yaml"
assert_same_file agent-newwords-dictionary "$ROOT_DIR/native/resources/Rime/agent_newwords.dict.yaml" "$RIME/agent_newwords.dict.yaml"
assert_same_file agent-spoken-dictionary "$ROOT_DIR/native/resources/Rime/agent_spoken.dict.yaml" "$RIME/agent_spoken.dict.yaml"
assert_same_file agent-ai-names-dictionary "$ROOT_DIR/native/resources/Rime/agent_ai_names.dict.yaml" "$RIME/agent_ai_names.dict.yaml"
assert_same_file agent-pack-poetry-dictionary "$ROOT_DIR/native/resources/Rime/agent_pack_poetry.dict.yaml" "$RIME/agent_pack_poetry.dict.yaml"
assert_same_file agent-pack-cad-dictionary "$ROOT_DIR/native/resources/Rime/agent_pack_cad.dict.yaml" "$RIME/agent_pack_cad.dict.yaml"
assert_same_file agent-pack-industry-dictionary "$ROOT_DIR/native/resources/Rime/agent_pack_industry.dict.yaml" "$RIME/agent_pack_industry.dict.yaml"
assert_same_file exact-reading-index "$ROOT_DIR/native/resources/Rime/agent_exact_readings.tsv" "$RIME/agent_exact_readings.tsv"
assert_same_file context-disambiguation-table "$ROOT_DIR/third_party/context-disambig/disambig-table-v1.json" "$BUNDLE/Contents/Resources/ContextDisambig.json"
assert_same_file input-notices "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$BUNDLE/Contents/Resources/THIRD_PARTY_NOTICES.md"
assert_same_file librime-license "$PINNED_RIME_DIR/licenses/LICENSE.librime" "$BUNDLE/Contents/Resources/Licenses/librime-BSD-3-Clause.txt"
assert_same_file pinyin-license "$PINNED_PINYIN_DIR/LICENSE" "$BUNDLE/Contents/Resources/Licenses/rime-pinyin-simp-Apache-2.0.txt"
assert_same_file pinyin-authors "$PINNED_PINYIN_DIR/AUTHORS" "$BUNDLE/Contents/Resources/Licenses/rime-pinyin-simp-AUTHORS.txt"
assert_same_file thuocl-license "$ROOT_DIR/third_party/licenses/THUOCL-MIT.txt" "$BUNDLE/Contents/Resources/Licenses/THUOCL-MIT.txt"
assert_same_file jieba-license "$ROOT_DIR/third_party/licenses/jieba-MIT.txt" "$BUNDLE/Contents/Resources/Licenses/jieba-MIT.txt"
assert_same_file pypinyin-license "$ROOT_DIR/third_party/licenses/pypinyin-MIT.txt" "$BUNDLE/Contents/Resources/Licenses/pypinyin-MIT.txt"
assert_same_file boost-license "$ROOT_DIR/third_party/licenses/boost-BSL-1.0.txt" "$BUNDLE/Contents/Resources/Licenses/boost-BSL-1.0.txt"
assert_same_file glog-license "$ROOT_DIR/third_party/licenses/glog-BSD-3-Clause.txt" "$BUNDLE/Contents/Resources/Licenses/glog-BSD-3-Clause.txt"
assert_same_file leveldb-license "$ROOT_DIR/third_party/licenses/leveldb-BSD-3-Clause.txt" "$BUNDLE/Contents/Resources/Licenses/leveldb-BSD-3-Clause.txt"
assert_same_file marisa-license "$ROOT_DIR/third_party/licenses/marisa-trie-COPYING.txt" "$BUNDLE/Contents/Resources/Licenses/marisa-trie-COPYING.txt"
assert_same_file opencc-license "$ROOT_DIR/third_party/licenses/OpenCC-Apache-2.0.txt" "$BUNDLE/Contents/Resources/Licenses/OpenCC-Apache-2.0.txt"
assert_same_file darts-license "$ROOT_DIR/third_party/licenses/darts-clone-COPYING.txt" "$BUNDLE/Contents/Resources/Licenses/darts-clone-COPYING.txt"
assert_same_file rapidjson-license "$ROOT_DIR/third_party/licenses/RapidJSON-LICENSE.txt" "$BUNDLE/Contents/Resources/Licenses/RapidJSON-LICENSE.txt"
assert_same_file yaml-cpp-license "$ROOT_DIR/third_party/licenses/yaml-cpp-MIT.txt" "$BUNDLE/Contents/Resources/Licenses/yaml-cpp-MIT.txt"
assert_same_file rust-apache-license "$ROOT_DIR/.build-deps/rust/1.94.1/licenses/LICENSE-APACHE" \
  "$BUNDLE/Contents/Resources/Licenses/Rust-standard-library-Apache-2.0.txt"
assert_same_file rust-mit-license "$ROOT_DIR/.build-deps/rust/1.94.1/licenses/LICENSE-MIT" \
  "$BUNDLE/Contents/Resources/Licenses/Rust-standard-library-MIT.txt"
assert_same_file rust-library-notices "$ROOT_DIR/.build-deps/rust/1.94.1/licenses/COPYRIGHT-library.html" \
  "$BUNDLE/Contents/Resources/Licenses/Rust-standard-library-COPYRIGHT.txt"
assert_same_file input-icon "$ROOT_DIR/native/macos/Resources/AgentInputMethod.icns" "$BUNDLE/Contents/Resources/AgentInputMethod.icns"
assert_same_file input-menu-icon "$ROOT_DIR/native/macos/Resources/AgentInputMethodMenu.pdf" "$BUNDLE/Contents/Resources/AgentInputMethodMenu.pdf"
assert_same_file input-strings-zh-hans "$ROOT_DIR/native/macos/Resources/zh-Hans.lproj/InfoPlist.strings" "$BUNDLE/Contents/Resources/zh-Hans.lproj/InfoPlist.strings"
assert_same_file input-strings-en "$ROOT_DIR/native/macos/Resources/en.lproj/InfoPlist.strings" "$BUNDLE/Contents/Resources/en.lproj/InfoPlist.strings"
assert_same_file manager-icon "$ROOT_DIR/src-tauri/icons/icon.icns" "$MANAGER_BUNDLE/Contents/Resources/icon.icns"
assert_same_file manager-notices "$ROOT_DIR/THIRD_PARTY_NOTICES.md" "$MANAGER_BUNDLE/Contents/Resources/_up_/THIRD_PARTY_NOTICES.md"
assert_same_file manager-native-registry "$ROOT_DIR/docs/current/licenses/DEPENDENCY_REGISTRY.md" \
  "$MANAGER_BUNDLE/Contents/Resources/_up_/docs/current/licenses/DEPENDENCY_REGISTRY.md"
assert_same_file manager-license-registry "$ROOT_DIR/docs/current/licenses/LOCKED_DEPENDENCIES.json" \
  "$MANAGER_BUNDLE/Contents/Resources/_up_/docs/current/licenses/LOCKED_DEPENDENCIES.json"
assert_same_file manager-native-license-assets "$ROOT_DIR/docs/current/licenses/NATIVE_LICENSE_ASSETS.json" \
  "$MANAGER_BUNDLE/Contents/Resources/_up_/docs/current/licenses/NATIVE_LICENSE_ASSETS.json"
cp "$PINNED_RIME_DIR/lib/librime.1.dylib" "$RUNTIME_ROOT/pinned-librime-unsigned.dylib"
cp "$BUNDLE/Contents/Frameworks/librime.1.dylib" "$RUNTIME_ROOT/bundled-librime-unsigned.dylib"
chmod u+w "$RUNTIME_ROOT/pinned-librime-unsigned.dylib" "$RUNTIME_ROOT/bundled-librime-unsigned.dylib"
codesign --remove-signature "$RUNTIME_ROOT/pinned-librime-unsigned.dylib" 2>/dev/null || true
codesign --remove-signature "$RUNTIME_ROOT/bundled-librime-unsigned.dylib" 2>/dev/null || true
assert_same_file librime-code "$RUNTIME_ROOT/pinned-librime-unsigned.dylib" "$RUNTIME_ROOT/bundled-librime-unsigned.dylib"

PUBLISHING=0
COMMITTED=0
PUBLISHED_TARGETS=()
PUBLISHED_BACKUPS=()
PUBLISHED_EXISTED=()
PUBLISH_TEMPS=()

rollback_publication() {
  local index target backup existed temp
  local failed=0
  for ((index=${#PUBLISHED_TARGETS[@]} - 1; index >= 0; index--)); do
    target="${PUBLISHED_TARGETS[$index]}"
    backup="${PUBLISHED_BACKUPS[$index]}"
    existed="${PUBLISHED_EXISTED[$index]}"
    if [[ "$existed" == "1" ]]; then
      if ! temp="$(mktemp "$(dirname "$target")/.rollback.$(basename "$target").XXXXXX")"; then
        failed=1
        continue
      fi
      if ! cp -p "$backup" "$temp" || ! mv -f "$temp" "$target"; then
        rm -f "$temp"
        failed=1
      fi
    else
      rm -f "$target" || failed=1
    fi
  done
  return "$failed"
}

cleanup() {
  local status=$?
  trap - EXIT
  trap '' HUP INT TERM
  if [[ "$COMMITTED" == "1" ]]; then
    local committed_temp
    if (( ${#PUBLISH_TEMPS[@]} > 0 )); then
      for committed_temp in "${PUBLISH_TEMPS[@]}"; do
        [[ -z "$committed_temp" ]] || rm -f "$committed_temp" || true
      done
    fi
    if ! remove_runtime_root; then
      printf 'warning: committed RC left a runtime directory requiring explicit recovery: %s\n' "$RUNTIME_ROOT" >&2 || true
    fi
    [[ "$STAGE_ROOT" == "$EVIDENCE"/.staging-joint-rc.* ]] && rm -rf "$STAGE_ROOT" || true
    if ! release_gate_lock; then
      printf 'warning: committed RC left a gate lock requiring explicit recovery: %s\n' "$GATE_LOCK" >&2 || true
    fi
    exit 0
  fi
  if [[ "$PUBLISHING" == "1" ]]; then
    rollback_publication || status=91
  fi
  local temp
  if (( ${#PUBLISH_TEMPS[@]} > 0 )); then
    for temp in "${PUBLISH_TEMPS[@]}"; do
      [[ -z "$temp" ]] || rm -f "$temp"
    done
  fi
  remove_runtime_root || status=92
  [[ "$STAGE_ROOT" == "$EVIDENCE"/.staging-joint-rc.* ]] && rm -rf "$STAGE_ROOT"
  release_gate_lock || status=92
  exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

OFFLINE_SANDBOX='(version 1) (allow default) (deny network*)'
RIME_DEP="$ROOT_DIR/.build-deps/librime/1.16.1/lib/librime.1.dylib"
PINYIN_DEP="$ROOT_DIR/.build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml"
[[ -f "$RIME_DEP" && -f "$PINYIN_DEP" ]] || {
  echo "pinned native dependencies must be bootstrapped before the offline joint gate" >&2
  exit 3
}
sandbox-exec -p "$OFFLINE_SANDBOX" "$ROOT_DIR/script/bootstrap_native.sh"
[[ "$(shasum -a 256 "$PINYIN_DEP" | awk '{print $1}')" == "$(shasum -a 256 "$RIME/pinyin_simp.dict.yaml" | awk '{print $1}')" ]] || {
  echo "bundle dictionary differs from the pinned offline build input" >&2
  exit 3
}

CARGO_TARGET_DIR="$RUNTIME_ROOT/cargo-target"
CARGO_HOME_DIR="$RUNTIME_ROOT/cargo-home"
CORE_LIBRARY_DIR="$CARGO_TARGET_DIR/release"
mkdir -p "$CARGO_HOME_DIR"
sandbox-exec -p "$OFFLINE_SANDBOX" env CARGO_HOME="$CARGO_HOME_DIR" \
  CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
  cargo build --locked --offline --manifest-path "$ROOT_DIR/native/core/Cargo.toml" --release --features librime
[[ -f "$CORE_LIBRARY_DIR/libagent_ime_core.a" && ! -L "$CORE_LIBRARY_DIR/libagent_ime_core.a" ]] || {
  echo "fresh isolated Rust core archive is missing" >&2
  exit 3
}

SWIFT_SCRATCH="$RUNTIME_ROOT/swift-build"
SWIFT_CACHE="$RUNTIME_ROOT/swift-cache"
SWIFT_CONFIG="$RUNTIME_ROOT/swift-config"
SWIFT_SECURITY="$RUNTIME_ROOT/swift-security"
SWIFT_ISOLATION_ARGS=(
  --package-path "$ROOT_DIR/native/macos"
  --scratch-path "$SWIFT_SCRATCH"
  --cache-path "$SWIFT_CACHE"
  --config-path "$SWIFT_CONFIG"
  --security-path "$SWIFT_SECURITY"
  --disable-sandbox
  --disable-automatic-resolution
  --skip-update
  --disable-prefetching
)
SWIFT_BUILD_ARGS=("${SWIFT_ISOLATION_ARGS[@]}" -c release)
sandbox-exec -p "$OFFLINE_SANDBOX" env CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
  AIM_CORE_LIBRARY_DIR="$CORE_LIBRARY_DIR" \
  swift package "${SWIFT_ISOLATION_ARGS[@]}" dump-package \
  > "$RUNTIME_ROOT/swift-package.json"
jq -e --arg core "$CORE_LIBRARY_DIR" '
  (.dependencies | length) == 0 and
  ([.targets[] | select(.name == "AgentIMECoreBridge") | .settings[]? |
    select(.tool == "linker") | .kind.unsafeFlags?._0[]?] | index($core)) != null and
  ([.targets[] | select(.name == "AgentIMECoreBridge") | .settings[]? |
    select(.tool == "linker") | .kind.unsafeFlags?._0[]? |
    select(type == "string" and contains("/native/core/target/"))] | length) == 0
' "$RUNTIME_ROOT/swift-package.json" >/dev/null || {
  echo "Swift package is not bound exclusively to the fresh isolated Rust core" >&2
  exit 3
}
sandbox-exec -p "$OFFLINE_SANDBOX" env CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
  AIM_CORE_LIBRARY_DIR="$CORE_LIBRARY_DIR" swift build "${SWIFT_BUILD_ARGS[@]}"
BIN="$(sandbox-exec -p "$OFFLINE_SANDBOX" env CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
  AIM_CORE_LIBRARY_DIR="$CORE_LIBRARY_DIR" swift build "${SWIFT_BUILD_ARGS[@]}" --show-bin-path)"
for harness in AgentIMEBehaviorHarness AgentIMEReplayHarness AgentIMEFuzzyHarness AgentIMEUserDBHarness AgentIMEPerformanceHarness; do
  [[ -x "$BIN/$harness" ]] || { echo "fresh release harness missing: $harness" >&2; exit 3; }
done

RUN_ID="$(date -u '+%Y%m%dT%H%M%SZ')-$$"
SHA_PREFIX="${ARTIFACT_SHA:0:12}"
ARTIFACT_PREFIX="rc-build${INPUT_BUILD}-${SHA_PREFIX}-${RUN_ID}"
BEHAVIOR_NAME="${ARTIFACT_PREFIX}-behavior-golden.txt"
BASE_NAME="${ARTIFACT_PREFIX}-base-500.json"
FUZZY_NAME="${ARTIFACT_PREFIX}-fuzzy-56.json"
USERDB_NAME="${ARTIFACT_PREFIX}-userdb.json"
USERDB_PROCESS_NAME="${ARTIFACT_PREFIX}-userdb-process-recovery.txt"
PERFORMANCE_NAME="${ARTIFACT_PREFIX}-performance-stress.json"
HARNESS_BUILD_NAME="${ARTIFACT_PREFIX}-harness-build.txt"

bind_json_report_metadata() {
  local report="$1"
  local metadata_temp
  jq -e --arg version "$PRODUCT_VERSION" --arg checksum "$ARTIFACT_SHA" '
    type == "object" and .artifactChecksum == $checksum and .artifactVersion == $version
  ' "$report" >/dev/null
  metadata_temp="$(mktemp "$RUNTIME_ROOT/report-metadata.XXXXXX")"
  jq \
    --arg checksum "$ARTIFACT_SHA" \
    --arg version "$INPUT_VERSION" \
    --arg build "$INPUT_BUILD" \
    --arg commit "$INPUT_COMMIT" \
    --arg inputs "$INPUT_TREE_SHA" \
    --arg run_id "$RUN_ID" \
    '. + {
      artifactChecksum: $checksum,
      artifactVersion: $version,
      artifactBuild: $build,
      productCommit: $commit,
      productInputsSHA256: $inputs,
      runId: $run_id
    }' "$report" > "$metadata_temp"
  mv -f "$metadata_temp" "$report"
}

BEHAVIOR_REPORT="$RESULTS/V03-01/$BEHAVIOR_NAME"
BEHAVIOR_RAW="$RUNTIME_ROOT/behavior-golden-real.txt"
sandbox-exec -p "$OFFLINE_SANDBOX" env DYLD_LIBRARY_PATH="$LIBRARY" \
  "$BIN/AgentIMEBehaviorHarness" \
  "$RIME" "$RUNTIME_ROOT/behavior-user" "$ROOT_DIR/quality/golden/behavior-v0.3.tsv" \
  "$BEHAVIOR_RAW" "$ARTIFACT_SHA"
{
  printf 'artifact_build=%s\nproduct_commit=%s\nproduct_inputs_sha256=%s\nrun_id=%s\n' \
    "$INPUT_BUILD" "$INPUT_COMMIT" "$INPUT_TREE_SHA" "$RUN_ID"
  cat "$BEHAVIOR_RAW"
  shasum -a 256 \
    "$BIN/AgentIMEBehaviorHarness" \
    "$ROOT_DIR/native/macos/Sources/AgentIMEBehaviorHarness/main.swift" \
    "$ROOT_DIR/native/core/agent-ime-core/src/state.rs" \
    "$ROOT_DIR/quality/golden/behavior-v0.3.tsv"
  sandbox-exec -p "$OFFLINE_SANDBOX" env CARGO_HOME="$CARGO_HOME_DIR" \
    CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR="$RUNTIME_ROOT/mock-supplement-cargo-target" \
    cargo test --release --locked --offline --manifest-path "$ROOT_DIR/native/core/Cargo.toml" \
      state::tests::frozen_behavior_golden_is_nonempty_complete_and_passing -- --exact
  printf 'supplemental_mock_behavior=PASS\n'
} > "$BEHAVIOR_REPORT" 2>&1
grep -qx "artifact_checksum=$ARTIFACT_SHA" "$BEHAVIOR_REPORT"
grep -qx "artifact_version=$PRODUCT_VERSION" "$BEHAVIOR_REPORT"
grep -qx 'execution_path=swift-coresession-c-abi-rust-state-librime' "$BEHAVIOR_REPORT"
grep -qx 'core_abi_version=6' "$BEHAVIOR_REPORT"
grep -qx 'real_decoder=librime' "$BEHAVIOR_REPORT"
grep -qx 'frozen_cases=29' "$BEHAVIOR_REPORT"
grep -qx 'semantic_cases_passed=29' "$BEHAVIOR_REPORT"
grep -qx 'semantic_cases_failed=0' "$BEHAVIOR_REPORT"
grep -qx 'semantic_pass_rate=1.000000' "$BEHAVIOR_REPORT"
grep -qx 'live_sessions_before=0' "$BEHAVIOR_REPORT"
grep -qx 'live_sessions_after=0' "$BEHAVIOR_REPORT"
grep -qx 'app_proof_required=true' "$BEHAVIOR_REPORT"
grep -qx 'app_proof_claimed=false' "$BEHAVIOR_REPORT"
grep -qx 'visible_runtime_gate=NOT_CLAIMED' "$BEHAVIOR_REPORT"
[[ "$(grep -Ec '^case\.[a-z0-9-]+=PASS detail=[a-z0-9.-]+$' "$BEHAVIOR_REPORT")" == 29 ]]
! grep -Eq '^case\.[a-z0-9-]+=FAIL ' "$BEHAVIOR_REPORT"
grep -qx 'behavior_semantic_gate=PASS' "$BEHAVIOR_REPORT"
grep -qx 'supplemental_mock_behavior=PASS' "$BEHAVIOR_REPORT"

sandbox-exec -p "$OFFLINE_SANDBOX" node "$ROOT_DIR/scripts/freeze_candidate_corpus.mjs" \
  > "$RUNTIME_ROOT/corpus-checksums.txt"
sandbox-exec -p "$OFFLINE_SANDBOX" node "$ROOT_DIR/scripts/validate_agent_daily_dictionary.mjs" \
  >> "$RUNTIME_ROOT/corpus-checksums.txt"
sandbox-exec -p "$OFFLINE_SANDBOX" "$ROOT_DIR/script/test_lexicon_l1_replay.sh" "$BUNDLE" "$BIN/AgentIMEReplayHarness" \
  > "$STAGE_ROOT/lexicon-l1.log" 2>&1
sandbox-exec -p "$OFFLINE_SANDBOX" node "$ROOT_DIR/scripts/validate_native_license_assets.mjs" \
  >> "$RUNTIME_ROOT/corpus-checksums.txt"
sandbox-exec -p "$OFFLINE_SANDBOX" env CARGO_NET_OFFLINE=true \
  node "$ROOT_DIR/scripts/generate_locked_dependency_registry.mjs" --check \
  >> "$RUNTIME_ROOT/corpus-checksums.txt"
BASE_REPORT="$RESULTS/V03-02/$BASE_NAME"
sandbox-exec -p "$OFFLINE_SANDBOX" env DYLD_LIBRARY_PATH="$LIBRARY" \
  "$BIN/AgentIMEReplayHarness" \
  "$RIME" "$RUNTIME_ROOT/base-user" "$ROOT_DIR/quality/corpus/base-500.json" \
  "$BASE_REPORT" 5 "$ARTIFACT_SHA"
bind_json_report_metadata "$BASE_REPORT"
jq -e --arg version "$PRODUCT_VERSION" --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .artifactVersion == $version and
  .candidateDeduplicationRule == "unicode-nfkc-trimmed-text+lowercase-ascii-reading-v1" and
  .candidatesMissingReadingKey == 0 and
  .cases == 500 and .repetitions == 5 and .sessions == 2500 and
  .attemptedSessions == 2500 and .createdSessions == 2500 and
  .executedSessions == 2500 and
  .top1Hits >= 425 and .top5Hits >= 485 and
  .mustPassTop1Cases == 100 and .mustPassTop1Hits == 100 and
  .boundaryCases == 50 and
  .abbreviationCases == 20 and .abbreviationTop5Hits >= 18 and
  .segmentationCases == 15 and .longPinyinCases == 15 and
  .top1Rate >= 0.85 and .top5Rate >= 0.97 and .mustPassTop1Rate == 1 and
  .abbreviationTop5Rate >= 0.90 and .stableCases == .cases and
  ((.failures | length) == (.cases - .top5Hits)) and
  all(.failures[]; .reason == "top5-miss") and
  (([.failures[].id] | unique | length) == (.failures | length))
' "$BASE_REPORT" >/dev/null
[[ -d "$RUNTIME_ROOT/base-user/pinyin_simp.userdb" && \
   ! -e "$RUNTIME_ROOT/base-user/agent_pinyin.userdb" && \
   ! -L "$RUNTIME_ROOT/base-user/pinyin_simp.userdb" ]] || {
  echo "aggregate dictionary changed the v0.3 UserDB name" >&2
  exit 3
}

FUZZY_REPORT="$RESULTS/V03-02/$FUZZY_NAME"
FUZZY_PARTS="$RUNTIME_ROOT/fuzzy-parts"
mkdir -p "$FUZZY_PARTS"
for group in zh-z ch-c sh-s n-l f-h an-ang en-eng in-ing; do
  group_user="$RUNTIME_ROOT/fuzzy-$group-user"
  group_report="$FUZZY_PARTS/$group.json"
  sandbox-exec -p "$OFFLINE_SANDBOX" env DYLD_LIBRARY_PATH="$LIBRARY" \
    "$BIN/AgentIMEFuzzyHarness" \
    "$RIME" "$group_user" "$ROOT_DIR/quality/corpus/fuzzy-56.json" \
    "$group_report" 5 "$group" "$ARTIFACT_SHA"
  jq -e --arg version "$PRODUCT_VERSION" --arg checksum "$ARTIFACT_SHA" --arg group "$group" '
    .artifactChecksum == $checksum and .artifactVersion == $version and
    .candidateDeduplicationRule == "unicode-nfkc-trimmed-text+lowercase-ascii-reading-v1" and
    .candidatesMissingReadingKey == 0 and
    .cases == 7 and .repetitions == 5 and .top5Hits == .cases and
    .isolationCases == 14 and .attemptedSessions == 105 and
    .createdSessions == .attemptedSessions and .executedSessions == .attemptedSessions and
    .stableCases == .cases and .top5Rate >= 0.90 and (.failures | length) == 0 and
    .isolationStableCases == .isolationCases and
    .disabledTop5Leaks == 0 and .otherMaskTop5Leaks == 0 and
    .existingSessionReloadChecks == 1 and .existingSessionReloadFailures == 0 and
    (.groups | length) == 1 and .groups[$group] >= 5
  ' "$group_report" >/dev/null
done
jq -s --arg version "$PRODUCT_VERSION" --arg checksum "$ARTIFACT_SHA" '
  {
    artifactVersion: $version, artifactChecksum: $checksum,
    candidateDeduplicationRule: (map(.candidateDeduplicationRule) | unique |
      if length == 1 then .[0] else "inconsistent" end),
    candidatesMissingReadingKey: (map(.candidatesMissingReadingKey) | add),
    cases: (map(.cases) | add), repetitions: 5,
    isolationCases: (map(.isolationCases) | add),
    attemptedSessions: (map(.attemptedSessions) | add),
    createdSessions: (map(.createdSessions) | add),
    executedSessions: (map(.executedSessions) | add),
    top5Hits: (map(.top5Hits) | add), stableCases: (map(.stableCases) | add),
    isolationStableCases: (map(.isolationStableCases) | add),
    disabledTop5Leaks: (map(.disabledTop5Leaks) | add),
    otherMaskTop5Leaks: (map(.otherMaskTop5Leaks) | add),
    existingSessionReloadChecks: (map(.existingSessionReloadChecks) | add),
    existingSessionReloadFailures: (map(.existingSessionReloadFailures) | add),
    groups: (reduce (map(.groups | to_entries[])[]) as $entry ({}; .[$entry.key] = $entry.value)),
    failures: (map(.failures) | add)
  } | .top5Rate = (.top5Hits / .cases)
' "$FUZZY_PARTS"/*.json > "$FUZZY_REPORT"
bind_json_report_metadata "$FUZZY_REPORT"
jq -e --arg version "$PRODUCT_VERSION" --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .artifactVersion == $version and
  .candidateDeduplicationRule == "unicode-nfkc-trimmed-text+lowercase-ascii-reading-v1" and
  .candidatesMissingReadingKey == 0 and
  .cases == 56 and .repetitions == 5 and .top5Hits == .cases and
  .isolationCases == 112 and .attemptedSessions == 840 and
  .createdSessions == .attemptedSessions and .executedSessions == .attemptedSessions and
  .stableCases == .cases and .top5Rate >= 0.90 and (.failures | length) == 0 and
  .isolationStableCases == .isolationCases and
  .disabledTop5Leaks == 0 and .otherMaskTop5Leaks == 0 and
  .existingSessionReloadChecks == 8 and .existingSessionReloadFailures == 0 and
  (.groups | length) == 8 and ([.groups[] >= 5] | all)
' "$FUZZY_REPORT" >/dev/null

USERDB_REPORT="$RESULTS/V03-04/$USERDB_NAME"
sandbox-exec -p "$OFFLINE_SANDBOX" env DYLD_LIBRARY_PATH="$LIBRARY" \
  "$BIN/AgentIMEUserDBHarness" \
  "$RIME" "$RUNTIME_ROOT/userdb-user" "$ARTIFACT_SHA" > "$USERDB_REPORT"
bind_json_report_metadata "$USERDB_REPORT"
jq -e --arg version "$PRODUCT_VERSION" --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .artifactVersion == $version and
  .trainingSelections >= 30 and .promotedTop1 == .target and
  .afterDeleteTop1 != .target and .continuousSelectionCommit == "你好" and
  (.userDBFiles | index("pinyin_simp.userdb")) != null
' "$USERDB_REPORT" >/dev/null

USERDB_PROCESS_REPORT="$RESULTS/V03-04/$USERDB_PROCESS_NAME"
{
  printf 'artifact_checksum=%s\nartifact_version=%s\nartifact_build=%s\nproduct_commit=%s\nproduct_inputs_sha256=%s\nrun_id=%s\n' \
    "$ARTIFACT_SHA" "$INPUT_VERSION" "$INPUT_BUILD" "$INPUT_COMMIT" "$INPUT_TREE_SHA" "$RUN_ID"
  sandbox-exec -p "$OFFLINE_SANDBOX" \
    "$ROOT_DIR/script/test_userdb_process_recovery.sh" "$EXECUTION_BUNDLE" "$ARTIFACT_SHA"
} > "$USERDB_PROCESS_REPORT" 2>&1
grep -q "^userdb_process_recovery=PASS mode=RELEASE_EVIDENCE artifact_checksum=$ARTIFACT_SHA " \
  "$USERDB_PROCESS_REPORT"
grep -q '^after_select_hook_calls=1 hook_state=stopped restart_inspect_selection_calls=0 crash_learning_count=0 explicit_retry_learning_count=1$' \
  "$USERDB_PROCESS_REPORT"
grep -q '^same_physical_event_replay_id_stable=true different_physical_event_ids_unique=true replay_diagnostic_count=1 ' \
  "$USERDB_PROCESS_REPORT"
grep -qx 'post_sync_crash_exit=137 post_sync_first_learning_count=1 same_event_replay_learning_count=1 different_event_learning_count=2 same_event_replay_select_hook_calls=0 zero_id_learning_count=0 product_name_select_hook_calls=0 ledger_directory_mode=700 ledger_file_mode=600' \
  "$USERDB_PROCESS_REPORT"
grep -qx 'pre_select_claim_crash_exit=137 pre_select_claim_hook_calls=1 same_event_replay_learning_count=0 different_event_learning_count=1 same_event_replay_select_hook_calls=0 same_event_replay_committed=true' \
  "$USERDB_PROCESS_REPORT"

PERFORMANCE_REPORT="$RESULTS/V03-06/$PERFORMANCE_NAME"
sandbox-exec -p "$OFFLINE_SANDBOX" env AIM_PERFORMANCE_OFFICIAL=1 \
  DYLD_LIBRARY_PATH="$LIBRARY" "$BIN/AgentIMEPerformanceHarness" \
  "$RIME" "$RUNTIME_ROOT/performance-user" "$ROOT_DIR/quality/corpus/base-500.json" \
  "$PERFORMANCE_REPORT" "$ARTIFACT_SHA" "$HOST" "$STRESS_SECONDS_REQUESTED" --official
bind_json_report_metadata "$PERFORMANCE_REPORT"
jq -e --arg version "$PRODUCT_VERSION" --arg checksum "$ARTIFACT_SHA" --argjson requested "$STRESS_SECONDS_REQUESTED" '
  .artifactChecksum == $checksum and .artifactVersion == $version and
  .buildType == "release" and .architecture == "arm64" and
  (.deviceModel | type == "string" and length > 0) and
  (.operatingSystem | type == "string" and length > 0) and
  .measurementPath == "IMKInputController.handle receipt before EventMapper through main-thread production CandidatePanel ViewModel application" and
  .mainThreadViewModelApplication == true and .windowPresentationDisabled == true and
  .windowLayoutExcludedByRegisteredEndpoint == true and
  .mainRunLoopDrainedBetweenSequences == true and
  .mainRunLoopAutoreleasePoolPerSequence == true and
  .resourceWindowSamplesAfterMainRunLoopDrain == true and
  (.resourceWindowSamplingRule | type == "string" and length > 0) and
  .resourceWindowRawSampleIntervalSeconds == 60 and
  .resourceWindowRawSamplesPerWindow == 5 and
  (.measurementRule | type == "string" and length > 0) and .warmupEvents == 1000 and
  .warmupElapsedSeconds >= 35 and
  .warmupMainRunLoopDrainedBetweenSequences == true and
  (.warmupResourceSamples | length) == 3 and
  .warmupResourceStabilizationSeconds >= 10 and
  (.warmBaselineRule | type == "string" and length > 0) and
  .resourceWindows[0].sample.residentKiB == (.warmupResourceSamples | map(.residentKiB) | sort | .[1]) and
  .resourceWindows[0].sample.fileDescriptors == (.warmupResourceSamples | map(.fileDescriptors) | sort | .[1]) and
  .resourceWindows[0].sample.threads == (.warmupResourceSamples | map(.threads) | sort | .[1]) and
  .official == true and .measurementMode == "official" and .officialGatePassed == true and
  .officialDurationGatePassed == true and .officialResourceWindowGatePassed == true and
  .runPassed == true and .stressSecondsRequested == $requested and
  .stressSecondsRequested >= 1800 and .measuredStressElapsedSeconds >= .stressSecondsRequested and
  (.resourceWindows | length) >= 7 and
  .resourceWindows[-1].elapsedSeconds >= $requested and
  .resourceWindows[-2].elapsedSeconds < $requested and
  ([.resourceWindows[1:-1][] | (.rawSamples | length)] | all(. == 5)) and
  ((.resourceWindows[-1].rawSamples | length) >= 1) and
  ((.resourceWindows[-1].rawSamples | length) <= 5) and
  .sequences >= 10000 and .keyEvents >= 100000 and
  .latencySamples == .keyEvents and .verifiedCommits > 0 and .crossSessionChecks > 0 and
  .insertTextLatencySamples > 0 and
  (.maximumInsertTextLatencyMilliseconds | type) == "number" and
  (.insertTextP50Milliseconds | type) == "number" and
  (.insertTextP95Milliseconds | type) == "number" and
  (.insertTextP99Milliseconds | type) == "number" and
  .insertTextP50Milliseconds >= 0 and
  .insertTextP50Milliseconds <= .insertTextP95Milliseconds and
  .insertTextP95Milliseconds <= .insertTextP99Milliseconds and
  .insertTextP99Milliseconds <= .maximumInsertTextLatencyMilliseconds and
  (.localCandidateQueryRule | type == "string" and length > 0) and
  .localCandidateQueryLatencySamples == .keyEvents and
  .localCandidateQueryP50Milliseconds <= .localCandidateQueryP95Milliseconds and
  .localCandidateQueryP95Milliseconds <= .localCandidateQueryP99Milliseconds and
  .localCandidateQueryP95Milliseconds <= 2.5 and
  .localCandidateQueryP99Milliseconds <= 5 and
  .candidateMixerEnabled == true and
  (.currentCandidatePipelineRule | type == "string" and length > 0) and
  .currentCandidatePipelineLatencySamples == .keyEvents and
  .currentCandidatePipelineP50Milliseconds <= .currentCandidatePipelineP95Milliseconds and
  .currentCandidatePipelineP95Milliseconds <= .currentCandidatePipelineP99Milliseconds and
  .currentCandidatePipelineP95Milliseconds <= 20 and
  .coldStartExecutionPath == "process-entry/non-nil-IMKServer/production-AgentInputController/activate/handle/marked+candidates/close" and
  .coldStartProductionController == true and .coldStartServerNonNil == true and
  .coldStartFirstEventConsumed == true and .coldStartMarkedTextApplied == true and
  .coldStartCandidateCountPositive == true and .coldStartLiveHandlesAfterZero == true and
  .coldStartTISConnectionRegistrationClaimed == false and
  .coldStartPreparationStatus == 0 and .coldStarts == 20 and
  (.coldStartMilliseconds | length) == 20 and .coldStartP95Milliseconds <= 500 and
  .p50Milliseconds <= 16 and .p95Milliseconds <= 35 and .p99Milliseconds <= 60 and
  .lifecycleSessions == 1000 and .lifecycleLiveHandlesBefore == 0 and
  .lifecycleLiveHandlesAfter == 0 and .resourceGatePassed == true and .failures == 0
' "$PERFORMANCE_REPORT" >/dev/null

HARNESS_BUILD_REPORT="$RESULTS/V03-06/$HARNESS_BUILD_NAME"
{
  printf 'artifact_checksum=%s\nmanager_checksum=%s\nartifact_version=%s\nartifact_build=%s\nproduct_commit=%s\nproduct_inputs_sha256=%s\nrun_id=%s\n' \
    "$ARTIFACT_SHA" "$MANAGER_SHA" "$INPUT_VERSION" "$INPUT_BUILD" "$INPUT_COMMIT" "$INPUT_TREE_SHA" "$RUN_ID"
  printf 'configuration=release\ncargo_flags=--release --locked --offline\n'
  printf 'input_build_configuration=%s\nmanager_build_configuration=%s\n' \
    "$INPUT_CONFIGURATION" "$MANAGER_CONFIGURATION"
  printf 'swift_flags=release fresh-scratch disable-automatic-resolution skip-update disable-prefetching disable-swiftpm-sandbox outer-deny-network\n'
  printf 'quality_execution=deny-network\n'
  printf 'swift_external_dependency_count=0\ncargo_home=%s\ncargo_target_dir=%s\ncore_library_dir=%s\nbehavior_mock_supplement_target_dir=%s\nharness_build_path=%s\n' \
    "$CARGO_HOME_DIR" "$CARGO_TARGET_DIR" "$CORE_LIBRARY_DIR" \
    "$RUNTIME_ROOT/mock-supplement-cargo-target" "$BIN"
  shasum -a 256 "$CORE_LIBRARY_DIR/libagent_ime_core.a"
  shasum -a 256 "$BIN/AgentIMEBehaviorHarness" \
    "$BIN/AgentIMEReplayHarness" "$BIN/AgentIMEFuzzyHarness" \
    "$BIN/AgentIMEUserDBHarness" "$BIN/AgentIMEPerformanceHarness"
  shasum -a 256 "$PINNED_RIME_DIR/lib/librime.1.dylib" \
    "$BUNDLE/Contents/Frameworks/librime.1.dylib" \
    "$RUNTIME_ROOT/pinned-librime-unsigned.dylib" \
    "$RUNTIME_ROOT/bundled-librime-unsigned.dylib" \
    "$ROOT_DIR/native/resources/Rime/default.yaml" "$RIME/default.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_pinyin.schema.yaml" "$RIME/agent_pinyin.schema.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_pinyin.dict.yaml" "$RIME/agent_pinyin.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_common_fix.dict.yaml" "$RIME/agent_common_fix.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_daily.dict.yaml" "$RIME/agent_daily.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_english.dict.yaml" "$RIME/agent_english.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_project_terms.dict.yaml" "$RIME/agent_project_terms.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_tech_terms.dict.yaml" "$RIME/agent_tech_terms.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_ai_terms.dict.yaml" "$RIME/agent_ai_terms.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_pack_terms.dict.yaml" "$RIME/agent_pack_terms.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_idioms.dict.yaml" "$RIME/agent_idioms.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_thuocl_it.dict.yaml" "$RIME/agent_thuocl_it.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_thuocl_caijing.dict.yaml" "$RIME/agent_thuocl_caijing.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_modern.dict.yaml" "$RIME/agent_modern.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_general.dict.yaml" "$RIME/agent_general.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_newwords.dict.yaml" "$RIME/agent_newwords.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_spoken.dict.yaml" "$RIME/agent_spoken.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_ai_names.dict.yaml" "$RIME/agent_ai_names.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_pack_poetry.dict.yaml" "$RIME/agent_pack_poetry.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_pack_cad.dict.yaml" "$RIME/agent_pack_cad.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_pack_industry.dict.yaml" "$RIME/agent_pack_industry.dict.yaml" \
    "$ROOT_DIR/native/resources/Rime/agent_exact_readings.tsv" "$RIME/agent_exact_readings.tsv" \
    "$ROOT_DIR/third_party/context-disambig/disambig-table-v1.json" "$BUNDLE/Contents/Resources/ContextDisambig.json" \
    "$PINNED_PINYIN_DIR/pinyin_simp.dict.yaml" "$RIME/pinyin_simp.dict.yaml" \
    "$ROOT_DIR/THIRD_PARTY_NOTICES.md" \
    "$BUNDLE/Contents/Resources/THIRD_PARTY_NOTICES.md" \
    "$MANAGER_BUNDLE/Contents/Resources/_up_/THIRD_PARTY_NOTICES.md" \
    "$PINNED_RIME_DIR/licenses/LICENSE.librime" \
    "$BUNDLE/Contents/Resources/Licenses/librime-BSD-3-Clause.txt" \
    "$PINNED_PINYIN_DIR/LICENSE" \
    "$BUNDLE/Contents/Resources/Licenses/rime-pinyin-simp-Apache-2.0.txt" \
    "$PINNED_PINYIN_DIR/AUTHORS" \
    "$BUNDLE/Contents/Resources/Licenses/rime-pinyin-simp-AUTHORS.txt" \
    "$ROOT_DIR/third_party/licenses/THUOCL-MIT.txt" \
    "$ROOT_DIR/third_party/licenses/jieba-MIT.txt" \
    "$ROOT_DIR/third_party/licenses/pypinyin-MIT.txt" \
    "$BUNDLE/Contents/Resources/Licenses/THUOCL-MIT.txt" \
    "$BUNDLE/Contents/Resources/Licenses/jieba-MIT.txt" \
    "$BUNDLE/Contents/Resources/Licenses/pypinyin-MIT.txt" \
    "$ROOT_DIR/third_party/licenses/boost-BSL-1.0.txt" \
    "$BUNDLE/Contents/Resources/Licenses/boost-BSL-1.0.txt" \
    "$ROOT_DIR/third_party/licenses/glog-BSD-3-Clause.txt" \
    "$BUNDLE/Contents/Resources/Licenses/glog-BSD-3-Clause.txt" \
    "$ROOT_DIR/third_party/licenses/leveldb-BSD-3-Clause.txt" \
    "$BUNDLE/Contents/Resources/Licenses/leveldb-BSD-3-Clause.txt" \
    "$ROOT_DIR/third_party/licenses/marisa-trie-COPYING.txt" \
    "$BUNDLE/Contents/Resources/Licenses/marisa-trie-COPYING.txt" \
    "$ROOT_DIR/third_party/licenses/OpenCC-Apache-2.0.txt" \
    "$BUNDLE/Contents/Resources/Licenses/OpenCC-Apache-2.0.txt" \
    "$ROOT_DIR/third_party/licenses/darts-clone-COPYING.txt" \
    "$BUNDLE/Contents/Resources/Licenses/darts-clone-COPYING.txt" \
    "$ROOT_DIR/third_party/licenses/RapidJSON-LICENSE.txt" \
    "$BUNDLE/Contents/Resources/Licenses/RapidJSON-LICENSE.txt" \
    "$ROOT_DIR/.build-deps/rust/1.94.1/licenses/LICENSE-APACHE" \
    "$BUNDLE/Contents/Resources/Licenses/Rust-standard-library-Apache-2.0.txt" \
    "$ROOT_DIR/.build-deps/rust/1.94.1/licenses/LICENSE-MIT" \
    "$BUNDLE/Contents/Resources/Licenses/Rust-standard-library-MIT.txt" \
    "$ROOT_DIR/.build-deps/rust/1.94.1/licenses/COPYRIGHT-library.html" \
    "$BUNDLE/Contents/Resources/Licenses/Rust-standard-library-COPYRIGHT.txt" \
    "$ROOT_DIR/third_party/licenses/yaml-cpp-MIT.txt" \
    "$BUNDLE/Contents/Resources/Licenses/yaml-cpp-MIT.txt" \
    "$ROOT_DIR/docs/current/licenses/LOCKED_DEPENDENCIES.json" \
    "$MANAGER_BUNDLE/Contents/Resources/_up_/docs/current/licenses/LOCKED_DEPENDENCIES.json" \
    "$ROOT_DIR/docs/current/licenses/DEPENDENCY_REGISTRY.md" \
    "$MANAGER_BUNDLE/Contents/Resources/_up_/docs/current/licenses/DEPENDENCY_REGISTRY.md" \
    "$ROOT_DIR/docs/current/licenses/NATIVE_LICENSE_ASSETS.json" \
    "$MANAGER_BUNDLE/Contents/Resources/_up_/docs/current/licenses/NATIVE_LICENSE_ASSETS.json" \
    "$ROOT_DIR/native/macos/Resources/AgentInputMethod.icns" \
    "$BUNDLE/Contents/Resources/AgentInputMethod.icns" \
    "$ROOT_DIR/native/macos/Resources/AgentInputMethodMenu.pdf" \
    "$BUNDLE/Contents/Resources/AgentInputMethodMenu.pdf" \
    "$ROOT_DIR/src-tauri/icons/icon.icns" \
    "$MANAGER_BUNDLE/Contents/Resources/icon.icns"
  cat "$RUNTIME_ROOT/corpus-checksums.txt"
} > "$HARNESS_BUILD_REPORT"

[[ "$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$INPUT_COMMIT")" == "$INPUT_COMMIT" ]]
[[ "$("$INPUT_GUARD" fingerprint "$ROOT_DIR" "$INPUT_COMMIT")" == "$INPUT_TREE_SHA" ]]
codesign --verify --deep --strict "$BUNDLE"
codesign --verify --deep --strict "$MANAGER_BUNDLE"
codesign --verify --deep --strict "$EXECUTION_BUNDLE"
[[ "$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")" == "$ARTIFACT_SHA" ]]
[[ "$("$ROOT_DIR/script/artifact_checksum.sh" "$MANAGER_BUNDLE")" == "$MANAGER_SHA" ]]
[[ "$("$ROOT_DIR/script/artifact_checksum.sh" "$EXECUTION_BUNDLE")" == "$ARTIFACT_SHA" ]]
"$FREEZE_TOOL" --verify-frozen "$BUNDLE" "$MANAGER_BUNDLE" >/dev/null

for ((task_index=0; task_index<${#EVIDENCE_TASKS[@]}; task_index++)); do
  task="${EVIDENCE_TASKS[$task_index]}"
  artifact_dir="$EVIDENCE/$task/artifacts"
  baseline_hash="${EVIDENCE_BASELINE_HASHES[$task_index]}"
  if [[ "$baseline_hash" == "ABSENT" ]]; then
    [[ ! -e "$artifact_dir" && ! -L "$artifact_dir" ]] || {
      echo "formal evidence changed during the joint RC run: $task" >&2
      exit 3
    }
  else
    [[ -d "$artifact_dir" && ! -L "$artifact_dir" ]] || {
      echo "formal evidence disappeared during the joint RC run: $task" >&2
      exit 3
    }
    [[ "$("$ROOT_DIR/script/artifact_checksum.sh" "$artifact_dir")" == "$baseline_hash" ]] || {
      echo "formal evidence changed during the joint RC run: $task" >&2
      exit 3
    }
  fi
done

PUBLISH_TREE="$STAGE_ROOT/publish-tree"
for task in "${EVIDENCE_TASKS[@]}"; do
  official_dir="$EVIDENCE/$task/artifacts"
  staged_dir="$PUBLISH_TREE/$task/artifacts"
  mkdir -p "$staged_dir"
  if [[ -d "$official_dir" ]]; then
    [[ -z "$(find "$official_dir" -type l -print -quit)" ]] || {
      echo "symlink is forbidden in formal evidence artifacts: $official_dir" >&2
      exit 3
    }
    ditto "$official_dir" "$staged_dir"
  fi
  cp -p "$RESULTS/$task"/* "$staged_dir/"
  (
    cd "$staged_dir"
    find . -type f ! -path './MANIFEST.sha256' -print | LC_ALL=C sort | while IFS= read -r file_path; do
      shasum -a 256 "$file_path"
    done
  ) > "$staged_dir/MANIFEST.sha256"
  (cd "$staged_dir" && shasum -a 256 -c MANIFEST.sha256 >/dev/null)
done

POINTER_STAGE="$STAGE_ROOT/RC_ARTIFACT.sha256"
{
  printf '# artifact_version=%s\n# artifact_build=%s\n# product_commit=%s\n# product_inputs_sha256=%s\n' \
    "$INPUT_VERSION" "$INPUT_BUILD" "$INPUT_COMMIT" "$INPUT_TREE_SHA"
  printf '# team_identifier_sha256=%s\n# leaf_authority_sha256=%s\n# run_id=%s\n' \
    "$INPUT_TEAM_SHA256" "$INPUT_AUTHORITY_SHA256" "$RUN_ID"
  printf '# pointer_report_set_sha256=%s\n' "$(printf '0%.0s' {1..64})"
  printf '# behavior_report=V03-01/artifacts/%s\n' "$BEHAVIOR_NAME"
  printf '# base_report=V03-02/artifacts/%s\n# fuzzy_report=V03-02/artifacts/%s\n' "$BASE_NAME" "$FUZZY_NAME"
  printf '# userdb_report=V03-04/artifacts/%s\n' "$USERDB_NAME"
  printf '# userdb_process_report=V03-04/artifacts/%s\n' "$USERDB_PROCESS_NAME"
  printf '# performance_report=V03-06/artifacts/%s\n# harness_build_report=V03-06/artifacts/%s\n' \
    "$PERFORMANCE_NAME" "$HARNESS_BUILD_NAME"
  printf '%s  %s\n%s  %s\n' "$ARTIFACT_SHA" "$BUNDLE" "$MANAGER_SHA" "$MANAGER_BUNDLE"
} > "$POINTER_STAGE"

atomic_publish() {
  local source="$1"
  local target="$2"
  local backup existed temp index
  mkdir -p "$(dirname "$target")"
  if [[ -e "$target" || -L "$target" ]]; then
    [[ -f "$target" && ! -L "$target" ]] || {
      echo "publication target must be a regular non-symlink file: $target" >&2
      return 3
    }
  fi
  index="${#PUBLISHED_TARGETS[@]}"
  backup="$STAGE_ROOT/backups/$index"
  mkdir -p "$(dirname "$backup")"
  if [[ -f "$target" ]]; then
    cp -p "$target" "$backup"
    existed=1
  else
    existed=0
  fi
  PUBLISHED_TARGETS+=("$target")
  PUBLISHED_BACKUPS+=("$backup")
  PUBLISHED_EXISTED+=("$existed")
  temp="$(mktemp "$(dirname "$target")/.joint-rc.$(basename "$target").XXXXXX")"
  PUBLISH_TEMPS+=("$temp")
  cp -p "$source" "$temp"
  [[ "$(stat -f %d "$temp")" == "$(stat -f %d "$(dirname "$target")")" ]]
  mv -f "$temp" "$target"
}

PUBLISHING=1
for task in "${EVIDENCE_TASKS[@]}"; do
  staged_dir="$PUBLISH_TREE/$task/artifacts"
  official_dir="$EVIDENCE/$task/artifacts"
  while IFS= read -r staged_file; do
    target_file="$official_dir/$(basename "$staged_file")"
    [[ ! -e "$target_file" && ! -L "$target_file" ]] || {
      echo "immutable RC artifact already exists: $target_file" >&2
      exit 3
    }
    atomic_publish "$staged_file" "$target_file"
  done < <(find "$RESULTS/$task" -maxdepth 1 -type f -print | LC_ALL=C sort)
  atomic_publish "$staged_dir/MANIFEST.sha256" "$official_dir/MANIFEST.sha256"
done

for task in "${EVIDENCE_TASKS[@]}"; do
  (cd "$EVIDENCE/$task/artifacts" && shasum -a 256 -c MANIFEST.sha256 >/dev/null)
done
for published_report in \
  "V03-01/$BEHAVIOR_NAME" \
  "V03-02/$BASE_NAME" "V03-02/$FUZZY_NAME" \
  "V03-04/$USERDB_NAME" "V03-04/$USERDB_PROCESS_NAME" \
  "V03-06/$PERFORMANCE_NAME" "V03-06/$HARNESS_BUILD_NAME"; do
  report_task="${published_report%%/*}"
  report_name="${published_report#*/}"
  cmp -s "$RESULTS/$report_task/$report_name" "$EVIDENCE/$report_task/artifacts/$report_name" || {
    echo "published RC report failed post-publish verification: $published_report" >&2
    exit 3
  }
done
# RC_ARTIFACT is the transaction commit point. Prepare and validate the exact
# same-filesystem rename source first; a signal sees either the complete old
# pointer or the complete new pointer, never a hybrid.
POINTER_TEMP="$(mktemp "$EVIDENCE/.joint-rc.RC_ARTIFACT.sha256.XXXXXX")"
PUBLISH_TEMPS+=("$POINTER_TEMP")
cp -p "$POINTER_STAGE" "$POINTER_TEMP"
[[ "$(stat -f %d "$POINTER_TEMP")" == "$(stat -f %d "$EVIDENCE")" ]]
[[ "$(awk '/^[0-9a-f]{64}  / { print $1; exit }' "$POINTER_TEMP")" == "$ARTIFACT_SHA" ]]
[[ "$(awk '/^[0-9a-f]{64}  / { count += 1; if (count == 2) { print $1; exit } }' "$POINTER_TEMP")" == "$MANAGER_SHA" ]]
POINTER_REPORT_SET_SHA="$(sandbox-exec -p "$OFFLINE_SANDBOX" node --input-type=module - \
  "$ROOT_DIR/scripts/dogfood_checkpoint.mjs" "$POINTER_TEMP" <<'NODE'
import { pathToFileURL } from "node:url";

const modulePath = pathToFileURL(process.argv[2]).href;
const pointerPath = process.argv[3];
const { computeFrozenPointerReportSetSHA256 } = await import(modulePath);
console.log(computeFrozenPointerReportSetSHA256(pointerPath));
NODE
)"
[[ "$POINTER_REPORT_SET_SHA" =~ ^[0-9a-f]{64}$ ]]
POINTER_FILLED="$(mktemp "$EVIDENCE/.joint-rc.RC_ARTIFACT.filled.sha256.XXXXXX")"
PUBLISH_TEMPS+=("$POINTER_FILLED")
awk -v checksum="$POINTER_REPORT_SET_SHA" '
  /^# pointer_report_set_sha256=/ { print "# pointer_report_set_sha256=" checksum; next }
  { print }
' "$POINTER_TEMP" > "$POINTER_FILLED"
mv -f "$POINTER_FILLED" "$POINTER_TEMP"
[[ "$(grep -c '^# pointer_report_set_sha256=' "$POINTER_TEMP")" == 1 ]]
grep -qx "# pointer_report_set_sha256=$POINTER_REPORT_SET_SHA" "$POINTER_TEMP"
shasum -a 256 "$POINTER_TEMP" >/dev/null
sandbox-exec -p "$OFFLINE_SANDBOX" node --input-type=module - \
  "$ROOT_DIR/scripts/dogfood_checkpoint.mjs" "$POINTER_TEMP" <<'NODE'
import { pathToFileURL } from "node:url";

const modulePath = pathToFileURL(process.argv[2]).href;
const pointerPath = process.argv[3];
const { inspectFrozenReleasePointer } = await import(modulePath);
inspectFrozenReleasePointer(pointerPath);
NODE

# Recheck the immutable pair at the transaction boundary so the pointer cannot
# commit after a frozen-mode, manifest, signature, or checksum regression.
"$FREEZE_TOOL" --verify-frozen "$BUNDLE" "$MANAGER_BUNDLE" >/dev/null
[[ "$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")" == "$ARTIFACT_SHA" ]]
[[ "$("$ROOT_DIR/script/artifact_checksum.sh" "$MANAGER_BUNDLE")" == "$MANAGER_SHA" ]]

# No signal may observe the successful pointer rename before the shell records
# COMMITTED. The pointer is the release commit point; all remaining output and
# owned-temporary cleanup is deliberately non-fatal once it exists.
trap '' HUP INT TERM
if ! mv -f "$POINTER_TEMP" "$existing_pointer"; then
  if ! rollback_publication; then
    exit 91
  fi
  PUBLISHING=0
  exit 3
fi
PUBLISHING=0
COMMITTED=1

printf 'joint_rc_artifact=%s manager_artifact=%s build=%s product_commit=%s product_inputs_sha256=%s stress_seconds=%s\n' \
  "$ARTIFACT_SHA" "$MANAGER_SHA" "$INPUT_BUILD" "$INPUT_COMMIT" "$INPUT_TREE_SHA" "$STRESS_SECONDS_REQUESTED" || true
exit 0
