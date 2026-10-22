#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCT_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
# This gate proves exactly-once learning claims across crashes and replays with one
# pick of the single character 时. Single characters are taught to Rime from their
# second pick on (core rime.rs); learn every pick here so one pick is the claim under test.
export ZHIZI_SINGLE_LEARN=0
DEVELOPMENT_FIXTURE=0
GENERIC_DEVELOPMENT_FIXTURE=0

usage() {
  cat >&2 <<'USAGE'
usage:
  test_userdb_process_recovery.sh <inputmethod-bundle> <expected-artifact-sha256>
  test_userdb_process_recovery.sh --development-fixture <inputmethod-bundle>
  test_userdb_process_recovery.sh --development-fixture-build1019 [inputmethod-bundle]

The first form is the release-evidence mode. It requires an explicit frozen
bundle/checksum, performs its own locked/offline harness build, and enforces
strict code-signature verification. The other forms are DEVELOPMENT_ONLY
fixtures and are never release evidence.
USAGE
}

if [[ "${1:-}" == "--development-fixture" ]]; then
  DEVELOPMENT_FIXTURE=1
  GENERIC_DEVELOPMENT_FIXTURE=1
  shift
  [[ "$#" -eq 1 && -d "$1" ]] || { usage; exit 2; }
  BUNDLE="$1"
  EXPECTED_BUNDLE_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
  export AIM_USERDB_TEST_BOOT_SESSION_UUID="development-only-boot-session"
elif [[ "${1:-}" == "--development-fixture-build1019" ]]; then
  DEVELOPMENT_FIXTURE=1
  shift
  [[ "$#" -le 1 ]] || { usage; exit 2; }
  BUNDLE="${1:-$ROOT_DIR/build/rc/0.3.0-build1019/Agent Input Method.inputmethod}"
  EXPECTED_BUNDLE_SHA="13b3224343eda9e1cd199fbbf072ca17aed3482f3a4696bea3f1c0ebcac9efbf"
  EXPECTED_LIBRIME_SHA="1ca83d174a80b8ad326e14ed8e06a503b2334a0932f85ff3e149359ceb06d08a"
  EXPECTED_DEFAULT_SHA="501746bf466474c23532ebee40364ca6771a3b337f3ae5ea8639e0d629a7ebc6"
  EXPECTED_SCHEMA_SHA="954025d5bbb33704160b027ee4263dcfa4ef5955391c16779fb78aa3c6f2dd46"
  EXPECTED_DICTIONARY_SHA="e341598343a0f0f2035bb1aafc34a7f3bb7887deeecb3f60796262aaa2983e6b"
  export AIM_USERDB_TEST_BOOT_SESSION_UUID="development-only-boot-session"
else
  [[ "$#" -eq 2 ]] || { usage; exit 2; }
  BUNDLE="$1"
  EXPECTED_BUNDLE_SHA="$2"
  [[ "$EXPECTED_BUNDLE_SHA" =~ ^[0-9a-f]{64}$ ]] || {
    echo "expected artifact checksum must be 64 lowercase hexadecimal characters" >&2
    exit 2
  }
  if [[ -n "${AIM_USERDB_PROCESS_SKIP_CODESIGN+x}" ||
        -n "${AIM_USERDB_PROCESS_SKIP_BUILD+x}" ||
        -n "${AIM_USERDB_PROCESS_HARNESS+x}" ||
        -n "${AIM_USERDB_TEST_BOOT_SESSION_UUID+x}" ]]; then
    echo "release-evidence mode forbids AIM_USERDB_PROCESS_SKIP_* and test overrides" >&2
    exit 2
  fi
fi

FRAMEWORKS="$BUNDLE/Contents/Frameworks"
RIME="$BUNDLE/Contents/Resources/Rime"
TEMP_ROOT="$(mktemp -d /tmp/agent-ime-userdb-process.XXXXXX)"
CHILD_PID=""
LOCK_HOLDER_PID=""

cleanup() {
  if [[ -n "$CHILD_PID" ]] && kill -0 "$CHILD_PID" 2>/dev/null; then
    kill -KILL "$CHILD_PID" 2>/dev/null || true
    wait "$CHILD_PID" 2>/dev/null || true
  fi
  if [[ -n "$LOCK_HOLDER_PID" ]] && kill -0 "$LOCK_HOLDER_PID" 2>/dev/null; then
    kill -KILL "$LOCK_HOLDER_PID" 2>/dev/null || true
    wait "$LOCK_HOLDER_PID" 2>/dev/null || true
  fi
  chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true
  rm -rf "$TEMP_ROOT"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

checksum() { shasum -a 256 "$1" | awk '{ print $1 }'; }
text_checksum() { shasum -a 256 | awk '{ print $1 }'; }
plist_value() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null; }
required_plist_value() {
  local value
  value="$(plist_value "$1" "$2")" || {
    echo "missing required $2 in bundle: $1" >&2
    return 3
  }
  [[ -n "$value" ]] || {
    echo "empty required $2 in bundle: $1" >&2
    return 3
  }
  printf '%s\n' "$value"
}
signature_value() {
  codesign -dv --verbose=4 "$1" 2>&1 | awk -F= -v key="$2" '
    $1 == key && !found { value=substr($0, length(key) + 2); found=1 }
    END { if (found) print value; else exit 3 }
  '
}
assert_same_file() {
  [[ -f "$2" && ! -L "$2" && -f "$3" && ! -L "$3" && "$(checksum "$2")" == "$(checksum "$3")" ]] || {
    echo "$1 does not match the project-pinned input" >&2
    return 3
  }
}
json_raw() { plutil -extract "$2" raw -o - "$1"; }
json_optional_raw() { plutil -extract "$2" raw -o - "$1" 2>/dev/null || true; }
learning_count() {
  local entry="$1"
  if [[ -z "$entry" ]]; then
    printf '0\n'
  elif [[ "$entry" =~ c=([0-9]+) ]]; then
    printf '%s\n' "${BASH_REMATCH[1]}"
  else
    echo "unrecognized synced learning entry" >&2
    return 1
  fi
}

