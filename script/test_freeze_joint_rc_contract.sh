#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
FREEZE_TOOL="$ROOT_DIR/script/freeze_joint_rc.sh"
JOINT_GATE="$ROOT_DIR/script/run_joint_rc_gates.sh"
OFFICIAL_VALIDATOR="$ROOT_DIR/script/validate_official_input_bundle.sh"
FAULT_GATE="$ROOT_DIR/script/run_rc_fault_drills.sh"
USERDB_GATE="$ROOT_DIR/script/test_userdb_process_recovery.sh"
DOGFOOD_VALIDATOR="$ROOT_DIR/scripts/dogfood_checkpoint.mjs"
TEST_ROOT="$(/usr/bin/mktemp -d /tmp/agent-ime-freeze-contract.XXXXXX)"
TEST_ROOT_INODE="$(/usr/bin/stat -f %i "$TEST_ROOT")"

cleanup() {
  local status="$?"
  trap - EXIT HUP INT TERM
  if [[ "$TEST_ROOT" == /tmp/agent-ime-freeze-contract.* \
    && -d "$TEST_ROOT" && ! -L "$TEST_ROOT" \
    && "$(/usr/bin/stat -f %i "$TEST_ROOT" 2>/dev/null || true)" == "$TEST_ROOT_INODE" ]]; then
    /bin/chmod -R u+w "$TEST_ROOT" 2>/dev/null || true
    /bin/rm -rf "$TEST_ROOT"
  fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

fail() {
  echo "freeze contract test failed: $*" >&2
  exit 1
}

require_literal() {
  local file="$1"
  local literal="$2"
  /usr/bin/grep -Fq -- "$literal" "$file" || fail "missing '$literal' in $(basename "$file")"
}

/bin/bash -n "$FREEZE_TOOL"
/bin/bash -n "$JOINT_GATE"
/bin/bash -n "$OFFICIAL_VALIDATOR"
/bin/bash -n "$FAULT_GATE"
/bin/bash -n "$USERDB_GATE"

require_literal "$FREEZE_TOOL" 'renameatx_np(AT_FDCWD, source, AT_FDCWD, destination, UInt32(RENAME_EXCL))'
require_literal "$FREEZE_TOOL" 'METADATA_BASENAME="FROZEN_ARTIFACTS"'
require_literal "$FREEZE_TOOL" 'EXPECTED_VERSION="$(tr -d '"'"'[:space:]'"'"' < "$ROOT_DIR/VERSION")"'
require_literal "$FREEZE_TOOL" 'AUTHORIZED_TEAM_ID="${AIM_AUTHORIZED_TEAM_ID-}"'
require_literal "$FREEZE_TOOL" 'creating a frozen RC requires AIM_AUTHORIZED_TEAM_ID'
require_literal "$FREEZE_TOOL" 'validate_pair_contract "$SOURCE_INPUT" "$SOURCE_MANAGER" "$AUTHORIZED_TEAM_ID"'
require_literal "$FREEZE_TOOL" 'MINIMUM_BUILD=1020'
require_literal "$FREEZE_TOOL" '/usr/bin/codesign --verify --deep --strict --verbose=2 "$input_bundle"'
require_literal "$FREEZE_TOOL" 'verified_commit="$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$input_commit")"'
require_literal "$FREEZE_TOOL" 'expected_inputs="$("$INPUT_GUARD" fingerprint "$ROOT_DIR" "$input_commit")"'
require_literal "$FREEZE_TOOL" 'cmp -s "$metadata_file" <(emit_frozen_metadata)'
require_literal "$FREEZE_TOOL" "printf 'team_identifier_sha256=%s\\n'"
require_literal "$FREEZE_TOOL" "printf 'leaf_authority_sha256=%s\\n'"
require_literal "$FREEZE_TOOL" 'RC staging and publication target must share a filesystem'
require_literal "$FREEZE_TOOL" '/bin/chmod -R a-w "$STAGE_ROOT"'
require_literal "$FREEZE_TOOL" '\( -perm -0200 -o -perm -0020 -o -perm -0002 \)'
require_literal "$FREEZE_TOOL" '/usr/bin/stat -f %l "$file_path"'
require_literal "$FREEZE_TOOL" '/usr/bin/ditto --norsrc --noextattr --noqtn --noacl'
require_literal "$FREEZE_TOOL" 'assert_canonical_extended_metadata "$directory"'
require_literal "$FREEZE_TOOL" 'assert_no_user_write "$directory"'
require_literal "$FREEZE_TOOL" 'local directory'
require_literal "$JOINT_GATE" '"$FREEZE_TOOL" --verify-frozen "$BUNDLE" "$MANAGER_BUNDLE" >/dev/null'
freeze_verifier_calls="$(/usr/bin/grep -Fc -- '"$FREEZE_TOOL" --verify-frozen "$BUNDLE" "$MANAGER_BUNDLE" >/dev/null' "$JOINT_GATE")"
[[ "$freeze_verifier_calls" -ge 3 ]] || fail "joint gate must reverify the frozen pair through pointer commit"
require_literal "$JOINT_GATE" 'EXECUTION_BUNDLE="$RUNTIME_ROOT/Agent Input Method.inputmethod"'
require_literal "$JOINT_GATE" '/usr/bin/ditto --norsrc --noextattr --noqtn --noacl "$BUNDLE" "$EXECUTION_BUNDLE"'
require_literal "$JOINT_GATE" '/bin/chmod -R u+w "$EXECUTION_BUNDLE"'
require_literal "$JOINT_GATE" '/usr/bin/xattr -cr "$EXECUTION_BUNDLE" "$EXECUTION_METADATA"'
require_literal "$JOINT_GATE" '/bin/chmod -R a-w "$EXECUTION_BUNDLE"'
require_literal "$JOINT_GATE" 'RUNTIME_ROOT_INODE="$(stat -f %i "$RUNTIME_ROOT")"'
require_literal "$JOINT_GATE" '"$(stat -f %i "$RUNTIME_ROOT" 2>/dev/null || true)" == "$RUNTIME_ROOT_INODE"'
require_literal "$JOINT_GATE" 'chmod -R u+w "$RUNTIME_ROOT" || return 1'
require_literal "$JOINT_GATE" 'remove_runtime_root || status=92'
require_literal "$JOINT_GATE" 'RIME="$EXECUTION_BUNDLE/Contents/Resources/Rime"'
require_literal "$JOINT_GATE" 'HOST="$EXECUTION_BUNDLE/Contents/MacOS/AgentInputMethod"'
require_literal "$JOINT_GATE" '"$ROOT_DIR/script/test_userdb_process_recovery.sh" "$EXECUTION_BUNDLE" "$ARTIFACT_SHA"'
require_literal "$JOINT_GATE" '"$ROOT_DIR/script/artifact_checksum.sh" "$EXECUTION_BUNDLE"'
if /usr/bin/grep -Fq -- 'test_userdb_process_recovery.sh" "$BUNDLE"' "$JOINT_GATE"; then
  fail "joint gate may not execute the UserDB process gate from the frozen source bundle"
fi
empty_publish_array_guards="$(/usr/bin/grep -Fc -- 'if (( ${#PUBLISH_TEMPS[@]} > 0 )); then' "$JOINT_GATE")"
[[ "$empty_publish_array_guards" -eq 2 ]] || fail "joint gate cleanup must guard empty Bash 3.2 arrays"
require_literal "$OFFICIAL_VALIDATOR" '\( -perm -0200 -o -perm -0020 -o -perm -0002 \)'
require_literal "$FAULT_GATE" '\( -perm -0200 -o -perm -0020 -o -perm -0002 \)'
require_literal "$DOGFOOD_VALIDATOR" 'const expectedEntries = ["Agent Input Method.app", "Agent Input Method.inputmethod", "FROZEN_ARTIFACTS"].sort();'
require_literal "$DOGFOOD_VALIDATOR" '(status.mode & 0o222) !== 0'
require_literal "$DOGFOOD_VALIDATOR" 'status.nlink !== 1'
require_literal "$DOGFOOD_VALIDATOR" 'frozen RC input and manager must share one directory'
require_literal "$JOINT_GATE" "printf '# team_identifier_sha256=%s\\n# leaf_authority_sha256=%s\\n# run_id=%s\\n'"
require_literal "$OFFICIAL_VALIDATOR" '"# team_identifier_sha256"'
require_literal "$OFFICIAL_VALIDATOR" '"# leaf_authority_sha256"'
require_literal "$FAULT_GATE" '"# team_identifier_sha256"'
require_literal "$FAULT_GATE" '"# leaf_authority_sha256"'
require_literal "$FAULT_GATE" 'extract_leaf_certificate "$INPUT_BUNDLE" "$TEMP_ROOT/frozen-input-leaf-"'
require_literal "$FAULT_GATE" '/usr/bin/codesign --force --sign "$RC_LEAF_IDENTITY_SHA1" --timestamp=none "$IPC"'
require_literal "$FAULT_GATE" '/usr/bin/codesign --verify --strict --verbose=2 "$IPC"'
require_literal "$FAULT_GATE" '[[ "$(bundle_team_id "$IPC")" == "$INPUT_TEAM_ID" ]]'
require_literal "$FAULT_GATE" '[[ "$(bundle_authority "$IPC")" == "$INPUT_AUTHORITY" ]]'
require_literal "$FAULT_GATE" 'cmp -s "$RC_LEAF_CERTIFICATE" "$IPC_LEAF_CERTIFICATE"'
require_literal "$FAULT_GATE" 'ipc_signature=apple-development ipc_leaf_certificate_sha256=%s ipc_team_identifier_sha256=%s ipc_leaf_authority_sha256=%s'
require_literal "$FAULT_GATE" '"$IPC" serve-count "$SOCKET" 6'
require_literal "$FAULT_GATE" 'AIMIPC/1 hello signed-hello'
require_literal "$FAULT_GATE" 'AIMIPC/1 request signed-request synthetic'
require_literal "$FAULT_GATE" 'AIMIPC/1 cancel signed-request'
require_literal "$FAULT_GATE" 'real-payload-disabled-in-v0.3'
require_literal "$FAULT_GATE" 'error unknown protocol-version'
require_literal "$FAULT_GATE" 'signed_stalled_backend_exit=5 configured_timeout_ms=250'
require_literal "$FAULT_GATE" 'unsigned_cargo_test_supplement=PASS'
require_literal "$FAULT_GATE" 'not_deployment_smoke=true'
require_literal "$USERDB_GATE" '"team_identifier_sha256"'
require_literal "$USERDB_GATE" '"leaf_authority_sha256"'
require_literal "$DOGFOOD_VALIDATOR" '"team_identifier_sha256", "leaf_authority_sha256"'
for forbidden_identity_literal in \
  "printf 'team_identifier=%s" \
  "printf '# team_identifier=%s" \
  "printf '# leaf_authority=%s" \
  '"# team_identifier"' \
  '"# leaf_authority"'; do
  if /usr/bin/grep -Fq -- "$forbidden_identity_literal" \
    "$FREEZE_TOOL" "$JOINT_GATE" "$OFFICIAL_VALIDATOR" "$FAULT_GATE" "$USERDB_GATE" "$DOGFOOD_VALIDATOR"; then
    fail "RC evidence producer or validator still persists a raw signing identity: $forbidden_identity_literal"
  fi
done
if /usr/bin/grep -Fq -- '-perm -u+w' "$FREEZE_TOOL" "$OFFICIAL_VALIDATOR" "$FAULT_GATE"; then
  fail "a formal frozen-artifact validator still checks only owner-write permission"
fi
if /usr/bin/grep -Eq -- '\$1 == key \{[^}]*exit|\$1 == "(TeamIdentifier|Authority)" \{[^}]*exit' \
  "$FREEZE_TOOL" "$JOINT_GATE" "$OFFICIAL_VALIDATOR" "$FAULT_GATE" "$USERDB_GATE"; then
  fail "a codesign parser still exits before consuming verbose output under pipefail"
fi
if /usr/bin/grep -Fq -- 'build/native/release/Agent Input Method.inputmethod' "$JOINT_GATE"; then
  fail "joint gate still accepts a mutable build/native default"
fi

ipc_unsigned_line="$(/usr/bin/grep -Fn 'IPC_UNSIGNED_CHECKSUM="$(file_checksum "$IPC")"' "$FAULT_GATE" | /usr/bin/awk -F: 'NR == 1 { print $1 }')"
ipc_sign_line="$(/usr/bin/grep -Fn '/usr/bin/codesign --force --sign "$RC_LEAF_IDENTITY_SHA1" --timestamp=none "$IPC"' "$FAULT_GATE" | /usr/bin/awk -F: 'NR == 1 { print $1 }')"
ipc_verify_line="$(/usr/bin/grep -Fn '/usr/bin/codesign --verify --strict --verbose=2 "$IPC"' "$FAULT_GATE" | /usr/bin/awk -F: 'NR == 1 { print $1 }')"
ipc_signed_line="$(/usr/bin/grep -Fn 'IPC_CHECKSUM="$(file_checksum "$IPC")"' "$FAULT_GATE" | /usr/bin/awk -F: 'NR == 1 { print $1 }')"
[[ "$ipc_unsigned_line" =~ ^[0-9]+$ && "$ipc_sign_line" =~ ^[0-9]+$ \
  && "$ipc_verify_line" =~ ^[0-9]+$ && "$ipc_signed_line" =~ ^[0-9]+$ \
  && "$ipc_unsigned_line" -lt "$ipc_sign_line" \
  && "$ipc_sign_line" -lt "$ipc_verify_line" \
  && "$ipc_verify_line" -lt "$ipc_signed_line" ]] \
  || fail "IPC signing, strict verification, and signed checksum are not ordered fail-closed"

if /usr/bin/grep -Eq -- '/bin/rm -rf "?\$TARGET_DIR|rm -rf "?\$TARGET_DIR' "$FREEZE_TOOL"; then
  fail "freeze tool may delete or replace an existing target"
fi
if /usr/bin/grep -Eq -- '(^|[[:space:]])(/bin/)?(rm|rmdir)[[:space:]].*\$(BUNDLE|MANAGER_BUNDLE|SOURCE_INPUT|SOURCE_MANAGER)' \
  "$FREEZE_TOOL" "$JOINT_GATE"; then
  fail "an RC source or frozen artifact path appears in a deletion command"
fi

if "$FREEZE_TOOL" "$TEST_ROOT/missing.inputmethod" "$TEST_ROOT/missing.app" \
  >"$TEST_ROOT/missing-team.stdout" 2>"$TEST_ROOT/missing-team.stderr"; then
  fail "freeze creation without an authorized TeamIdentifier unexpectedly passed"
fi
/usr/bin/grep -Fq 'requires AIM_AUTHORIZED_TEAM_ID' "$TEST_ROOT/missing-team.stderr" \
  || fail "freeze creation did not fail closed on a missing Team authorization"
if AIM_AUTHORIZED_TEAM_ID=lowercase1 "$FREEZE_TOOL" \
  "$TEST_ROOT/missing.inputmethod" "$TEST_ROOT/missing.app" \
  >"$TEST_ROOT/invalid-team.stdout" 2>"$TEST_ROOT/invalid-team.stderr"; then
  fail "freeze creation with an invalid authorized TeamIdentifier unexpectedly passed"
fi
/usr/bin/grep -Fq 'exactly 10 uppercase ASCII letters/digits' "$TEST_ROOT/invalid-team.stderr" \
  || fail "freeze creation did not reject an invalid Team authorization"
if AIM_AUTHORIZED_TEAM_ID=ABCDEFGHIJ "$FREEZE_TOOL" \
  "$TEST_ROOT/missing.inputmethod" "$TEST_ROOT/missing.app" \
  >"$TEST_ROOT/missing.stdout" 2>"$TEST_ROOT/missing.stderr"; then
  fail "missing source pair unexpectedly passed"
fi
[[ ! -e "$ROOT_DIR/build/rc/.freeze-staging.contract-test" ]] || fail "missing-source check touched repository RC state"

/bin/mkdir "$TEST_ROOT/permissions"
printf 'permission regression\n' > "$TEST_ROOT/permissions/group-writable"
/bin/chmod 0020 "$TEST_ROOT/permissions/group-writable"
[[ -z "$(/usr/bin/find "$TEST_ROOT/permissions" -type f -perm -0200 -print -quit)" ]] \
  || fail "group-only writable fixture unexpectedly has owner-write permission"
[[ -n "$(/usr/bin/find "$TEST_ROOT/permissions" -type f \
  \( -perm -0200 -o -perm -0020 -o -perm -0002 \) -print -quit)" ]] \
  || fail "all-write-bit check missed a group-writable entry"
/bin/chmod -R a-w "$TEST_ROOT/permissions"
[[ -z "$(/usr/bin/find "$TEST_ROOT/permissions" \
  \( -perm -0200 -o -perm -0020 -o -perm -0002 \) -print -quit)" ]] \
  || fail "a-w did not remove every writable mode bit"

/bin/mkdir -p "$TEST_ROOT/runtime-cleanup/readonly"
printf 'temporary execution copy\n' > "$TEST_ROOT/runtime-cleanup/readonly/value"
runtime_cleanup_inode="$(/usr/bin/stat -f %i "$TEST_ROOT/runtime-cleanup")"
/bin/chmod -R a-w "$TEST_ROOT/runtime-cleanup"
[[ -d "$TEST_ROOT/runtime-cleanup" && ! -L "$TEST_ROOT/runtime-cleanup" \
  && "$(/usr/bin/stat -f %i "$TEST_ROOT/runtime-cleanup")" == "$runtime_cleanup_inode" ]] \
  || fail "runtime cleanup fixture identity changed"
/bin/chmod -R u+w "$TEST_ROOT/runtime-cleanup"
/bin/rm -rf "$TEST_ROOT/runtime-cleanup"
[[ ! -e "$TEST_ROOT/runtime-cleanup" && ! -L "$TEST_ROOT/runtime-cleanup" ]] \
  || fail "verified read-only runtime cleanup left a residue"

/bin/mkdir "$TEST_ROOT/copy-source"
printf 'canonical copy\n' > "$TEST_ROOT/copy-source/value"
/usr/bin/xattr -w com.agentime.freeze-contract source-metadata "$TEST_ROOT/copy-source/value"
/bin/chmod +a "$(/usr/bin/id -un) allow write" "$TEST_ROOT/copy-source/value"
/usr/bin/ditto --norsrc --noextattr --noqtn --noacl \
  "$TEST_ROOT/copy-source" "$TEST_ROOT/copy-destination"
if /usr/bin/xattr -p com.agentime.freeze-contract "$TEST_ROOT/copy-destination/value" >/dev/null 2>&1; then
  fail "canonical ditto copy preserved a source extended attribute"
fi
if /bin/ls -lde "$TEST_ROOT/copy-destination/value" \
  | /usr/bin/awk 'NR > 1 && $1 ~ /^[0-9]+:$/ { found=1 } END { exit(found ? 0 : 1) }'; then
  fail "canonical ditto copy preserved a source ACL"
fi
/bin/chmod -RN "$TEST_ROOT/copy-destination"
/usr/bin/xattr -cr "$TEST_ROOT/copy-destination"
while IFS= read -r -d '' copied_entry; do
  while IFS= read -r copied_attribute; do
    [[ -z "$copied_attribute" || "$copied_attribute" == "com.apple.provenance" ]] \
      || fail "canonical frozen tree retained an unapproved extended attribute"
  done < <(/usr/bin/xattr "$copied_entry")
done < <(/usr/bin/find "$TEST_ROOT/copy-destination" -print0)

publish_exclusive() {
  local source="$1"
  local destination="$2"
  /bin/mkdir -p "$TEST_ROOT/swift-module-cache"
  /usr/bin/xcrun swift -module-cache-path "$TEST_ROOT/swift-module-cache" \
    - "$source" "$destination" <<'SWIFT'
import Darwin
import Foundation

let arguments = CommandLine.arguments
guard arguments.count == 3 else { exit(2) }
let result = arguments[1].withCString { source in
    arguments[2].withCString { destination in
        renameatx_np(AT_FDCWD, source, AT_FDCWD, destination, UInt32(RENAME_EXCL))
    }
}
exit(result == 0 ? 0 : 1)
SWIFT
}

/bin/mkdir "$TEST_ROOT/source-one"
printf 'first\n' > "$TEST_ROOT/source-one/value"
publish_exclusive "$TEST_ROOT/source-one" "$TEST_ROOT/published"
[[ ! -e "$TEST_ROOT/source-one" && -f "$TEST_ROOT/published/value" ]] \
  || fail "exclusive publication into an absent target was not atomic"

/bin/mkdir "$TEST_ROOT/source-two"
printf 'second\n' > "$TEST_ROOT/source-two/value"
if publish_exclusive "$TEST_ROOT/source-two" "$TEST_ROOT/published"; then
  fail "exclusive publication overwrote an existing target"
fi
[[ -f "$TEST_ROOT/source-two/value" ]] || fail "failed exclusive publication consumed its source"
[[ "$(<"$TEST_ROOT/published/value")" == "first" ]] || fail "existing target changed after exclusive publication failed"

echo "freeze joint RC contract tests passed"