learning_total_from_report() {
  local report="$1"
  local total=0 entry count
  while IFS= read -r entry; do
    [[ -n "$entry" ]] || continue
    count="$(learning_count "$entry")"
    total=$((total + count))
  done < <(jq -r '.learningEntries[]?' "$report")
  printf '%s\n' "$total"
}

require_equal() {
  local actual="$1" expected="$2" label="$3"
  if [[ "$actual" != "$expected" ]]; then
    echo "$label mismatch: expected=$expected actual=$actual" >&2
    exit 1
  fi
}

run_harness_expecting_zero_select_hooks() {
  local control="$1"
  local executable="$2"
  local log="$3"
  shift 3
  env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" --process-hook-reset "$control"
  env AIM_USERDB_TEST_CONTROL_PATH="$control" DYLD_LIBRARY_PATH="$FRAMEWORKS" \
    "$executable" "$@" > "$log" 2>&1 &
  CHILD_PID=$!
  local completed=0 status=0
  for _ in {1..200}; do
    env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
      --process-hook-inspect "$control" "$HOOK_REPORT"
    if [[ "$(json_raw "$HOOK_REPORT" selectCalls)" != "0" ]]; then
      sed -n '1,120p' "$log" >&2
      echo "unexpected librime selection hook call: $executable" >&2
      return 1
    fi
    if ! kill -0 "$CHILD_PID" 2>/dev/null; then
      if wait "$CHILD_PID"; then status=0; else status=$?; fi
      CHILD_PID=""
      completed=1
      break
    fi
    sleep 0.05
  done
  [[ "$completed" -eq 1 && "$status" -eq 0 ]] || {
    sed -n '1,120p' "$log" >&2
    echo "zero-hook process did not complete successfully: $executable" >&2
    return 1
  }
  [[ "$(json_raw "$HOOK_REPORT" selectCalls)" == "0" ]]
}

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]]
[[ "$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")" == "$EXPECTED_BUNDLE_SHA" ]]
[[ -f "$FRAMEWORKS/librime.1.dylib" ]]
[[ -f "$RIME/default.yaml" ]]
[[ -f "$RIME/agent_pinyin.schema.yaml" ]]
[[ -f "$RIME/pinyin_simp.dict.yaml" ]]
if [[ "$DEVELOPMENT_FIXTURE" -eq 1 ]]; then
  if [[ "$GENERIC_DEVELOPMENT_FIXTURE" -eq 1 ]]; then
    DEV_COMMIT="$(required_plist_value "$BUNDLE" AgentIMEProductCommit)"
    DEV_INPUTS_SHA256="$(required_plist_value "$BUNDLE" AgentIMEProductInputsSHA256)"
    [[ "$DEV_COMMIT" =~ ^[0-9a-f]{40}$ && "$DEV_INPUTS_SHA256" =~ ^[0-9a-f]{64}$ ]] || {
      echo "invalid development ProductCommit/ProductInputsSHA256" >&2
      exit 3
    }
    INPUT_GUARD="$ROOT_DIR/script/rc_gate_support.sh"
    [[ -x "$INPUT_GUARD" ]]
    [[ "$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$DEV_COMMIT")" == "$DEV_COMMIT" ]]
    [[ "$("$INPUT_GUARD" fingerprint "$ROOT_DIR" "$DEV_COMMIT")" == "$DEV_INPUTS_SHA256" ]]
    assert_same_file default-schema "$ROOT_DIR/native/resources/Rime/default.yaml" "$RIME/default.yaml"
    assert_same_file agent-schema "$ROOT_DIR/native/resources/Rime/agent_pinyin.schema.yaml" "$RIME/agent_pinyin.schema.yaml"
    assert_same_file dictionary \
      "$ROOT_DIR/.build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml" \
      "$RIME/pinyin_simp.dict.yaml"
    cp "$ROOT_DIR/.build-deps/librime/1.16.1/lib/librime.1.dylib" "$TEMP_ROOT/pinned-librime-unsigned.dylib"
    cp "$FRAMEWORKS/librime.1.dylib" "$TEMP_ROOT/bundled-librime-unsigned.dylib"
    codesign --remove-signature "$TEMP_ROOT/pinned-librime-unsigned.dylib" 2>/dev/null || true
    codesign --remove-signature "$TEMP_ROOT/bundled-librime-unsigned.dylib" 2>/dev/null || true
    assert_same_file librime-code \
      "$TEMP_ROOT/pinned-librime-unsigned.dylib" "$TEMP_ROOT/bundled-librime-unsigned.dylib"
  else
    [[ "$(checksum "$FRAMEWORKS/librime.1.dylib")" == "$EXPECTED_LIBRIME_SHA" ]]
    [[ "$(checksum "$RIME/default.yaml")" == "$EXPECTED_DEFAULT_SHA" ]]
    [[ "$(checksum "$RIME/agent_pinyin.schema.yaml")" == "$EXPECTED_SCHEMA_SHA" ]]
    [[ "$(checksum "$RIME/pinyin_simp.dict.yaml")" == "$EXPECTED_DICTIONARY_SHA" ]]
  fi
else
  VERSION="$(required_plist_value "$BUNDLE" CFBundleShortVersionString)"
  BUILD="$(required_plist_value "$BUNDLE" CFBundleVersion)"
  BUNDLE_ID="$(required_plist_value "$BUNDLE" CFBundleIdentifier)"
  PRODUCT_COMMIT="$(required_plist_value "$BUNDLE" AgentIMEProductCommit)"
  PRODUCT_INPUTS_SHA256="$(required_plist_value "$BUNDLE" AgentIMEProductInputsSHA256)"
  BUILD_CONFIGURATION="$(required_plist_value "$BUNDLE" AgentIMEBuildConfiguration)"
  TEAM_IDENTIFIER="$(signature_value "$BUNDLE" TeamIdentifier)"
  LEAF_AUTHORITY="$(signature_value "$BUNDLE" Authority)"
  FROZEN_METADATA="$(dirname "$BUNDLE")/FROZEN_ARTIFACTS"
  [[ -f "$FROZEN_METADATA" && ! -L "$FROZEN_METADATA" && "$(stat -f %l "$FROZEN_METADATA")" == "1" ]] || {
    echo "official UserDB gate requires regular single-link FROZEN_ARTIFACTS metadata" >&2
    exit 3
  }
  FROZEN_TEAM_IDENTIFIER_SHA256="$(awk -F= '$1 == "team_identifier_sha256" { count += 1; value = $2 } END { if (count == 1) print value; else exit 3 }' "$FROZEN_METADATA")"
  FROZEN_LEAF_AUTHORITY_SHA256="$(awk -F= '$1 == "leaf_authority_sha256" { count += 1; value = $2 } END { if (count == 1) print value; else exit 3 }' "$FROZEN_METADATA")"
  [[ "$VERSION" == "$PRODUCT_VERSION" ]] || { echo "unexpected input version: $VERSION" >&2; exit 3; }
  [[ "$BUILD" =~ ^[1-9][0-9]{0,8}$ ]] && (( 10#$BUILD >= 1020 )) || {
    echo "official UserDB gate requires build >= 1020: $BUILD" >&2
    exit 3
  }
  [[ "$BUNDLE_ID" == "com.hongye.facai.inputmethod.zhizi" ]] || {
    echo "unexpected input bundle identifier: $BUNDLE_ID" >&2
    exit 3
  }
  [[ "$PRODUCT_COMMIT" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid ProductCommit" >&2; exit 3; }
  [[ "$PRODUCT_INPUTS_SHA256" =~ ^[0-9a-f]{64}$ ]] || {
    echo "invalid ProductInputsSHA256" >&2
    exit 3
  }
  [[ "$BUILD_CONFIGURATION" == "release" ]] || { echo "input bundle is not release" >&2; exit 3; }
  [[ "$TEAM_IDENTIFIER" =~ ^[A-Z0-9]{10}$ && "$LEAF_AUTHORITY" == "Apple Development:"* ]] || {
    echo "input signing identity is invalid" >&2
    exit 3
  }
  [[ "$FROZEN_TEAM_IDENTIFIER_SHA256" =~ ^[0-9a-f]{64}$ \
    && "$FROZEN_LEAF_AUTHORITY_SHA256" =~ ^[0-9a-f]{64}$ \
    && "$(printf '%s' "$TEAM_IDENTIFIER" | text_checksum)" == "$FROZEN_TEAM_IDENTIFIER_SHA256" \
    && "$(printf '%s' "$LEAF_AUTHORITY" | text_checksum)" == "$FROZEN_LEAF_AUTHORITY_SHA256" ]] || {
    echo "frozen metadata/input signing identity hashes are invalid or mismatched" >&2
    exit 3
  }
  if [[ -n "${AIM_AUTHORIZED_TEAM_ID-}" ]]; then
    [[ "${AIM_AUTHORIZED_TEAM_ID}" =~ ^[A-Z0-9]{10}$ && "${AIM_AUTHORIZED_TEAM_ID}" == "$TEAM_IDENTIFIER" ]] || {
      echo "AIM_AUTHORIZED_TEAM_ID does not match the frozen RC TeamIdentifier" >&2
      exit 3
    }
  fi
  INPUT_GUARD="$ROOT_DIR/script/rc_gate_support.sh"
  [[ -x "$INPUT_GUARD" ]]
  [[ "$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$PRODUCT_COMMIT")" == "$PRODUCT_COMMIT" ]]
  [[ "$("$INPUT_GUARD" fingerprint "$ROOT_DIR" "$PRODUCT_COMMIT")" == "$PRODUCT_INPUTS_SHA256" ]]
  assert_same_file default-schema "$ROOT_DIR/native/resources/Rime/default.yaml" "$RIME/default.yaml"
  assert_same_file agent-schema "$ROOT_DIR/native/resources/Rime/agent_pinyin.schema.yaml" "$RIME/agent_pinyin.schema.yaml"
  assert_same_file dictionary \
    "$ROOT_DIR/.build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml" \
    "$RIME/pinyin_simp.dict.yaml"
  cp "$ROOT_DIR/.build-deps/librime/1.16.1/lib/librime.1.dylib" "$TEMP_ROOT/pinned-librime-unsigned.dylib"
  cp "$FRAMEWORKS/librime.1.dylib" "$TEMP_ROOT/bundled-librime-unsigned.dylib"
  codesign --remove-signature "$TEMP_ROOT/pinned-librime-unsigned.dylib" 2>/dev/null || true
  codesign --remove-signature "$TEMP_ROOT/bundled-librime-unsigned.dylib" 2>/dev/null || true
  assert_same_file librime-code \
    "$TEMP_ROOT/pinned-librime-unsigned.dylib" "$TEMP_ROOT/bundled-librime-unsigned.dylib"
fi
if [[ "$DEVELOPMENT_FIXTURE" -eq 0 || "${AIM_USERDB_PROCESS_SKIP_CODESIGN:-0}" != "1" ]]; then
  codesign --verify --deep --strict "$BUNDLE"
fi

if [[ "$DEVELOPMENT_FIXTURE" -eq 0 || "${AIM_USERDB_PROCESS_SKIP_BUILD:-0}" != "1" ]]; then
  CORE_TARGET_DIR="$TEMP_ROOT/core-target"
  CARGO_HOME_DIR="$TEMP_ROOT/cargo-home"
  CARGO_VENDOR_DIR="$TEMP_ROOT/cargo-vendor"
  SWIFT_SCRATCH="$TEMP_ROOT/swift-build"
  SWIFT_CACHE="$TEMP_ROOT/swift-cache"
  SWIFT_CONFIG="$TEMP_ROOT/swift-config"
  SWIFT_SECURITY="$TEMP_ROOT/swift-security"
  SWIFT_HOME="$TEMP_ROOT/swift-home"
  SWIFT_MODULE_CACHE="$TEMP_ROOT/swift-module-cache"
  mkdir -p "$CARGO_HOME_DIR" "$CARGO_VENDOR_DIR" "$SWIFT_CACHE" "$SWIFT_CONFIG" "$SWIFT_SECURITY" \
    "$SWIFT_HOME" "$SWIFT_MODULE_CACHE"
  # The build intentionally uses a fresh CARGO_HOME so user config cannot
  # influence the recovery harness. Seed that isolated home through Cargo's
  # checksum-verified vendor format while the normal locked cache is still
  # visible; the build itself remains locked, offline and target-isolated.
  CARGO_NET_OFFLINE=true cargo vendor --locked --offline \
    --manifest-path "$ROOT_DIR/native/core/Cargo.toml" "$CARGO_VENDOR_DIR" >/dev/null
  CARGO_HOME="$CARGO_HOME_DIR" CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR="$CORE_TARGET_DIR" cargo build --locked --offline \
    --manifest-path "$ROOT_DIR/native/core/Cargo.toml" --release --features librime \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config "source.vendored-sources.directory=\"$CARGO_VENDOR_DIR\""
  HOME="$SWIFT_HOME" CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" \
    SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" \
    AIM_CORE_LIBRARY_DIR="$CORE_TARGET_DIR/release" swift build \
    --package-path "$ROOT_DIR/native/macos" --scratch-path "$SWIFT_SCRATCH" \
    --cache-path "$SWIFT_CACHE" --config-path "$SWIFT_CONFIG" \
    --security-path "$SWIFT_SECURITY" --disable-sandbox \
    -c release --disable-automatic-resolution --skip-update --disable-prefetching \
    --product AgentIMEUserDBHarness
  SWIFT_BIN_PATH="$(HOME="$SWIFT_HOME" CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" \
    SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" \
    AIM_CORE_LIBRARY_DIR="$CORE_TARGET_DIR/release" swift build \
    --package-path "$ROOT_DIR/native/macos" --scratch-path "$SWIFT_SCRATCH" \
    --cache-path "$SWIFT_CACHE" --config-path "$SWIFT_CONFIG" \
    --security-path "$SWIFT_SECURITY" --disable-sandbox \
    -c release --disable-automatic-resolution --skip-update --disable-prefetching \
    --show-bin-path)"
  HARNESS="$SWIFT_BIN_PATH/AgentIMEUserDBHarness"
else
  HARNESS="${AIM_USERDB_PROCESS_HARNESS:?set AIM_USERDB_PROCESS_HARNESS when skipping build}"
fi
[[ -x "$HARNESS" ]]

CONTROL_ROOT="$TEMP_ROOT/control-root"
REPLAY_ROOT="$TEMP_ROOT/replay-root"
CLAIM_ROOT="$TEMP_ROOT/claim-root"
CRASH_ROOT="$TEMP_ROOT/crash-root"
POST_SYNC_ROOT="$TEMP_ROOT/post-sync-root"
ZERO_ID_ROOT="$TEMP_ROOT/zero-id-root"
PRODUCT_NAME_ROOT="$TEMP_ROOT/product-name-root"
CONTENDED_ROOT="$TEMP_ROOT/contended-root"
DIAGNOSTICS="$TEMP_ROOT/diagnostics"
CONTROL_SELECT="$TEMP_ROOT/control-select.json"
REPLAY_SELECT="$TEMP_ROOT/replay-select.json"
CONTROL_INSPECT="$TEMP_ROOT/control-inspect.json"
CLAIM_PRESELECT="$TEMP_ROOT/claim-preselect.json"
CLAIM_REPLAY="$TEMP_ROOT/claim-replay.json"
CLAIM_DIFFERENT="$TEMP_ROOT/claim-different.json"
CRASH_PRESELECT="$TEMP_ROOT/crash-preselect.json"
CRASH_INSPECT="$TEMP_ROOT/crash-inspect.json"
CRASH_REPLAY="$TEMP_ROOT/crash-replay.json"
HOOK_CONTROL="$TEMP_ROOT/after-select-control.bin"
HOOK_REPORT="$TEMP_ROOT/after-select-control.json"
CLAIM_CONTROL="$TEMP_ROOT/pre-select-control.bin"
CLAIM_REPLAY_HOOK_CONTROL="$TEMP_ROOT/pre-select-replay-hook-control.bin"
CRASH_REPLAY_HOOK_CONTROL="$TEMP_ROOT/after-select-replay-hook-control.bin"
HIGH_ID_RETRY_REPORT_1="$TEMP_ROOT/high-id-retry-report-1.json"
HIGH_ID_RETRY_REPORT_2="$TEMP_ROOT/high-id-retry-report-2.json"
RESTART_SELECT="$TEMP_ROOT/restart-select.json"
POST_SYNC_SELECT="$TEMP_ROOT/post-sync-select.json"
POST_SYNC_REPLAY="$TEMP_ROOT/post-sync-replay.json"
POST_SYNC_DIFFERENT="$TEMP_ROOT/post-sync-different.json"
POST_SYNC_STOP_CONTROL="$TEMP_ROOT/post-sync-stop-control.bin"
POST_SYNC_HOOK_CONTROL="$TEMP_ROOT/post-sync-hook-control.bin"
ZERO_ID_REPORT="$TEMP_ROOT/zero-id-report.json"
PRODUCT_NAME_REPORT="$TEMP_ROOT/product-name-report.json"
CONTENDED_REPORT="$TEMP_ROOT/contended-report.json"
CONTENDED_RECOVERY_REPORT="$TEMP_ROOT/contended-recovery-report.json"
PRODUCT_NAME_HOOK_CONTROL="$TEMP_ROOT/product-name-hook-control.bin"
CONTENDED_HOOK_CONTROL="$TEMP_ROOT/contended-hook-control.bin"
REPLAY_TIMESTAMP="1000000000"
CRASH_TIMESTAMP="2000000000"
RESTART_TIMESTAMP="3000000000"
POST_SYNC_TIMESTAMP="4000000000"
POST_SYNC_DIFFERENT_TIMESTAMP="5000000000"
CLAIM_TIMESTAMP="6000000000"
CLAIM_DIFFERENT_TIMESTAMP="7000000000"

env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-select-once "$RIME" "$CONTROL_ROOT" "$DIAGNOSTICS" "$CONTROL_SELECT" "$REPLAY_TIMESTAMP"
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-inspect "$RIME" "$CONTROL_ROOT" "$CONTROL_INSPECT"
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-select-once "$RIME" "$REPLAY_ROOT" "$DIAGNOSTICS" "$REPLAY_SELECT" "$REPLAY_TIMESTAMP"

CONTROL_EVENT_ID="$(json_raw "$CONTROL_SELECT" eventID)"
REPLAY_EVENT_ID="$(json_raw "$REPLAY_SELECT" eventID)"
[[ "$CONTROL_EVENT_ID" == "$REPLAY_EVENT_ID" ]]
[[ "$(json_raw "$CONTROL_SELECT" diagnosticCount)" == "1" ]]
[[ "$(json_raw "$REPLAY_SELECT" diagnosticCount)" == "1" ]]

# Crash after the durable claim but before librime select. Replaying the same
# event must commit the candidate text without learning; a different event may
# then learn exactly once.
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" --process-hook-reset "$CLAIM_CONTROL"
env AIM_USERDB_TEST_PRESELECT_CONTROL_PATH="$CLAIM_CONTROL" \
  DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" --process-claim-and-wait \
  "$RIME" "$CLAIM_ROOT" "$CLAIM_PRESELECT" "$CLAIM_TIMESTAMP" \
  > "$TEMP_ROOT/pre-select-worker.log" 2>&1 &
CHILD_PID=$!
CLAIM_STOPPED=0
for _ in {1..200}; do
  kill -0 "$CHILD_PID" 2>/dev/null || {
    sed -n '1,120p' "$TEMP_ROOT/pre-select-worker.log" >&2
    echo "pre-select worker exited before the claim hook" >&2
    exit 1
  }
  if [[ -f "$CLAIM_PRESELECT" ]]; then
    env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
      --process-hook-inspect "$CLAIM_CONTROL" "$HOOK_REPORT"
    if [[ "$(json_raw "$HOOK_REPORT" hookState)" == "5" && \
          "$(json_raw "$HOOK_REPORT" selectCalls)" == "1" && \
          "$(json_raw "$HOOK_REPORT" processID)" == "$CHILD_PID" ]]; then
      CLAIM_STOPPED=1
      break
    fi
  fi
  sleep 0.05
done
[[ "$CLAIM_STOPPED" -eq 1 && -f "$CLAIM_PRESELECT" ]]
CLAIM_EVENT_ID="$(json_raw "$CLAIM_PRESELECT" eventID)"
[[ "$CLAIM_EVENT_ID" != "0" ]]
kill -KILL "$CHILD_PID"
if wait "$CHILD_PID" 2>/dev/null; then
  CLAIM_CRASH_STATUS=0
else
  CLAIM_CRASH_STATUS=$?
fi
CHILD_PID=""
[[ "$CLAIM_CRASH_STATUS" -eq 137 ]]

run_harness_expecting_zero_select_hooks \
  "$CLAIM_REPLAY_HOOK_CONTROL" "$HARNESS" "$TEMP_ROOT/pre-select-replay.log" \
  --process-select-with-delivery-id "$RIME" "$CLAIM_ROOT" "$DIAGNOSTICS" \
  "$CLAIM_REPLAY" "$CLAIM_EVENT_ID"
[[ "$(json_raw "$CLAIM_REPLAY" eventID)" == "$CLAIM_EVENT_ID" ]]
[[ "$(json_raw "$CLAIM_REPLAY" committedTarget)" == "true" ]]
CLAIM_REPLAY_LEARNING_COUNT="$(learning_total_from_report "$CLAIM_REPLAY")"
require_equal "$CLAIM_REPLAY_LEARNING_COUNT" 0 "pre-select same-event replay learning count"

env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-select-once "$RIME" "$CLAIM_ROOT" "$DIAGNOSTICS" \
  "$CLAIM_DIFFERENT" "$CLAIM_DIFFERENT_TIMESTAMP"
[[ "$(json_raw "$CLAIM_DIFFERENT" eventID)" != "$CLAIM_EVENT_ID" ]]
CLAIM_DIFFERENT_LEARNING_COUNT="$(learning_total_from_report "$CLAIM_DIFFERENT")"
require_equal "$CLAIM_DIFFERENT_LEARNING_COUNT" 1 "pre-select different-event learning count"

env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" --process-hook-reset "$HOOK_CONTROL"
env AIM_USERDB_TEST_CONTROL_PATH="$HOOK_CONTROL" DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-train-and-wait "$RIME" "$CRASH_ROOT" "$CRASH_PRESELECT" "$CRASH_TIMESTAMP" \
  > "$TEMP_ROOT/crash-worker.log" 2>&1 &
CHILD_PID=$!
for _ in {1..200}; do
  kill -0 "$CHILD_PID" 2>/dev/null || {
    sed -n '1,120p' "$TEMP_ROOT/crash-worker.log" >&2
    echo "userdb crash worker exited before the after-select hook" >&2
    exit 1
  }
  if [[ -f "$CRASH_PRESELECT" ]]; then
    env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
      --process-hook-inspect "$HOOK_CONTROL" "$HOOK_REPORT"
    if [[ "$(json_raw "$HOOK_REPORT" hookState)" == "1" && \
          "$(json_raw "$HOOK_REPORT" selectCalls)" == "1" ]]; then
      break
    fi
  fi
  sleep 0.05
done
[[ -f "$CRASH_PRESELECT" && -f "$HOOK_REPORT" ]]
[[ "$(json_raw "$CRASH_PRESELECT" databaseStructurallyValid)" == "true" ]]
[[ "$(json_raw "$HOOK_REPORT" hookState)" == "1" ]]
[[ "$(json_raw "$HOOK_REPORT" selectCalls)" == "1" ]]
[[ "$(json_raw "$HOOK_REPORT" processID)" == "$CHILD_PID" ]]
[[ "$(json_raw "$HOOK_REPORT" selectedResult)" != "0" ]]
kill -KILL "$CHILD_PID"
if wait "$CHILD_PID" 2>/dev/null; then
  CRASH_STATUS=0
else
  CRASH_STATUS=$?
fi
CHILD_PID=""
[[ "$CRASH_STATUS" -eq 137 ]]

env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-inspect "$RIME" "$CRASH_ROOT" "$CRASH_INSPECT"
[[ "$(json_raw "$CRASH_INSPECT" selectionCalls)" == "0" ]]
[[ "$(json_raw "$CRASH_INSPECT" databaseStructurallyValid)" == "true" ]]
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-hook-inspect "$HOOK_CONTROL" "$HOOK_REPORT"
[[ "$(json_raw "$HOOK_REPORT" selectCalls)" == "1" ]]
[[ "$(json_raw "$HOOK_REPORT" hookState)" == "1" ]]

CRASH_EVENT_ID="$(json_raw "$CRASH_PRESELECT" eventID)"
[[ "$CONTROL_EVENT_ID" != "$CRASH_EVENT_ID" ]]
CONTROL_LEARNING_COUNT="$(learning_total_from_report "$CONTROL_INSPECT")"
CRASH_LEARNING_COUNT="$(learning_total_from_report "$CRASH_INSPECT")"
require_equal "$CONTROL_LEARNING_COUNT" 1 "control learning count"
require_equal "$CRASH_LEARNING_COUNT" 0 "pre-sync crash learning count"

# The selection call returned successfully inside librime, but the process was
# killed before a UserDB sync. The durable pre-selection claim must make an
# exact-event replay a normal no-learning commit and must not re-enter librime
# selection. Losing this one unsynced learning update is the safe side of the
# at-most-once boundary.
run_harness_expecting_zero_select_hooks \
  "$CRASH_REPLAY_HOOK_CONTROL" "$HARNESS" "$TEMP_ROOT/after-select-replay.log" \
  --process-select-with-delivery-id "$RIME" "$CRASH_ROOT" "$DIAGNOSTICS" \
  "$CRASH_REPLAY" "$CRASH_EVENT_ID"
[[ "$(json_raw "$CRASH_REPLAY" eventID)" == "$CRASH_EVENT_ID" ]]
[[ "$(json_raw "$CRASH_REPLAY" committedTarget)" == "true" ]]
CRASH_REPLAY_LEARNING_COUNT="$(learning_total_from_report "$CRASH_REPLAY")"
require_equal "$CRASH_REPLAY_LEARNING_COUNT" 0 \
  "after-select pre-sync same-event replay learning count"

HIGH_SELECTION_EVENT_ID="18446744073709551557"
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-retry-diagnostic "$DIAGNOSTICS" "$HIGH_SELECTION_EVENT_ID" "$HIGH_ID_RETRY_REPORT_1"
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-retry-diagnostic "$DIAGNOSTICS" "$HIGH_SELECTION_EVENT_ID" "$HIGH_ID_RETRY_REPORT_2"
[[ "$(json_raw "$HIGH_ID_RETRY_REPORT_1" diagnosticCount)" == "1" ]]
[[ "$(json_raw "$HIGH_ID_RETRY_REPORT_2" diagnosticCount)" == "1" ]]

env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-select-once "$RIME" "$CRASH_ROOT" "$DIAGNOSTICS" "$RESTART_SELECT" "$RESTART_TIMESTAMP"
RESTART_EVENT_ID="$(json_raw "$RESTART_SELECT" eventID)"
[[ "$RESTART_EVENT_ID" != "$CRASH_EVENT_ID" ]]
[[ "$RESTART_EVENT_ID" != "$CONTROL_EVENT_ID" ]]
RESTART_LEARNING_COUNT="$(learning_total_from_report "$RESTART_SELECT")"
require_equal "$RESTART_LEARNING_COUNT" 1 "explicit retry learning count"

# Prove the opposite crash boundary as well: once librime learning is synced,
# killing the process and replaying the exact same physical event against the
# same UserDB must not call librime selection or increment learning. A distinct
# physical event must still increment exactly once.
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-hook-reset "$POST_SYNC_STOP_CONTROL"
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-sync-and-wait "$RIME" "$POST_SYNC_ROOT" "$POST_SYNC_SELECT" \
  "$POST_SYNC_TIMESTAMP" "$POST_SYNC_STOP_CONTROL" \
  > "$TEMP_ROOT/post-sync-worker.log" 2>&1 &
CHILD_PID=$!
POST_SYNC_STOPPED=0
for _ in {1..200}; do
  kill -0 "$CHILD_PID" 2>/dev/null || {
    sed -n '1,120p' "$TEMP_ROOT/post-sync-worker.log" >&2
    echo "post-sync worker exited before SIGSTOP" >&2
    exit 1
  }
  if [[ -f "$POST_SYNC_SELECT" ]]; then
    env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
      --process-hook-inspect "$POST_SYNC_STOP_CONTROL" "$HOOK_REPORT"
    if [[ "$(json_raw "$HOOK_REPORT" hookState)" == "3" && \
          "$(json_raw "$HOOK_REPORT" processID)" == "$CHILD_PID" && \
          "$(json_raw "$HOOK_REPORT" selectCalls)" == "0" ]]; then
      POST_SYNC_STOPPED=1
      break
    fi
  fi
  sleep 0.05
done
[[ "$POST_SYNC_STOPPED" -eq 1 && -f "$POST_SYNC_SELECT" ]]
POST_SYNC_EVENT_ID="$(json_raw "$POST_SYNC_SELECT" eventID)"
POST_SYNC_FIRST_COUNT="$(learning_total_from_report "$POST_SYNC_SELECT")"
[[ "$POST_SYNC_EVENT_ID" != "0" ]]
require_equal "$POST_SYNC_FIRST_COUNT" 1 "post-sync first learning count"
kill -KILL "$CHILD_PID"
if wait "$CHILD_PID" 2>/dev/null; then
  POST_SYNC_CRASH_STATUS=0
else
  POST_SYNC_CRASH_STATUS=$?
fi
CHILD_PID=""
[[ "$POST_SYNC_CRASH_STATUS" -eq 137 ]]

run_harness_expecting_zero_select_hooks \
  "$POST_SYNC_HOOK_CONTROL" "$HARNESS" "$TEMP_ROOT/post-sync-replay.log" \
  --process-select-with-delivery-id "$RIME" "$POST_SYNC_ROOT" "$DIAGNOSTICS" \
  "$POST_SYNC_REPLAY" "$POST_SYNC_EVENT_ID"
[[ "$(json_raw "$POST_SYNC_REPLAY" eventID)" == "$POST_SYNC_EVENT_ID" ]]
POST_SYNC_REPLAY_COUNT="$(learning_total_from_report "$POST_SYNC_REPLAY")"
require_equal "$POST_SYNC_REPLAY_COUNT" 1 "same-event replay learning count"

env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-select-once "$RIME" "$POST_SYNC_ROOT" "$DIAGNOSTICS" \
  "$POST_SYNC_DIFFERENT" "$POST_SYNC_DIFFERENT_TIMESTAMP"
[[ "$(json_raw "$POST_SYNC_DIFFERENT" eventID)" != "$POST_SYNC_EVENT_ID" ]]
POST_SYNC_DIFFERENT_COUNT="$(learning_total_from_report "$POST_SYNC_DIFFERENT")"
# The first deliberate lower-ranked selection already promoted the target to
# Top1. A later physical event may commit it, but reflex Top1 commits must not
# add another UserDB learning count (layer-0 anti-lock-in contract).
require_equal "$POST_SYNC_DIFFERENT_COUNT" 1 "top1 different-event no-learning count"

# Missing stable delivery identity and any ledger failure are no-learning
# commits. This path still returns the chosen text to the host.
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-select-zero-id "$RIME" "$ZERO_ID_ROOT" "$ZERO_ID_REPORT"
[[ "$(json_raw "$ZERO_ID_REPORT" eventID)" == "0" ]]
ZERO_ID_LEARNING_COUNT="$(learning_total_from_report "$ZERO_ID_REPORT")"
require_equal "$ZERO_ID_LEARNING_COUNT" 0 "zero-ID learning count"

# A separate process holds the exact ledger lock while a real librime session
# commits a candidate. Contention must fail closed immediately: the candidate
# still commits, no select/learning hook runs, and learning recovers after the
# holder releases the lock.
CONTENDED_LEDGER_DIR="$CONTENDED_ROOT/Rime/.agent-ime-selection-ledger-v1"
CONTENDED_LOCK="$CONTENDED_LEDGER_DIR/ledger.lock"
CONTENDED_READY="$TEMP_ROOT/contended-lock-ready"
CONTENDED_RELEASE="$TEMP_ROOT/contended-lock-release"
mkdir -p "$CONTENDED_LEDGER_DIR"
chmod 700 "$CONTENDED_ROOT" "$CONTENDED_ROOT/Rime" "$CONTENDED_LEDGER_DIR"
: > "$CONTENDED_LOCK"
chmod 600 "$CONTENDED_LOCK"
/usr/bin/perl -MFcntl=:flock -e '
  use strict;
  use warnings;
  open(my $lock, "+<", $ARGV[0]) or die "open lock: $!";
  flock($lock, LOCK_EX) or die "flock: $!";
  open(my $ready, ">", $ARGV[1]) or die "open ready: $!";
  print {$ready} "ready\n";
  close($ready) or die "close ready: $!";
  while (!-e $ARGV[2]) { select(undef, undef, undef, 0.01); }
' "$CONTENDED_LOCK" "$CONTENDED_READY" "$CONTENDED_RELEASE" &
LOCK_HOLDER_PID=$!
for _ in {1..100}; do
  [[ -f "$CONTENDED_READY" ]] && break
  kill -0 "$LOCK_HOLDER_PID" 2>/dev/null || {
    echo "ledger lock holder exited before readiness" >&2
    exit 1
  }
  sleep 0.01
done
[[ -f "$CONTENDED_READY" ]]
run_harness_expecting_zero_select_hooks \
  "$CONTENDED_HOOK_CONTROL" "$HARNESS" "$TEMP_ROOT/contended-select.log" \
  --process-select-with-delivery-id "$RIME" "$CONTENDED_ROOT" "$DIAGNOSTICS" \
  "$CONTENDED_REPORT" "$CONTROL_EVENT_ID"
[[ "$(json_raw "$CONTENDED_REPORT" committedTarget)" == "true" ]]
CONTENDED_LEARNING_COUNT="$(learning_total_from_report "$CONTENDED_REPORT")"
require_equal "$CONTENDED_LEARNING_COUNT" 0 "contended-lock learning count"
CONTENDED_SELECTION_NS="$(json_raw "$CONTENDED_REPORT" selectionElapsedNanoseconds)"
[[ "$CONTENDED_SELECTION_NS" =~ ^[0-9]+$ && "$CONTENDED_SELECTION_NS" -lt 100000000 ]]
touch "$CONTENDED_RELEASE"
wait "$LOCK_HOLDER_PID"
LOCK_HOLDER_PID=""
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-select-once "$RIME" "$CONTENDED_ROOT" "$DIAGNOSTICS" \
  "$CONTENDED_RECOVERY_REPORT" "9000000000"
CONTENDED_RECOVERY_COUNT="$(learning_total_from_report "$CONTENDED_RECOVERY_REPORT")"
require_equal "$CONTENDED_RECOVERY_COUNT" 1 "post-contention learning count"

# Even though the copied test binary still exports the support symbol, the C
# shim must not discover/call it unless the executable name is the dedicated
# UserDB harness name used above.
PRODUCT_NAME_HARNESS="$TEMP_ROOT/AgentIMEProductLikeHarness"
cp -p "$HARNESS" "$PRODUCT_NAME_HARNESS"
chmod 700 "$PRODUCT_NAME_HARNESS"
run_harness_expecting_zero_select_hooks \
  "$PRODUCT_NAME_HOOK_CONTROL" "$PRODUCT_NAME_HARNESS" \
  "$TEMP_ROOT/product-name-hook.log" --process-select-with-delivery-id "$RIME" \
  "$PRODUCT_NAME_ROOT" "$DIAGNOSTICS" "$PRODUCT_NAME_REPORT" \
  "$CONTROL_EVENT_ID"
PRODUCT_NAME_LEARNING_COUNT="$(learning_total_from_report "$PRODUCT_NAME_REPORT")"
require_equal "$PRODUCT_NAME_LEARNING_COUNT" 1 "product-name learning count"

LEDGER_DIRECTORY="$POST_SYNC_ROOT/Rime/.agent-ime-selection-ledger-v1"
[[ -d "$LEDGER_DIRECTORY" && ! -L "$LEDGER_DIRECTORY" ]]
[[ "$(stat -f %Lp "$LEDGER_DIRECTORY")" == "700" ]]
for ledger_file in "$LEDGER_DIRECTORY/ledger.lock" "$LEDGER_DIRECTORY/delivery-ids.bin"; do
  [[ -f "$ledger_file" && ! -L "$ledger_file" ]]
  [[ "$(stat -f %Lp "$ledger_file")" == "600" ]]
done
[[ "$(stat -f %z "$LEDGER_DIRECTORY/delivery-ids.bin")" -le 32780 ]]

RECOVERY_ROOT="$TEMP_ROOT/recovery-root"
RECOVERY_DATABASE="$RECOVERY_ROOT/Rime/pinyin_simp.userdb"
RECOVERY_BACKUPS="$RECOVERY_ROOT/UserDBBackups"
mkdir -p "$RECOVERY_ROOT/Rime" "$RECOVERY_BACKUPS"
ditto "$CONTROL_ROOT/Rime/pinyin_simp.userdb" "$RECOVERY_DATABASE"

for fixture in older newest-valid newest-invalid; do
  mkdir -p "$RECOVERY_BACKUPS/$fixture"
  ditto "$CONTROL_ROOT/Rime/pinyin_simp.userdb" \
    "$RECOVERY_BACKUPS/$fixture/pinyin_simp.userdb"
done
printf 'older\n' > "$RECOVERY_BACKUPS/older/pinyin_simp.userdb/TEST_MARKER"
printf 'newest-valid\n' > "$RECOVERY_BACKUPS/newest-valid/pinyin_simp.userdb/TEST_MARKER"
printf 'newest-invalid\n' > "$RECOVERY_BACKUPS/newest-invalid/pinyin_simp.userdb/TEST_MARKER"
printf '../invalid\n' > "$RECOVERY_BACKUPS/newest-invalid/pinyin_simp.userdb/CURRENT"
touch -t 202607130101 "$RECOVERY_BACKUPS/older"
touch -t 202607130102 "$RECOVERY_BACKUPS/newest-valid"
touch -t 202607130103 "$RECOVERY_BACKUPS/newest-invalid"
printf 'corrupt-current\n' > "$RECOVERY_DATABASE/TEST_MARKER"
printf '../invalid\n' > "$RECOVERY_DATABASE/CURRENT"

RECOVERY_REPORT="$TEMP_ROOT/recovery-report.json"
env DYLD_LIBRARY_PATH="$FRAMEWORKS" "$HARNESS" \
  --process-prepare "$RECOVERY_ROOT" "$RECOVERY_REPORT"
[[ "$(json_raw "$RECOVERY_REPORT" preparation.quarantined)" == "true" ]]
[[ "$(json_raw "$RECOVERY_REPORT" preparation.restored)" == "true" ]]
[[ "$(json_raw "$RECOVERY_REPORT" preparation.backedUp)" == "true" ]]
[[ "$(json_raw "$RECOVERY_REPORT" databaseStructurallyValid)" == "true" ]]
[[ "$(json_raw "$RECOVERY_REPORT" restoredMarkers.0)" == "newest-valid" ]]
[[ "$(json_raw "$RECOVERY_REPORT" quarantineMarkers.0)" == "corrupt-current" ]]
[[ "$(json_raw "$RECOVERY_REPORT" backupCount)" -le 2 ]]

FINAL_BUNDLE_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"
[[ "$FINAL_BUNDLE_SHA" == "$EXPECTED_BUNDLE_SHA" ]] || {
  echo "frozen bundle changed while the UserDB process gate was running" >&2
  exit 3
}

if [[ "$DEVELOPMENT_FIXTURE" -eq 1 ]]; then
  RESULT_MODE="DEVELOPMENT_ONLY"
else
  RESULT_MODE="RELEASE_EVIDENCE"
fi
printf 'userdb_process_recovery=PASS mode=%s artifact_checksum=%s librime_checksum=%s crash_exit=%s\n' \
  "$RESULT_MODE" "$FINAL_BUNDLE_SHA" "$(checksum "$FRAMEWORKS/librime.1.dylib")" "$CRASH_STATUS"
printf 'after_select_hook_calls=1 hook_state=stopped restart_inspect_selection_calls=0 crash_learning_count=%s explicit_retry_learning_count=%s\n' \
  "$CRASH_LEARNING_COUNT" "$RESTART_LEARNING_COUNT"
printf 'same_physical_event_replay_id_stable=true different_physical_event_ids_unique=true replay_diagnostic_count=1 high_uint64_retry_diagnostic_count=1 restored_marker=newest-valid quarantine_marker=corrupt-current backups=%s\n' \
  "$(json_raw "$RECOVERY_REPORT" backupCount)"
printf 'post_sync_crash_exit=%s post_sync_first_learning_count=%s same_event_replay_learning_count=%s different_event_learning_count=%s same_event_replay_select_hook_calls=0 zero_id_learning_count=%s product_name_select_hook_calls=0 ledger_directory_mode=700 ledger_file_mode=600\n' \
  "$POST_SYNC_CRASH_STATUS" "$POST_SYNC_FIRST_COUNT" "$POST_SYNC_REPLAY_COUNT" \
  "$POST_SYNC_DIFFERENT_COUNT" "$ZERO_ID_LEARNING_COUNT"
printf 'pre_select_claim_crash_exit=%s pre_select_claim_hook_calls=1 same_event_replay_learning_count=%s different_event_learning_count=%s same_event_replay_select_hook_calls=0 same_event_replay_committed=true\n' \
  "$CLAIM_CRASH_STATUS" "$CLAIM_REPLAY_LEARNING_COUNT" "$CLAIM_DIFFERENT_LEARNING_COUNT"
printf 'after_select_pre_sync_same_event_replay_learning_count=%s same_event_replay_select_hook_calls=0 same_event_replay_committed=true different_event_learning_count=%s\n' \
  "$CRASH_REPLAY_LEARNING_COUNT" "$RESTART_LEARNING_COUNT"
printf 'contended_ledger_lock=PASS selection_elapsed_nanoseconds=%s committed=true select_hook_calls=0 learning_count=%s post_release_learning_count=%s\n' \
  "$CONTENDED_SELECTION_NS" "$CONTENDED_LEARNING_COUNT" "$CONTENDED_RECOVERY_COUNT"
