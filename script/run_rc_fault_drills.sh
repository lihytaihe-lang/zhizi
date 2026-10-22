#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCT_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
EVIDENCE_ROOT="$ROOT_DIR/docs/current/evidence/v0.2-v0.3"
RC_POINTER="$EVIDENCE_ROOT/RC_ARTIFACT.sha256"
RC_INPUT_GUARD="$ROOT_DIR/script/rc_gate_support.sh"
ARTIFACT_DIR="$EVIDENCE_ROOT/V03-06/artifacts"
ARTIFACT_MANIFEST="$ARTIFACT_DIR/MANIFEST.sha256"

reject_symlink_path() {
  local current="$1"
  [[ "$current" == /* ]] || return 1
  while [[ "$current" != "/" ]]; do
    [[ ! -L "$current" ]] || return 1
    current="$(dirname "$current")"
  done
}

usage() {
  echo "usage: $0 [<frozen-input-bundle> <frozen-manager-bundle> [output-log]]" >&2
  exit 2
}

if [[ $# -eq 0 ]]; then
  [[ -f "$RC_POINTER" && ! -L "$RC_POINTER" ]] || {
    echo "missing regular frozen RC pointer: $RC_POINTER" >&2
    exit 2
  }
  INPUT_BUNDLE="$(awk '/^[0-9a-f]{64}  / { print substr($0, 67); exit }' "$RC_POINTER")"
  MANAGER_BUNDLE="$(awk '/^[0-9a-f]{64}  / { count += 1; if (count == 2) { print substr($0, 67); exit } }' "$RC_POINTER")"
  OUTPUT=""
elif [[ $# -eq 2 || $# -eq 3 ]]; then
  INPUT_BUNDLE="$1"
  MANAGER_BUNDLE="$2"
  OUTPUT="${3:-}"
else
  usage
fi

[[ "$INPUT_BUNDLE" == /* && "$MANAGER_BUNDLE" == /* ]] || {
  echo "fault drill requires absolute frozen artifact paths" >&2
  exit 2
}
[[ -d "$INPUT_BUNDLE" && -d "$MANAGER_BUNDLE" ]] || {
  echo "frozen RC artifacts are missing" >&2
  exit 2
}

INPUT_BUNDLE="$(cd "$INPUT_BUNDLE" && pwd -P)"
MANAGER_BUNDLE="$(cd "$MANAGER_BUNDLE" && pwd -P)"
PRECHECK_INPUT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$INPUT_BUNDLE")"
PRECHECK_MANAGER_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$MANAGER_BUNDLE")"
PRECHECK_BUILD="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$INPUT_BUNDLE/Contents/Info.plist")"
RUN_ID="$(date -u '+%Y%m%dT%H%M%SZ')-$$"
if [[ -z "$OUTPUT" ]]; then
  OUTPUT="$ARTIFACT_DIR/rc-fault-drills-build${PRECHECK_BUILD}-${PRECHECK_INPUT_SHA:0:12}-${RUN_ID}.txt"
fi
[[ "$OUTPUT" == /* ]] || OUTPUT="$ROOT_DIR/$OUTPUT"
OUTPUT_NAME="$(basename "$OUTPUT")"
[[ "$OUTPUT_NAME" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*\.txt$ ]] || {
  echo "fault drill output name is unsafe" >&2
  exit 2
}
[[ -d "$ARTIFACT_DIR" && ! -L "$ARTIFACT_DIR" ]] || { echo "missing regular V03-06 artifact directory" >&2; exit 2; }
reject_symlink_path "$ARTIFACT_DIR" || { echo "symlink in formal evidence path" >&2; exit 2; }
[[ "$(cd "$(dirname "$OUTPUT")" && pwd -P)" == "$(cd "$ARTIFACT_DIR" && pwd -P)" ]] || {
  echo "fault drill output must be a direct V03-06 artifact" >&2
  exit 2
}
[[ -f "$ARTIFACT_MANIFEST" && ! -L "$ARTIFACT_MANIFEST" ]] || {
  echo "missing regular V03-06 artifact manifest" >&2
  exit 2
}
[[ ! -e "$OUTPUT" && ! -L "$OUTPUT" ]] || { echo "fault drill output is immutable and already exists: $OUTPUT" >&2; exit 2; }

SNAPSHOT_CTL="$MANAGER_BUNDLE/Contents/MacOS/snapshot-fixture-ctl"
SNAPSHOT_HARNESS=""
HOST="$INPUT_BUNDLE/Contents/MacOS/AgentInputMethod"
IPC=""
INSTALLER="$ROOT_DIR/script/install_input_method.sh"
INSTALLER_TEST="$ROOT_DIR/script/test_install_input_method.sh"
INSTALLED="$HOME/Library/Input Methods/Agent Input Method.app"
INSTALLED_LEGACY="$HOME/Library/Input Methods/Agent Input Method.inputmethod"
INSTALL_STATE="$HOME/Library/Application Support/AgentInputMethod/InstallState"
EXPECTED_INPUT_CHECKSUM="$PRECHECK_INPUT_SHA"
EXPECTED_MANAGER_CHECKSUM="$PRECHECK_MANAGER_SHA"
EXPECTED_BUILD="$PRECHECK_BUILD"
EXPECTED_TEAM_SHA256=""
EXPECTED_AUTHORITY_SHA256=""
EXPECTED_INSTALLER_CHECKSUM=""
EXPECTED_INSTALLER_TEST_CHECKSUM=""
EXPECTED_POINTER_CHECKSUM=""

TEMP_ROOT="$(mktemp -d /tmp/agent-ime-rc-fault.XXXXXX)"
RAW_LOG="$TEMP_ROOT/fault.log"
IPC_PID=""
HOST_PID=""
SQLITE_LOCK_PID=""
INSTALL_CYCLE_STARTED=0
HAD_ORIGINAL=0
HAD_INSTALL_STATE=0
PRESERVE_TEMP=0
ORIGINAL_KIND="absent"
ORIGINAL_CHECKSUM=""
ORIGINAL_STATE_CHECKSUM=""
DISK_DEVICE=""
SIGNAL_STATUS=0
SUPPORT_GIT_COMMIT=""
SUPPORT_SOURCE_CHECKSUM=""
SNAPSHOT_HARNESS_CHECKSUM=""
IPC_CHECKSUM=""
IPC_UNSIGNED_CHECKSUM=""
IPC_LEAF_CERTIFICATE_SHA256=""
IPC_TEAM_IDENTIFIER_SHA256=""
IPC_LEAF_AUTHORITY_SHA256=""
SWIFT_BIN_PATH=""
SWIFT_TEST_BUNDLE=""
SWIFT_TEST_EXECUTABLE=""
SWIFT_TEST_CHECKSUM=""
FAULT_COMPLETED=0
FAULT_LEASE_HELD=0
FAULT_LEASE_DIR="$HOME/Library/Application Support/AgentInputMethod/.install-fault-drill.lock"
FAULT_LEASE_TOKEN="$(uuidgen | tr '[:upper:]' '[:lower:]')-$$"
TEE_PID=""
HOST_STATE_CAPTURED=0
HOST_WAS_RUNNING=0

exec 3>&1 4>&2
exec > >(tee "$RAW_LOG") 2>&1
TEE_PID=$!

remove_tree() {
  local path="$1"
  [[ -e "$path" || -L "$path" ]] || return 0
  if [[ -L "$path" ]]; then rm -f "$path"; return 0; fi
  if [[ -d "$path" ]]; then
    find "$path" -type d -exec chmod u+rwx {} + 2>/dev/null || true
  fi
  rm -rf "$path"
}

acquire_fault_lease() {
  local parent
  parent="$(dirname "$FAULT_LEASE_DIR")"
  reject_symlink_path "$parent" || return 1
  [[ -d "$parent" && ! -L "$parent" ]] || {
    echo "fault drill requires an existing regular AgentInputMethod state parent" >&2
    return 4
  }
  [[ ! -e "$FAULT_LEASE_DIR" && ! -L "$FAULT_LEASE_DIR" ]] || {
    echo "another fault drill owns the installer state: $FAULT_LEASE_DIR" >&2
    return 6
  }
  mkdir "$FAULT_LEASE_DIR" || return 6
  FAULT_LEASE_HELD=1
  chmod 700 "$FAULT_LEASE_DIR" || { rmdir "$FAULT_LEASE_DIR"; FAULT_LEASE_HELD=0; return 1; }
  if ! printf '%s\n' "$FAULT_LEASE_TOKEN" > "$FAULT_LEASE_DIR/token" ||
     ! printf '%s\n' "$$" > "$FAULT_LEASE_DIR/pid"; then
    rm -f "$FAULT_LEASE_DIR/token" "$FAULT_LEASE_DIR/pid"
    rmdir "$FAULT_LEASE_DIR" 2>/dev/null || true
    FAULT_LEASE_HELD=0
    return 1
  fi
  chmod 600 "$FAULT_LEASE_DIR/token" "$FAULT_LEASE_DIR/pid" || return 1
}

release_fault_lease() {
  [[ "$FAULT_LEASE_HELD" -eq 1 ]] || return 0
  [[ -d "$FAULT_LEASE_DIR" && ! -L "$FAULT_LEASE_DIR" &&
     -f "$FAULT_LEASE_DIR/token" && ! -L "$FAULT_LEASE_DIR/token" &&
     "$(stat -f '%l' "$FAULT_LEASE_DIR/token")" -eq 1 &&
     -f "$FAULT_LEASE_DIR/pid" && ! -L "$FAULT_LEASE_DIR/pid" &&
     "$(stat -f '%l' "$FAULT_LEASE_DIR/pid")" -eq 1 ]] || return 1
  [[ "$(awk 'NR == 1 { print; exit }' "$FAULT_LEASE_DIR/token")" == "$FAULT_LEASE_TOKEN" ]] || return 1
  rm -f "$FAULT_LEASE_DIR/token" "$FAULT_LEASE_DIR/pid" || return 1
  rmdir "$FAULT_LEASE_DIR" || return 1
  FAULT_LEASE_HELD=0
}

publish_success_log() (
  set -euo pipefail
  local output_temp manifest_temp manifest_backup output_name checksum publish_lock manifest_before_sha
  output_name="$(basename "$OUTPUT")"
  publish_lock="$ARTIFACT_DIR/.fault-publish.lock"
  [[ ! -L "$publish_lock" ]] || return 1
  mkdir "$publish_lock" 2>/dev/null || return 1
  trap 'rmdir "$publish_lock" 2>/dev/null || true' EXIT
  (cd "$ARTIFACT_DIR" && shasum -a 256 -c "$(basename "$ARTIFACT_MANIFEST")") >/dev/null || return 1
  manifest_before_sha="$(file_checksum "$ARTIFACT_MANIFEST")" || return 1
  output_temp="$(mktemp "$ARTIFACT_DIR/.fault-log.XXXXXX")" || return 1
  manifest_temp="$(mktemp "$ARTIFACT_DIR/.manifest.XXXXXX")" || { rm -f "$output_temp"; return 1; }
  manifest_backup="$(mktemp "$ARTIFACT_DIR/.manifest-backup.XXXXXX")" || {
    rm -f "$output_temp" "$manifest_temp"
    return 1
  }
  cp -p "$ARTIFACT_MANIFEST" "$manifest_backup" || {
    rm -f "$output_temp" "$manifest_temp" "$manifest_backup"
    return 1
  }
  if ! cp "$RAW_LOG" "$output_temp" || ! chmod 444 "$output_temp"; then
    rm -f "$output_temp" "$manifest_temp" "$manifest_backup"
    return 1
  fi
  checksum="$(file_checksum "$output_temp")" || { rm -f "$output_temp" "$manifest_temp" "$manifest_backup"; return 1; }
  if ! awk -v target="./$output_name" '$2 != target { print }' "$ARTIFACT_MANIFEST" > "$manifest_temp" ||
     ! printf '%s  ./%s\n' "$checksum" "$output_name" >> "$manifest_temp" ||
     ! LC_ALL=C sort -o "$manifest_temp" "$manifest_temp" ||
     ! chmod 644 "$manifest_temp"; then
    rm -f "$output_temp" "$manifest_temp" "$manifest_backup"
    return 1
  fi
  [[ "$(file_checksum "$ARTIFACT_MANIFEST")" == "$manifest_before_sha" ]] || {
    rm -f "$output_temp" "$manifest_temp" "$manifest_backup"
    return 1
  }
  [[ ! -e "$OUTPUT" && ! -L "$OUTPUT" ]] || { rm -f "$output_temp" "$manifest_temp" "$manifest_backup"; return 1; }
  if ! mv "$output_temp" "$OUTPUT"; then
    rm -f "$output_temp" "$manifest_temp" "$manifest_backup"
    return 1
  fi
  if ! mv -f "$manifest_temp" "$ARTIFACT_MANIFEST"; then
    chmod u+w "$OUTPUT" 2>/dev/null || true
    rm -f "$OUTPUT" "$manifest_temp"
    mv -f "$manifest_backup" "$ARTIFACT_MANIFEST" 2>/dev/null || true
    return 1
  fi
  if ! (cd "$ARTIFACT_DIR" && shasum -a 256 -c "$(basename "$ARTIFACT_MANIFEST")") >/dev/null; then
    chmod u+w "$OUTPUT" 2>/dev/null || true
    rm -f "$OUTPUT"
    mv -f "$manifest_backup" "$ARTIFACT_MANIFEST" || return 1
    return 1
  fi
  rm -f "$manifest_backup"
)

bundle_build() {
  /usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$1/Contents/Info.plist"
}

bundle_value() {
  /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist"
}

bundle_team_id() {
  codesign -dv --verbose=4 "$1" 2>&1 | awk -F= '
    $1 == "TeamIdentifier" && !found { value=$2; found=1 }
    END { if (found) print value; else exit 3 }
  '
}

bundle_authority() {
  codesign -dv --verbose=4 "$1" 2>&1 | awk -F= '
    $1 == "Authority" && !found { value=substr($0, length($1) + 2); found=1 }
    END { if (found) print value; else exit 3 }
  '
}

extract_leaf_certificate() {
  local signed_path="$1" prefix="$2" leaf="${2}0"
  [[ -e "$signed_path" && ! -L "$signed_path" && ! -e "$leaf" && ! -L "$leaf" ]] || return 1
  /usr/bin/codesign --display --extract-certificates "$prefix" "$signed_path" >/dev/null 2>&1 || return 1
  [[ -f "$leaf" && ! -L "$leaf" && "$(stat -f '%l' "$leaf")" -eq 1 ]] || return 1
  printf '%s\n' "$leaf"
}

directory_checksum() {
  "$ROOT_DIR/script/artifact_checksum.sh" "$1"
}

file_checksum() {
  shasum -a 256 "$1" | awk '{ print $1 }'
}

text_checksum() {
  shasum -a 256 | awk '{ print $1 }'
}

support_source_checksum() {
  local file checksum untracked manifest="$TEMP_ROOT/support-source.manifest"
  untracked="$(git -C "$ROOT_DIR" ls-files --others --exclude-standard -- native/macos native/ipc)"
  if [[ -n "$untracked" ]]; then
    echo "untracked native support source prevents a reproducible harness pin:" >&2
    printf '%s\n' "$untracked" >&2
    return 1
  fi
  : > "$manifest"
  while IFS= read -r file; do
    [[ -f "$ROOT_DIR/$file" ]] || continue
    checksum="$(file_checksum "$ROOT_DIR/$file")" || return 1
    printf '%s  %s\n' "$checksum" "$file" >> "$manifest"
  done < <(git -C "$ROOT_DIR" ls-files -- native/macos native/ipc)
  [[ -s "$manifest" ]] || return 1
  file_checksum "$manifest"
}

verify_support_tool_pins() {
  [[ "$(file_checksum "$INSTALLER")" == "$EXPECTED_INSTALLER_CHECKSUM" ]] || return 1
  [[ "$(file_checksum "$INSTALLER_TEST")" == "$EXPECTED_INSTALLER_TEST_CHECKSUM" ]] || return 1
  [[ "$(file_checksum "$SNAPSHOT_HARNESS")" == "$SNAPSHOT_HARNESS_CHECKSUM" ]] || return 1
  [[ "$(file_checksum "$IPC")" == "$IPC_CHECKSUM" ]] || return 1
  /usr/bin/codesign --verify --strict "$IPC" >/dev/null 2>&1 || return 1
  [[ "$(bundle_team_id "$IPC")" == "$INPUT_TEAM_ID" ]] || return 1
  [[ "$(bundle_authority "$IPC")" == "$INPUT_AUTHORITY" ]] || return 1
  [[ "$(printf '%s' "$(bundle_team_id "$IPC")" | text_checksum)" == "$IPC_TEAM_IDENTIFIER_SHA256" ]] || return 1
  [[ "$(printf '%s' "$(bundle_authority "$IPC")" | text_checksum)" == "$IPC_LEAF_AUTHORITY_SHA256" ]] || return 1
  [[ "$(file_checksum "$SWIFT_TEST_EXECUTABLE")" == "$SWIFT_TEST_CHECKSUM" ]] || return 1
  [[ "$(file_checksum "$RC_POINTER")" == "$EXPECTED_POINTER_CHECKSUM" ]] || return 1
  [[ "$($ROOT_DIR/script/artifact_checksum.sh "$INPUT_BUNDLE")" == "$EXPECTED_INPUT_CHECKSUM" ]] || return 1
  [[ "$($ROOT_DIR/script/artifact_checksum.sh "$MANAGER_BUNDLE")" == "$EXPECTED_MANAGER_CHECKSUM" ]] || return 1
  [[ "$($RC_INPUT_GUARD verify-inputs "$ROOT_DIR" "$PRODUCT_COMMIT")" == "$PRODUCT_COMMIT" ]] || return 1
}

restore_state_snapshot() {
  local parent stage old restored_checksum
  parent="$(dirname "$INSTALL_STATE")"
  stage="$parent/.InstallState.fault-restore.$$"
  old="$parent/.InstallState.fault-old.$$"
  remove_tree "$stage" || return 1
  remove_tree "$old" || return 1
  mkdir -p "$parent" || return 1
  if [[ "$HAD_INSTALL_STATE" -eq 1 ]]; then
    ditto "$TEMP_ROOT/original-install-state" "$stage" || return 1
    [[ "$(directory_checksum "$stage")" == "$ORIGINAL_STATE_CHECKSUM" ]] || return 1
  fi
  if [[ -e "$INSTALL_STATE" ]]; then mv "$INSTALL_STATE" "$old" || return 1; fi
  if [[ "$HAD_INSTALL_STATE" -eq 1 ]]; then
    if ! mv "$stage" "$INSTALL_STATE"; then
      [[ ! -e "$old" ]] || mv "$old" "$INSTALL_STATE"
      return 1
    fi
    if restored_checksum="$(directory_checksum "$INSTALL_STATE")"; then
      :
    else
      remove_tree "$INSTALL_STATE" || true
      [[ ! -e "$old" ]] || mv "$old" "$INSTALL_STATE"
      return 1
    fi
    if [[ "$restored_checksum" != "$ORIGINAL_STATE_CHECKSUM" ]]; then
      remove_tree "$INSTALL_STATE" || return 1
      [[ ! -e "$old" ]] || mv "$old" "$INSTALL_STATE"
      return 1
    fi
  fi
  remove_tree "$old" || return 1
}

canonical_existing_file() {
  local path="$1" parent
  [[ "$path" == /* && -f "$path" && ! -L "$path" ]] || return 1
  parent="$(cd "$(dirname "$path")" && pwd -P)" || return 1
  printf '%s/%s\n' "$parent" "$(basename "$path")"
}

real_host_executable_path() {
  local pid="$1" output path
  [[ -x /usr/sbin/lsof ]] || return 2
  output="$(/usr/sbin/lsof -a -p "$pid" -d txt -Fn 2>/dev/null)" || return 2
  path="$(printf '%s\n' "$output" | awk 'substr($0, 1, 1) == "n" { print substr($0, 2); exit }')"
  [[ -n "$path" ]] || return 2
  printf '%s\n' "$path"
}

real_host_pid_matches_installed_bundle() {
  local pid="$1" actual status bundle expected
  if actual="$(real_host_executable_path "$pid")"; then
    :
  else
    kill -0 "$pid" 2>/dev/null || return 1
    return 2
  fi
  actual="$(canonical_existing_file "$actual")" || {
    kill -0 "$pid" 2>/dev/null || return 1
    return 2
  }
  for bundle in "$INSTALLED" "$INSTALLED_LEGACY"; do
    expected="$(canonical_existing_file "$bundle/Contents/MacOS/AgentInputMethod" 2>/dev/null)" || continue
    [[ "$actual" == "$expected" ]] && return 0
  done
  return 1
}

collect_real_input_host_pids() {
  local pids status pid found=0
  if pids="$(pgrep -U "$(id -u)" -x AgentInputMethod 2>/dev/null)"; then
    :
  else
    status=$?
    [[ "$status" -eq 1 ]] && return 1
    return 2
  fi
  while IFS= read -r pid; do
    [[ -n "$pid" ]] || continue
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 2
    if real_host_pid_matches_installed_bundle "$pid"; then
      printf '%s\n' "$pid"
      found=1
    else
      status=$?
      [[ "$status" -eq 1 ]] || return 2
    fi
  done <<< "$pids"
  [[ "$found" -eq 1 ]]
}

probe_real_input_host() {
  collect_real_input_host_pids >/dev/null
}

capture_real_host_state() {
  local probe_status
  if probe_real_input_host; then
    HOST_WAS_RUNNING=1
  else
    probe_status=$?
    [[ "$probe_status" -eq 1 ]] || return 1
    HOST_WAS_RUNNING=0
  fi
  HOST_STATE_CAPTURED=1
  printf 'initial_input_host_running=%s\n' "$HOST_WAS_RUNNING"
}

restore_real_host_state() {
  local probe_status
  [[ "$HOST_STATE_CAPTURED" -eq 1 && "$HOST_WAS_RUNNING" -eq 1 ]] || return 0
  if probe_real_input_host; then return 0; else probe_status=$?; fi
  [[ "$probe_status" -eq 1 ]] || return 1
  [[ -d "$INSTALLED" && ! -L "$INSTALLED" ]] || return 1
  /usr/bin/open -g "$INSTALLED" || return 1
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25; do
    if probe_real_input_host; then
      printf 'restored_initial_input_host_running=true\n'
      return 0
    else
      probe_status=$?
      [[ "$probe_status" -eq 1 ]] || return 1
    fi
    sleep 0.2
  done
  return 1
}

restore_initial_install() {
  local recovery_status restore_status
  capture_status "$TEMP_ROOT/restore-recover.txt" env AIM_INSTALL_FAULT_LEASE_TOKEN="$FAULT_LEASE_TOKEN" \
    "$INSTALLER" --recover
  recovery_status="$CAPTURED_STATUS"
  [[ "$recovery_status" -eq 0 ]] || return "$recovery_status"
  if [[ "$HAD_ORIGINAL" -eq 1 ]]; then
    [[ "$ORIGINAL_KIND" == "app" ]] || return 91
    capture_status "$TEMP_ROOT/restore-install.txt" env AIM_INSTALL_FAULT_LEASE_TOKEN="$FAULT_LEASE_TOKEN" \
      "$INSTALLER" --install "$INPUT_BUNDLE" "$RC_POINTER"
    restore_status="$CAPTURED_STATUS"
    [[ "$restore_status" -eq 0 ]] || return "$restore_status"
    [[ -d "$INSTALLED" ]] || return 91
    [[ "$(directory_checksum "$INSTALLED")" == "$ORIGINAL_CHECKSUM" ]] || return 91
  else
    capture_status "$TEMP_ROOT/restore-uninstall.txt" env AIM_INSTALL_FAULT_LEASE_TOKEN="$FAULT_LEASE_TOKEN" \
      "$INSTALLER" --uninstall
    restore_status="$CAPTURED_STATUS"
    [[ "$restore_status" -eq 0 ]] || return "$restore_status"
    [[ ! -e "$INSTALLED" && ! -e "$INSTALLED_LEGACY" ]] || return 91
  fi
  restore_state_snapshot || return 92
  restore_real_host_state || return 94
}

terminate_owned_child() {
  local pid="$1" signal="$2" parent
  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 0
  parent="$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')"
  [[ "$parent" == "$$" ]] || return 0
  kill "-$signal" "$pid" 2>/dev/null || true
}

cleanup() {
  local status=$? restore_status=0
  if [[ "$SIGNAL_STATUS" -ne 0 ]]; then status="$SIGNAL_STATUS"; fi
  trap - EXIT
  trap '' HUP INT TERM
  set +e
  if [[ -n "$IPC_PID" ]] && kill -0 "$IPC_PID" 2>/dev/null; then terminate_owned_child "$IPC_PID" TERM; fi
  if [[ -n "$HOST_PID" ]] && kill -0 "$HOST_PID" 2>/dev/null; then terminate_owned_child "$HOST_PID" KILL; fi
  exec 9>&- 2>/dev/null || true
  if [[ -n "$SQLITE_LOCK_PID" ]] && kill -0 "$SQLITE_LOCK_PID" 2>/dev/null; then
    terminate_owned_child "$SQLITE_LOCK_PID" TERM
  fi
  if [[ -n "$DISK_DEVICE" ]]; then hdiutil detach "$DISK_DEVICE" -force >/dev/null 2>&1 || true; DISK_DEVICE=""; fi
  if [[ "$status" -ne 0 && "$INSTALL_CYCLE_STARTED" -eq 1 ]]; then
    echo "fault drill failed during install cycle; restoring initial Agent Input Method state" >&2
    if restore_initial_install; then
      echo "fault drill initial installation and state restored exactly" >&2
    else
      restore_status=$?
      PRESERVE_TEMP=1
      echo "fault drill recovery failed status=$restore_status; evidence retained at $TEMP_ROOT" >&2
      status=93
    fi
  fi
  if ! release_fault_lease; then
    PRESERVE_TEMP=1
    echo "fault drill lease release failed; manual inspection required at $FAULT_LEASE_DIR" >&2
    status=92
  fi
  if [[ "$FAULT_COMPLETED" -ne 1 && "$status" -eq 0 ]]; then status=95; fi
  if [[ "$status" -ne 0 || "$FAULT_COMPLETED" -ne 1 ]]; then
    PRESERVE_TEMP=1
  fi

  exec 1>&3 2>&4
  exec 3>&- 4>&-
  if [[ -n "$TEE_PID" ]]; then wait "$TEE_PID" || status=94; fi
  if [[ "$status" -eq 0 && "$FAULT_COMPLETED" -eq 1 ]]; then
    if publish_success_log; then
      printf 'fault drill evidence published atomically: %s\n' "$OUTPUT"
      printf 'the frozen RC pointer remains valid: appended fault evidence does not alter the seven-report pointer closure\n'
    else
      status=94
      PRESERVE_TEMP=1
      printf 'fault drill passed but evidence publication failed; raw log retained at %s\n' "$RAW_LOG" >&2
    fi
  elif [[ "$PRESERVE_TEMP" -eq 1 ]]; then
    printf 'fault drill did not publish formal evidence; diagnostic data retained at %s\n' "$TEMP_ROOT" >&2
  fi
  if [[ "$PRESERVE_TEMP" -eq 0 ]]; then
    chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true
    rm -rf "$TEMP_ROOT"
  fi
  exit "$status"
}
trap cleanup EXIT

handle_signal() {
  SIGNAL_STATUS="$1"
  exit "$1"
}
trap 'handle_signal 129' HUP
trap 'handle_signal 130' INT
trap 'handle_signal 143' TERM

require_file() {
  [[ -x "$1" ]] || { echo "missing executable: $1" >&2; exit 2; }
}

capture_status() {
  local output_file="$1"
  shift
  if "$@" > "$output_file" 2>&1; then
    CAPTURED_STATUS=0
  else
    CAPTURED_STATUS=$?
  fi
}

wait_for_path() {
  local path="$1" kind="$2"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if [[ "$kind" == "socket" && -S "$path" ]]; then return 0; fi
    if [[ "$kind" == "file" && -f "$path" ]]; then return 0; fi
    sleep 0.2
  done
  return 1
}

bundle_version() {
  /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$1/Contents/Info.plist"
}

other_input_methods_inventory() {
  local manifest="$1" candidate name checksum
  : > "$manifest"
  for candidate in "$HOME/Library/Input Methods"/*.app "$HOME/Library/Input Methods"/*.inputmethod; do
    [[ -d "$candidate" ]] || continue
    name="$(basename "$candidate")"
    case "$name" in
      "Agent Input Method.app"|"Agent Input Method.inputmethod") continue ;;
    esac
    checksum="$($ROOT_DIR/script/artifact_checksum.sh "$candidate")" || return 1
    printf '%s  %s\n' "$checksum" "$name" >> "$manifest"
  done
  LC_ALL=C sort -o "$manifest" "$manifest" || return 1
  local count digest
  count="$(wc -l < "$manifest" | tr -d ' ')" || return 1
  digest="$(shasum -a 256 "$manifest" | awk '{ print $1 }')" || return 1
  printf '%s:%s\n' "$count" "$digest"
}

run_install_step() {
  local label="$1" expected_checksum="$2"
  shift 2
  capture_status "$TEMP_ROOT/install-$label.txt" env AIM_INSTALL_FAULT_LEASE_TOKEN="$FAULT_LEASE_TOKEN" \
    "$INSTALLER" "$@"
  [[ "$CAPTURED_STATUS" -eq 0 ]] || {
    sed "s#$HOME#~#g" "$TEMP_ROOT/install-$label.txt" >&2
    echo "install_cycle_step=$label exit=$CAPTURED_STATUS" >&2
    exit "$CAPTURED_STATUS"
  }
  [[ -d "$INSTALLED" ]]
  installed_checksum="$($ROOT_DIR/script/artifact_checksum.sh "$INSTALLED")"
  [[ "$installed_checksum" == "$expected_checksum" ]] || { echo "install_cycle_step=$label checksum mismatch" >&2; exit 4; }
  printf 'install_cycle_step=%s exit=0 checksum=%s\n' "$label" "$installed_checksum"
}

printf '[artifact-preflight]\n'
[[ -x "$RC_INPUT_GUARD" ]]
[[ -f "$RC_POINTER" && ! -L "$RC_POINTER" ]]
"$ROOT_DIR/script/validate_official_input_bundle.sh" "$INPUT_BUNDLE" "$RC_POINTER"
sandbox-exec -p '(version 1) (allow default) (deny network*)' node --input-type=module - \
  "$ROOT_DIR/scripts/dogfood_checkpoint.mjs" "$RC_POINTER" <<'NODE'
import { pathToFileURL } from "node:url";

const modulePath = pathToFileURL(process.argv[2]).href;
const pointerPath = process.argv[3];
const { inspectFrozenReleasePointer } = await import(modulePath);
inspectFrozenReleasePointer(pointerPath);
NODE
EXPECTED_TEAM_SHA256="$(awk -F= '$1 == "# team_identifier_sha256" { count += 1; value = $2 } END { if (count == 1) print value; else exit 3 }' "$RC_POINTER")"
EXPECTED_AUTHORITY_SHA256="$(awk -F= '$1 == "# leaf_authority_sha256" { count += 1; value = $2 } END { if (count == 1) print value; else exit 3 }' "$RC_POINTER")"
[[ "$EXPECTED_TEAM_SHA256" =~ ^[0-9a-f]{64}$ \
  && "$EXPECTED_AUTHORITY_SHA256" =~ ^[0-9a-f]{64}$ ]] \
  || { echo "frozen RC pointer signing identity hashes are invalid" >&2; exit 3; }
[[ "$EXPECTED_BUILD" =~ ^[1-9][0-9]{0,8}$ ]] && (( 10#$EXPECTED_BUILD >= 1020 ))
[[ "$(bundle_value "$INPUT_BUNDLE" CFBundleIdentifier)" == "com.hongye.facai.inputmethod.zhizi" ]]
[[ "$(bundle_value "$MANAGER_BUNDLE" CFBundleIdentifier)" == "com.agentinputlayer.desktop" ]]
[[ "$(bundle_version "$INPUT_BUNDLE")" == "$PRODUCT_VERSION" ]]
[[ "$(bundle_version "$MANAGER_BUNDLE")" == "$PRODUCT_VERSION" ]]
[[ "$(bundle_build "$MANAGER_BUNDLE")" == "$EXPECTED_BUILD" ]]
PRODUCT_COMMIT="$(bundle_value "$INPUT_BUNDLE" AgentIMEProductCommit)"
MANAGER_COMMIT="$(bundle_value "$MANAGER_BUNDLE" AgentIMEProductCommit)"
PRODUCT_INPUTS_SHA256="$(bundle_value "$INPUT_BUNDLE" AgentIMEProductInputsSHA256)"
MANAGER_INPUTS_SHA256="$(bundle_value "$MANAGER_BUNDLE" AgentIMEProductInputsSHA256)"
[[ "$PRODUCT_COMMIT" =~ ^[0-9a-f]{40}$ && "$MANAGER_COMMIT" == "$PRODUCT_COMMIT" ]]
[[ "$PRODUCT_INPUTS_SHA256" =~ ^[0-9a-f]{64}$ && "$MANAGER_INPUTS_SHA256" == "$PRODUCT_INPUTS_SHA256" ]]
[[ "$($RC_INPUT_GUARD verify-inputs "$ROOT_DIR" "$PRODUCT_COMMIT")" == "$PRODUCT_COMMIT" ]]
[[ "$($RC_INPUT_GUARD fingerprint "$ROOT_DIR" "$PRODUCT_COMMIT")" == "$PRODUCT_INPUTS_SHA256" ]]
codesign --verify --deep --strict "$INPUT_BUNDLE"
codesign --verify --deep --strict "$MANAGER_BUNDLE"
INPUT_AUTHORITY="$(bundle_authority "$INPUT_BUNDLE")"
MANAGER_AUTHORITY="$(bundle_authority "$MANAGER_BUNDLE")"
INPUT_TEAM_ID="$(bundle_team_id "$INPUT_BUNDLE")"
MANAGER_TEAM_ID="$(bundle_team_id "$MANAGER_BUNDLE")"
[[ "$INPUT_AUTHORITY" == "Apple Development:"* && "$MANAGER_AUTHORITY" == "$INPUT_AUTHORITY" ]]
[[ "$INPUT_TEAM_ID" =~ ^[A-Z0-9]{10}$ && "$MANAGER_TEAM_ID" == "$INPUT_TEAM_ID" ]]
[[ "$(printf '%s' "$INPUT_TEAM_ID" | text_checksum)" == "$EXPECTED_TEAM_SHA256" ]]
[[ "$(printf '%s' "$INPUT_AUTHORITY" | text_checksum)" == "$EXPECTED_AUTHORITY_SHA256" ]]
if [[ -n "${AIM_AUTHORIZED_TEAM_ID-}" ]]; then
  [[ "${AIM_AUTHORIZED_TEAM_ID}" =~ ^[A-Z0-9]{10}$ && "${AIM_AUTHORIZED_TEAM_ID}" == "$INPUT_TEAM_ID" ]] \
    || { echo "AIM_AUTHORIZED_TEAM_ID does not match the frozen RC TeamIdentifier" >&2; exit 3; }
fi
POINTER_INPUT_SHA="$(awk '/^[0-9a-f]{64}  / { print $1; exit }' "$RC_POINTER")"
POINTER_MANAGER_SHA="$(awk '/^[0-9a-f]{64}  / { count += 1; if (count == 2) { print $1; exit } }' "$RC_POINTER")"
POINTER_INPUT_PATH="$(awk '/^[0-9a-f]{64}  / { print substr($0, 67); exit }' "$RC_POINTER")"
POINTER_MANAGER_PATH="$(awk '/^[0-9a-f]{64}  / { count += 1; if (count == 2) { print substr($0, 67); exit } }' "$RC_POINTER")"
[[ "$POINTER_INPUT_SHA" == "$EXPECTED_INPUT_CHECKSUM" && "$POINTER_MANAGER_SHA" == "$EXPECTED_MANAGER_CHECKSUM" ]]
[[ -d "$POINTER_INPUT_PATH" && -d "$POINTER_MANAGER_PATH" ]]
[[ "$(cd "$POINTER_INPUT_PATH" && pwd -P)" == "$INPUT_BUNDLE" ]]
[[ "$(cd "$POINTER_MANAGER_PATH" && pwd -P)" == "$MANAGER_BUNDLE" ]]
[[ "$(awk -F= '$1 == "# artifact_build" { print $2; exit }' "$RC_POINTER")" == "$EXPECTED_BUILD" ]]
[[ "$(awk -F= '$1 == "# product_commit" { print $2; exit }' "$RC_POINTER")" == "$PRODUCT_COMMIT" ]]
[[ "$(awk -F= '$1 == "# product_inputs_sha256" { print $2; exit }' "$RC_POINTER")" == "$PRODUCT_INPUTS_SHA256" ]]
[[ "$(awk -F= '$1 == "# team_identifier_sha256" { print $2; exit }' "$RC_POINTER")" == "$EXPECTED_TEAM_SHA256" ]]
[[ "$(awk -F= '$1 == "# leaf_authority_sha256" { print $2; exit }' "$RC_POINTER")" == "$EXPECTED_AUTHORITY_SHA256" ]]
EXPECTED_INSTALLER_CHECKSUM="$(file_checksum "$INSTALLER")"
EXPECTED_INSTALLER_TEST_CHECKSUM="$(file_checksum "$INSTALLER_TEST")"
EXPECTED_POINTER_CHECKSUM="$(file_checksum "$RC_POINTER")"
printf 'artifact_checksum=%s manager_checksum=%s build=%s product_commit=%s product_inputs_sha256=%s team_identifier_sha256=%s leaf_authority_sha256=%s\n' \
  "$EXPECTED_INPUT_CHECKSUM" "$EXPECTED_MANAGER_CHECKSUM" "$EXPECTED_BUILD" "$PRODUCT_COMMIT" \
  "$PRODUCT_INPUTS_SHA256" "$EXPECTED_TEAM_SHA256" "$EXPECTED_AUTHORITY_SHA256"

printf '[support-harness-build]\n'
SUPPORT_GIT_COMMIT="$(git -C "$ROOT_DIR" rev-parse HEAD)"
[[ "$SUPPORT_GIT_COMMIT" == "$PRODUCT_COMMIT" ]]
SUPPORT_SOURCE_CHECKSUM="$(support_source_checksum)"
OFFLINE_SANDBOX='(version 1) (allow default) (deny network*)'
PINNED_RIME_DIR="$ROOT_DIR/.build-deps/librime/1.16.1"
PINNED_PINYIN_DIR="$ROOT_DIR/.build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0"
sandbox-exec -p "$OFFLINE_SANDBOX" bash -c \
  'source "$1"; validate_rime_dir "$2"; validate_pinyin_dir "$3"' \
  _ "$ROOT_DIR/script/bootstrap_native.sh" "$PINNED_RIME_DIR" "$PINNED_PINYIN_DIR"
cp "$PINNED_RIME_DIR/lib/librime.1.dylib" "$TEMP_ROOT/pinned-librime-unsigned.dylib"
cp "$INPUT_BUNDLE/Contents/Frameworks/librime.1.dylib" "$TEMP_ROOT/bundled-librime-unsigned.dylib"
chmod u+w "$TEMP_ROOT/pinned-librime-unsigned.dylib" "$TEMP_ROOT/bundled-librime-unsigned.dylib"
codesign --remove-signature "$TEMP_ROOT/pinned-librime-unsigned.dylib" 2>/dev/null || true
codesign --remove-signature "$TEMP_ROOT/bundled-librime-unsigned.dylib" 2>/dev/null || true
cmp -s "$TEMP_ROOT/pinned-librime-unsigned.dylib" "$TEMP_ROOT/bundled-librime-unsigned.dylib"
CORE_TARGET="$TEMP_ROOT/core-target"
SWIFT_SCRATCH="$TEMP_ROOT/swift-build"
IPC_TARGET="$TEMP_ROOT/ipc-target"
CARGO_HOME_DIR="$TEMP_ROOT/cargo-home"
SWIFT_CACHE="$TEMP_ROOT/swift-cache"
SWIFT_CONFIG="$TEMP_ROOT/swift-config"
SWIFT_SECURITY="$TEMP_ROOT/swift-security"
SWIFT_HOME="$TEMP_ROOT/swift-home"
SWIFT_MODULE_CACHE="$TEMP_ROOT/swift-module-cache"
mkdir -p "$CARGO_HOME_DIR" "$SWIFT_CACHE" "$SWIFT_CONFIG" "$SWIFT_SECURITY" \
  "$SWIFT_HOME" "$SWIFT_MODULE_CACHE"
sandbox-exec -p "$OFFLINE_SANDBOX" env CARGO_HOME="$CARGO_HOME_DIR" CARGO_NET_OFFLINE=true \
  CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$CORE_TARGET" cargo build --locked --offline --release \
  --manifest-path "$ROOT_DIR/native/core/Cargo.toml" --package agent-ime-core
SWIFT_ARGS=(
  --package-path "$ROOT_DIR/native/macos"
  -c release
  --scratch-path "$SWIFT_SCRATCH"
  --cache-path "$SWIFT_CACHE"
  --config-path "$SWIFT_CONFIG"
  --security-path "$SWIFT_SECURITY"
  --disable-automatic-resolution
  --skip-update
  --disable-prefetching
  --disable-sandbox
)
sandbox-exec -p "$OFFLINE_SANDBOX" env HOME="$SWIFT_HOME" \
  CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" \
  AIM_CORE_LIBRARY_DIR="$CORE_TARGET/release" \
  DYLD_LIBRARY_PATH="$CORE_TARGET/release:$ROOT_DIR/.build-deps/librime/1.16.1/lib" \
  swift test "${SWIFT_ARGS[@]}" \
  --filter HostSupportTests.testIncompatibleUserDBIsQuarantinedAndLastValidBackupRestored
sandbox-exec -p "$OFFLINE_SANDBOX" env HOME="$SWIFT_HOME" \
  CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" \
  AIM_CORE_LIBRARY_DIR="$CORE_TARGET/release" \
  DYLD_LIBRARY_PATH="$CORE_TARGET/release:$ROOT_DIR/.build-deps/librime/1.16.1/lib" \
  swift build "${SWIFT_ARGS[@]}" --product AgentIMESnapshotHarness
SWIFT_BIN_PATH="$(sandbox-exec -p "$OFFLINE_SANDBOX" env HOME="$SWIFT_HOME" \
  CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" \
  AIM_CORE_LIBRARY_DIR="$CORE_TARGET/release" swift build "${SWIFT_ARGS[@]}" --show-bin-path)"
SNAPSHOT_HARNESS="$SWIFT_BIN_PATH/AgentIMESnapshotHarness"
SWIFT_TEST_BUNDLE="$(find "$SWIFT_BIN_PATH" -maxdepth 1 -type d -name '*.xctest' -print -quit)"
[[ -n "$SWIFT_TEST_BUNDLE" ]]
SWIFT_TEST_EXECUTABLE="$(find "$SWIFT_TEST_BUNDLE/Contents/MacOS" -type f -perm -111 -print -quit)"
[[ -x "$SWIFT_TEST_EXECUTABLE" ]]
sandbox-exec -p "$OFFLINE_SANDBOX" env CARGO_HOME="$CARGO_HOME_DIR" CARGO_NET_OFFLINE=true \
  CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$IPC_TARGET" cargo build --locked --offline --release --manifest-path "$ROOT_DIR/native/ipc/Cargo.toml" \
  --bin agent-ime-ipc-spike
IPC="$IPC_TARGET/release/agent-ime-ipc-spike"
IPC_UNSIGNED_CHECKSUM="$(file_checksum "$IPC")"

# ADR-004 requires a real signed-deployment smoke, not an unsigned cargo test.
# Bind the temporary CLI to the exact leaf certificate already carried by the
# frozen input artifact. The certificate SHA-1 is used only as codesign's exact
# local keychain selector and is never written to Evidence.
RC_LEAF_CERTIFICATE="$(extract_leaf_certificate "$INPUT_BUNDLE" "$TEMP_ROOT/frozen-input-leaf-")"
RC_LEAF_IDENTITY_SHA1="$(/usr/bin/shasum -a 1 "$RC_LEAF_CERTIFICATE" | /usr/bin/awk '{ print $1 }')"
[[ "$RC_LEAF_IDENTITY_SHA1" =~ ^[0-9a-f]{40}$ ]]
/usr/bin/codesign --force --sign "$RC_LEAF_IDENTITY_SHA1" --timestamp=none "$IPC"
/usr/bin/codesign --verify --strict --verbose=2 "$IPC"
[[ "$(bundle_team_id "$IPC")" == "$INPUT_TEAM_ID" ]]
[[ "$(bundle_authority "$IPC")" == "$INPUT_AUTHORITY" ]]
IPC_LEAF_CERTIFICATE="$(extract_leaf_certificate "$IPC" "$TEMP_ROOT/signed-ipc-leaf-")"
cmp -s "$RC_LEAF_CERTIFICATE" "$IPC_LEAF_CERTIFICATE"
IPC_LEAF_CERTIFICATE_SHA256="$(file_checksum "$IPC_LEAF_CERTIFICATE")"
[[ "$IPC_LEAF_CERTIFICATE_SHA256" == "$(file_checksum "$RC_LEAF_CERTIFICATE")" ]]
IPC_TEAM_IDENTIFIER_SHA256="$(printf '%s' "$(bundle_team_id "$IPC")" | text_checksum)"
IPC_LEAF_AUTHORITY_SHA256="$(printf '%s' "$(bundle_authority "$IPC")" | text_checksum)"
[[ "$IPC_TEAM_IDENTIFIER_SHA256" == "$EXPECTED_TEAM_SHA256" ]]
[[ "$IPC_LEAF_AUTHORITY_SHA256" == "$EXPECTED_AUTHORITY_SHA256" ]]

require_file "$SNAPSHOT_CTL"
require_file "$SNAPSHOT_HARNESS"
require_file "$HOST"
require_file "$IPC"
require_file "$INSTALLER"
require_file "$INSTALLER_TEST"
[[ "$(file_checksum "$INSTALLER")" == "$EXPECTED_INSTALLER_CHECKSUM" ]]
[[ "$(file_checksum "$INSTALLER_TEST")" == "$EXPECTED_INSTALLER_TEST_CHECKSUM" ]]
SNAPSHOT_HARNESS_CHECKSUM="$(file_checksum "$SNAPSHOT_HARNESS")"
IPC_CHECKSUM="$(file_checksum "$IPC")"
SWIFT_TEST_CHECKSUM="$(file_checksum "$SWIFT_TEST_EXECUTABLE")"
printf 'support_git_commit=%s support_source_checksum=%s\n' "$SUPPORT_GIT_COMMIT" "$SUPPORT_SOURCE_CHECKSUM"
printf 'snapshot_harness_checksum=%s ipc_unsigned_checksum=%s ipc_signed_checksum=%s swift_test_executable_checksum=%s\n' \
  "$SNAPSHOT_HARNESS_CHECKSUM" "$IPC_UNSIGNED_CHECKSUM" "$IPC_CHECKSUM" "$SWIFT_TEST_CHECKSUM"
printf 'ipc_signature=apple-development ipc_leaf_certificate_sha256=%s ipc_team_identifier_sha256=%s ipc_leaf_authority_sha256=%s\n' \
  "$IPC_LEAF_CERTIFICATE_SHA256" "$IPC_TEAM_IDENTIFIER_SHA256" "$IPC_LEAF_AUTHORITY_SHA256"
printf 'installer_checksum=%s installer_test_checksum=%s\n' \
  "$EXPECTED_INSTALLER_CHECKSUM" "$EXPECTED_INSTALLER_TEST_CHECKSUM"
verify_support_tool_pins
INPUT_CHECKSUM="$($ROOT_DIR/script/artifact_checksum.sh "$INPUT_BUNDLE")"
MANAGER_CHECKSUM="$($ROOT_DIR/script/artifact_checksum.sh "$MANAGER_BUNDLE")"
[[ "$INPUT_CHECKSUM" == "$EXPECTED_INPUT_CHECKSUM" ]]
[[ "$MANAGER_CHECKSUM" == "$EXPECTED_MANAGER_CHECKSUM" ]]
[[ "$(bundle_version "$INPUT_BUNDLE")" == "$PRODUCT_VERSION" ]]
[[ "$(bundle_version "$MANAGER_BUNDLE")" == "$PRODUCT_VERSION" ]]
[[ "$(bundle_build "$INPUT_BUNDLE")" == "$EXPECTED_BUILD" ]]
[[ "$(bundle_build "$MANAGER_BUNDLE")" == "$EXPECTED_BUILD" ]]
[[ "$(bundle_team_id "$INPUT_BUNDLE")" == "$INPUT_TEAM_ID" ]]
[[ "$(bundle_team_id "$MANAGER_BUNDLE")" == "$INPUT_TEAM_ID" ]]
[[ -z "$(find "$INPUT_BUNDLE" "$MANAGER_BUNDLE" \
  \( -perm -0200 -o -perm -0020 -o -perm -0002 \) -print -quit)" ]]
while IFS= read -r -d '' frozen_entry; do
  [[ ! -w "$frozen_entry" ]]
done < <(find "$INPUT_BUNDLE" "$MANAGER_BUNDLE" -print0)
printf 'artifact_checksum=%s\nmanager_checksum=%s\nbuild=%s team_identifier_sha256=%s leaf_authority_sha256=%s\nstarted=%s\n' \
  "$INPUT_CHECKSUM" "$MANAGER_CHECKSUM" "$EXPECTED_BUILD" "$EXPECTED_TEAM_SHA256" \
  "$EXPECTED_AUTHORITY_SHA256" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"

printf '\n[snapshot]\n'
SNAPSHOT_ROOT="$TEMP_ROOT/snapshots"
"$SNAPSHOT_CTL" "$SNAPSHOT_ROOT" 40
"$SNAPSHOT_CTL" "$SNAPSHOT_ROOT" 41
"$SNAPSHOT_HARNESS" "$SNAPSHOT_ROOT" 41
ACTIVE_FILE="$(awk -F= '$1 == "file" { print $2 }' "$SNAPSHOT_ROOT/current")"
[[ -n "$ACTIVE_FILE" ]]
chmod u+w "$SNAPSHOT_ROOT/$ACTIVE_FILE"
printf 'corrupt\n' > "$SNAPSHOT_ROOT/$ACTIVE_FILE"
"$SNAPSHOT_HARNESS" "$SNAPSHOT_ROOT" 40
VALID_FALLBACK="$(find "$SNAPSHOT_ROOT" -maxdepth 1 -name 'snapshot-40-*.aimsnap' -print -quit)"
[[ -n "$VALID_FALLBACK" ]]
chmod 000 "$VALID_FALLBACK"
capture_status "$TEMP_ROOT/snapshot-permission.txt" "$SNAPSHOT_HARNESS" "$SNAPSHOT_ROOT" 40
chmod 600 "$VALID_FALLBACK"
[[ "$CAPTURED_STATUS" -eq 3 ]]
printf 'bad_current_fallback=40 permission_failure_exit=%s expected=3\n' "$CAPTURED_STATUS"

printf '\n[ipc]\n'
SOCKET="$TEMP_ROOT/ipc/control.sock"
"$IPC" serve-count "$SOCKET" 6 > "$TEMP_ROOT/ipc-server.txt" 2>&1 &
IPC_PID=$!
wait_for_path "$SOCKET" socket
[[ "$(stat -f '%Lp' "$(dirname "$SOCKET")")" == "700" ]]
[[ "$(stat -f '%Lp' "$SOCKET")" == "600" ]]
IPC_HELLO_RESPONSE="$($IPC request "$SOCKET" 'AIMIPC/1 hello signed-hello')"
IPC_HEALTH_RESPONSE="$($IPC request "$SOCKET" 'AIMIPC/1 health signed-health')"
IPC_REQUEST_RESPONSE="$($IPC request "$SOCKET" 'AIMIPC/1 request signed-request synthetic')"
IPC_CANCEL_RESPONSE="$($IPC request "$SOCKET" 'AIMIPC/1 cancel signed-request')"
IPC_REAL_PAYLOAD_RESPONSE="$(printf 'AIMIPC/1 request signed-user user-text\n' | /usr/bin/nc -U -w 1 "$SOCKET")"
IPC_VERSION_RESPONSE="$(printf 'AIMIPC/2 health signed-version\n' | /usr/bin/nc -U -w 1 "$SOCKET")"
wait "$IPC_PID"
IPC_PID=""
[[ "$IPC_HELLO_RESPONSE" == "ok signed-hello hello ime=0.3.0 backend=0.3.0 capabilities=health,synthetic_request,cancel" ]]
[[ "$IPC_HEALTH_RESPONSE" == "ok signed-health health status=ready snapshot=fixture-v1" ]]
[[ "$IPC_REQUEST_RESPONSE" == "ok signed-request accepted synthetic-only" ]]
[[ "$IPC_CANCEL_RESPONSE" == "ok signed-request cancel removed=1" ]]
[[ "$IPC_REAL_PAYLOAD_RESPONSE" == "error signed-user real-payload-disabled-in-v0.3" ]]
[[ "$IPC_VERSION_RESPONSE" == "error unknown protocol-version" ]]
printf 'signed_deployment_smoke=PASS parent_mode=700 socket_mode=600 same_uid=PASS hello=PASS health=PASS synthetic_request=PASS cancel_removed=1 real_payload_rejected=PASS version_mismatch_rejected=PASS\n'

TIMEOUT_SOCKET="$TEMP_ROOT/ipc-timeout/control.sock"
"$IPC" serve-once "$TIMEOUT_SOCKET" > "$TEMP_ROOT/ipc-timeout-server.txt" 2>&1 &
IPC_PID=$!
wait_for_path "$TIMEOUT_SOCKET" socket
kill -STOP "$IPC_PID"
if /usr/bin/time -p "$IPC" request "$TIMEOUT_SOCKET" 'AIMIPC/1 health signed-timeout' \
  > "$TEMP_ROOT/ipc-timeout.out" 2> "$TEMP_ROOT/ipc-timeout.err"; then
  TIMEOUT_STATUS=0
else
  TIMEOUT_STATUS=$?
fi
[[ "$TIMEOUT_STATUS" -eq 5 ]]
rg -q 'backend-unavailable' "$TEMP_ROOT/ipc-timeout.err"
TIMEOUT_SECONDS="$(awk '$1 == "real" { print $2 }' "$TEMP_ROOT/ipc-timeout.err")"
[[ -n "$TIMEOUT_SECONDS" ]]
awk -v seconds="$TIMEOUT_SECONDS" 'BEGIN { exit !(seconds < 0.75) }'
kill -CONT "$IPC_PID" 2>/dev/null || true
terminate_owned_child "$IPC_PID" TERM
wait "$IPC_PID" 2>/dev/null || true
IPC_PID=""
printf 'signed_stalled_backend_exit=5 configured_timeout_ms=250 process_elapsed_seconds=%s\n' "$TIMEOUT_SECONDS"

MISSING_SOCKET="$TEMP_ROOT/ipc/missing.sock"
if /usr/bin/time -p "$IPC" request "$MISSING_SOCKET" 'AIMIPC/1 health missing' \
  > "$TEMP_ROOT/ipc-missing.out" 2> "$TEMP_ROOT/ipc-missing.err"; then
  MISSING_STATUS=0
else
  MISSING_STATUS=$?
fi
[[ "$MISSING_STATUS" -eq 5 ]]
rg -q 'backend-unavailable' "$TEMP_ROOT/ipc-missing.err"
MISSING_SECONDS="$(awk '$1 == "real" { print $2 }' "$TEMP_ROOT/ipc-missing.err")"
[[ -n "$MISSING_SECONDS" ]]
awk -v seconds="$MISSING_SECONDS" 'BEGIN { exit !(seconds < 0.25) }'
sandbox-exec -p "$OFFLINE_SANDBOX" env CARGO_HOME="$CARGO_HOME_DIR" CARGO_NET_OFFLINE=true \
  CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$IPC_TARGET" cargo test --locked --offline --release --manifest-path "$ROOT_DIR/native/ipc/Cargo.toml"
verify_support_tool_pins
printf 'missing_backend_exit=5 process_elapsed_seconds=%s internal_gate_lt_ms=10\n' "$MISSING_SECONDS"
printf 'unsigned_cargo_test_supplement=PASS scope=permissions,payload-bounds,timeout,peer-uid,races not_deployment_smoke=true\n'

printf '\n[offline-and-manager-exited]\n'
OFFLINE_ROOT="$TEMP_ROOT/offline-home/Library/Application Support/AgentInputMethod"
OFFLINE_HOME="$TEMP_ROOT/offline-home"
mkdir -p "$OFFLINE_ROOT/Shared/Snapshots"
"$SNAPSHOT_CTL" "$OFFLINE_ROOT/Shared/Snapshots" 50 > "$TEMP_ROOT/offline-publish.txt"
/usr/bin/sandbox-exec -p '(version 1) (allow default) (deny network*)' \
  /usr/bin/env HOME="$OFFLINE_HOME" CFFIXED_USER_HOME="$OFFLINE_HOME" AIM_COLD_START_PROBE_DIR="$OFFLINE_ROOT" "$HOST"
printf 'manager_dependency_unavailable_by_isolation=true publisher_exited=true network_policy=deny-network frozen_host_probe=PASS\n'
sandbox-exec -p "$OFFLINE_SANDBOX" env HOME="$TEMP_ROOT/frontend-home" \
  node "$ROOT_DIR/scripts/run-frontend-tests.mjs" \
  --test-name-pattern='provider timeout falls back|SQLite lock error falls back'
if otool -L "$HOST" | rg -q 'sqlite|curl|CFNetwork'; then
  echo 'frozen input host unexpectedly links a background data/network dependency' >&2
  exit 4
fi

SQLITE_DB="$TEMP_ROOT/manager-lock.sqlite3"
SQLITE_FIFO="$TEMP_ROOT/manager-lock.commands"
SQLITE_LOG="$TEMP_ROOT/manager-lock-holder.txt"
/usr/bin/sqlite3 "$SQLITE_DB" \
  "CREATE TABLE fault_state (id INTEGER PRIMARY KEY, value TEXT NOT NULL); INSERT INTO fault_state VALUES (1, 'original');"
mkfifo "$SQLITE_FIFO"
/usr/bin/sqlite3 "$SQLITE_DB" < "$SQLITE_FIFO" > "$SQLITE_LOG" 2>&1 &
SQLITE_LOCK_PID=$!
exec 9> "$SQLITE_FIFO"
printf 'BEGIN IMMEDIATE;\nUPDATE fault_state SET value = %s WHERE id = 1;\n.print lock-ready\n' \
  "'uncommitted'" >&9
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if rg -q '^lock-ready$' "$SQLITE_LOG" 2>/dev/null; then break; fi
  sleep 0.1
done
rg -q '^lock-ready$' "$SQLITE_LOG"
capture_status "$TEMP_ROOT/sqlite-locked-write.txt" /usr/bin/sqlite3 \
  -cmd '.timeout 250' "$SQLITE_DB" "UPDATE fault_state SET value = 'replacement' WHERE id = 1;"
[[ "$CAPTURED_STATUS" -ne 0 ]]
rg -qi 'database is locked|database is busy' "$TEMP_ROOT/sqlite-locked-write.txt"
SQLITE_HOST_ROOT="$TEMP_ROOT/sqlite-lock-home/Library/Application Support/AgentInputMethod"
mkdir -p "$SQLITE_HOST_ROOT"
/usr/bin/sandbox-exec -p "$OFFLINE_SANDBOX" /usr/bin/env \
  HOME="$TEMP_ROOT/sqlite-lock-home" CFFIXED_USER_HOME="$TEMP_ROOT/sqlite-lock-home" \
  AIM_COLD_START_PROBE_DIR="$SQLITE_HOST_ROOT" "$HOST"
printf 'ROLLBACK;\n' >&9
exec 9>&-
wait "$SQLITE_LOCK_PID"
SQLITE_LOCK_PID=""
[[ "$(/usr/bin/sqlite3 "$SQLITE_DB" 'SELECT value FROM fault_state WHERE id = 1;')" == "original" ]]
/usr/bin/sqlite3 "$SQLITE_DB" "UPDATE fault_state SET value = 'replacement' WHERE id = 1;"
[[ "$(/usr/bin/sqlite3 "$SQLITE_DB" 'SELECT value FROM fault_state WHERE id = 1;')" == "replacement" ]]
printf 'provider_timeout_fallback=injected-explicit-timeout-PASS sqlite_lock=bounded-lock-and-recovery-PASS basic_input_under_sqlite_lock=PASS data_source_failure=bad-snapshot-fallback-PASS\n'

printf '\n[input-process-restart]\n'
RECOVERY_HOME="$TEMP_ROOT/recovery-home"
RECOVERY_MARKER="$RECOVERY_HOME/Library/Application Support/AgentInputMethod/Runtime/running.marker"
RECOVERY_LOG_DIR="$RECOVERY_HOME/Library/Logs/AgentInputMethod"
mkdir -p "$RECOVERY_HOME"
chmod 700 "$RECOVERY_HOME"
HOME="$RECOVERY_HOME" CFFIXED_USER_HOME="$RECOVERY_HOME" "$HOST" > "$TEMP_ROOT/recovery-first.txt" 2>&1 &
HOST_PID=$!
wait_for_path "$RECOVERY_MARKER" file
kill -9 "$HOST_PID"
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=""
HOME="$RECOVERY_HOME" CFFIXED_USER_HOME="$RECOVERY_HOME" "$HOST" > "$TEMP_ROOT/recovery-second.txt" 2>&1 &
HOST_PID=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if rg -q '"eventKind":"unclean-recovery"' "$RECOVERY_LOG_DIR" -g '*.jsonl' 2>/dev/null; then break; fi
  sleep 0.2
done
rg -q '"eventKind":"unclean-recovery"' "$RECOVERY_LOG_DIR" -g '*.jsonl'
kill -TERM "$HOST_PID" 2>/dev/null || true
sleep 0.2
if kill -0 "$HOST_PID" 2>/dev/null; then kill -9 "$HOST_PID" 2>/dev/null || true; fi
wait "$HOST_PID" 2>/dev/null || true
HOST_PID=""
printf 'sigkill_relaunch=true metadata_only_unclean_recovery=PASS isolated_home=true\n'

printf '\n[userdb-incompatible]\n'
verify_support_tool_pins
printf 'quarantine_restore=PASS temporary_fixture=true pre_pin_filtered_test=true swift_test_executable_checksum=%s\n' \
  "$SWIFT_TEST_CHECKSUM"

printf '\n[install-upgrade-rollback-uninstall]\n'
acquire_fault_lease
if [[ -e "$INSTALLED" && -e "$INSTALLED_LEGACY" ]]; then
  echo 'refusing real install drill while both current and legacy destinations exist' >&2
  exit 4
fi
if [[ -d "$INSTALLED" ]]; then
  ditto "$INSTALLED" "$TEMP_ROOT/original.inputmethod"
  HAD_ORIGINAL=1
  ORIGINAL_KIND="app"
  ORIGINAL_CHECKSUM="$(directory_checksum "$INSTALLED")"
elif [[ -d "$INSTALLED_LEGACY" ]]; then
  echo 'refusing real fault drill from legacy destination; migrate with installer first' >&2
  exit 4
fi
if [[ -L "$INSTALL_STATE" ]]; then
  echo 'refusing real fault drill with a symlinked InstallState' >&2
  exit 4
elif [[ -d "$INSTALL_STATE" ]]; then
  [[ ! -e "$INSTALL_STATE/install.lock" && ! -L "$INSTALL_STATE/install.lock" &&
     ! -e "$INSTALL_STATE/transaction.journal" && ! -L "$INSTALL_STATE/transaction.journal" &&
     ! -e "$INSTALL_STATE/recovery-required" && ! -L "$INSTALL_STATE/recovery-required" ]] || {
    echo 'refusing fault drill with an active or recovery-required installer transaction' >&2
    exit 4
  }
  ditto "$INSTALL_STATE" "$TEMP_ROOT/original-install-state"
  HAD_INSTALL_STATE=1
  ORIGINAL_STATE_CHECKSUM="$(directory_checksum "$INSTALL_STATE")"
fi
[[ "$HAD_ORIGINAL" -eq 1 ]] || { echo 'real install drill requires an existing installed frozen RC to restore' >&2; exit 4; }
[[ "$ORIGINAL_CHECKSUM" == "$INPUT_CHECKSUM" ]] || {
  echo 'installed input method is not the frozen RC; refusing to mutate real rollback state' >&2
  exit 4
}
[[ "$HAD_INSTALL_STATE" -eq 1 ]] || { echo 'real install drill requires an existing InstallState snapshot' >&2; exit 4; }
OTHER_BEFORE="$(other_input_methods_inventory "$TEMP_ROOT/other-before.manifest")"
capture_real_host_state

ISOLATED_FIXTURE="$TEMP_ROOT/isolated-previous.inputmethod"
ditto "$INPUT_BUNDLE" "$ISOLATED_FIXTURE"
chmod -R u+w "$ISOLATED_FIXTURE"
printf 'AIM-ISOLATED-INSTALL-CYCLE-PREVIOUS\n' > "$ISOLATED_FIXTURE/Contents/Resources/InstallCyclePrevious.txt"
codesign --force --deep --sign - --timestamp=none "$ISOLATED_FIXTURE"
codesign --verify --deep --strict "$ISOLATED_FIXTURE"
[[ "$(bundle_version "$ISOLATED_FIXTURE")" == "$PRODUCT_VERSION" ]]
[[ "$(bundle_build "$ISOLATED_FIXTURE")" == "$EXPECTED_BUILD" ]]
ISOLATED_FIXTURE_CHECKSUM="$(directory_checksum "$ISOLATED_FIXTURE")"
[[ "$ISOLATED_FIXTURE_CHECKSUM" != "$INPUT_CHECKSUM" ]]
chmod -R a-w "$ISOLATED_FIXTURE"
printf 'isolated_previous_fixture_checksum=%s signature=adhoc scope=/private/tmp-only\n' \
  "$ISOLATED_FIXTURE_CHECKSUM"

printf '\n[disk-full-and-upgrade-interruption]\n'
verify_support_tool_pins
"$INSTALLER_TEST" "$INPUT_BUNDLE"
DISK_IMAGE="$TEMP_ROOT/disk-full.dmg"
DISK_MOUNT="$TEMP_ROOT/disk-full-mount"
mkdir -p "$DISK_MOUNT"
hdiutil create -quiet -size 24m -fs HFS+ -volname AIMFault -ov "$DISK_IMAGE"
ATTACH_OUTPUT="$(hdiutil attach -nobrowse -mountpoint "$DISK_MOUNT" "$DISK_IMAGE")"
DISK_DEVICE="$(printf '%s\n' "$ATTACH_OUTPUT" | awk '/^\/dev\// { print $1; exit }')"
[[ -n "$DISK_DEVICE" ]]
DISK_REGISTER="$TEMP_ROOT/disk-register-ok.sh"
printf '#!/usr/bin/env bash\nexit 0\n' > "$DISK_REGISTER"
chmod 700 "$DISK_REGISTER"
DISK_INSTALL_DIR="$DISK_MOUNT/Input Methods"
DISK_STATE_DIR="$DISK_MOUNT/InstallState"
  AIM_INSTALL_TEST_MODE=1 AIM_INSTALL_DIR="$DISK_INSTALL_DIR" AIM_INSTALL_STATE_DIR="$DISK_STATE_DIR" \
  AIM_INSTALL_SKIP_SERVICE_REFRESH=1 AIM_INSTALL_SKIP_HOST_STOP=1 AIM_INSTALL_REGISTER_HELPER="$DISK_REGISTER" \
  "$INSTALLER" --install "$ISOLATED_FIXTURE" > "$TEMP_ROOT/disk-initial-install.txt"
[[ "$(directory_checksum "$DISK_INSTALL_DIR/Agent Input Method.app")" == "$ISOLATED_FIXTURE_CHECKSUM" ]]
FREE_K="$(df -k "$DISK_MOUNT" | awk 'NR == 2 { print $4 }')"
[[ "$FREE_K" -gt 1536 ]]
FILL_K=$((FREE_K - 1024))
mkfile "${FILL_K}k" "$DISK_MOUNT/fill.bin"
capture_status "$TEMP_ROOT/disk-full-upgrade.txt" env \
  AIM_INSTALL_TEST_MODE=1 AIM_INSTALL_DIR="$DISK_INSTALL_DIR" AIM_INSTALL_STATE_DIR="$DISK_STATE_DIR" \
  AIM_INSTALL_SKIP_SERVICE_REFRESH=1 AIM_INSTALL_SKIP_HOST_STOP=1 AIM_INSTALL_REGISTER_HELPER="$DISK_REGISTER" \
  "$INSTALLER" --install "$INPUT_BUNDLE"
[[ "$CAPTURED_STATUS" -ne 0 ]]
[[ "$(directory_checksum "$DISK_INSTALL_DIR/Agent Input Method.app")" == "$ISOLATED_FIXTURE_CHECKSUM" ]]
[[ ! -e "$DISK_STATE_DIR/transaction.journal" && ! -e "$DISK_STATE_DIR/recovery-required" ]]
[[ -z "$(find "$DISK_INSTALL_DIR" "$DISK_STATE_DIR" -maxdepth 1 \
  \( -name '.agent-ime-*' -o -name '.checksum.*' -o -name '.transaction.journal.*' \
     -o -name '.stale-install-lock.*' -o -name 'install.lock' \) -print -quit 2>/dev/null || true)" ]]
hdiutil detach "$DISK_DEVICE" >/dev/null
DISK_DEVICE=""
printf 'upgrade_sigkill_matrix=PASS disk_full_upgrade_exit=%s previous_bundle_preserved=true\n' "$CAPTURED_STATUS"

INSTALL_CYCLE_STARTED=1
run_install_step reinstall-frozen-rc "$INPUT_CHECKSUM" --install "$INPUT_BUNDLE" "$RC_POINTER"
run_install_step rollback-same-frozen-rc "$INPUT_CHECKSUM" --rollback

capture_status "$TEMP_ROOT/install-uninstall.txt" env AIM_INSTALL_FAULT_LEASE_TOKEN="$FAULT_LEASE_TOKEN" \
  "$INSTALLER" --uninstall
[[ "$CAPTURED_STATUS" -eq 0 ]]
[[ ! -e "$INSTALLED" && ! -e "$INSTALLED_LEGACY" ]]
capture_status "$TEMP_ROOT/tis-uninstalled-status.txt" \
  "$INPUT_BUNDLE/Contents/MacOS/AgentIMEInputSourceCtl" status
[[ "$CAPTURED_STATUS" -eq 3 ]] || {
  sed "s#$HOME#~#g" "$TEMP_ROOT/tis-uninstalled-status.txt" >&2
  echo "uninstall TIS absence check failed: expected=3 actual=$CAPTURED_STATUS" >&2
  exit 83
}
printf 'install_cycle_step=uninstall exit=0 destination_absent=true tis_absence_exit=3\n'

run_install_step restore-after-uninstall "$INPUT_CHECKSUM" --rollback
run_install_step final-install "$INPUT_CHECKSUM" --install "$INPUT_BUNDLE" "$RC_POINTER"
[[ "$(bundle_version "$INSTALLED")" == "$PRODUCT_VERSION" ]]
[[ "$($ROOT_DIR/script/artifact_checksum.sh "$INSTALLED")" == "$INPUT_CHECKSUM" ]]
codesign --verify --deep --strict "$INSTALLED"
OTHER_AFTER="$(other_input_methods_inventory "$TEMP_ROOT/other-after.manifest")"
[[ "$OTHER_BEFORE" == "$OTHER_AFTER" ]]
printf 'other_input_methods_inventory=%s unchanged=true final_checksum=%s\n' "$OTHER_AFTER" "$INPUT_CHECKSUM"

capture_status "$TEMP_ROOT/tis-status.txt" "$INSTALLED/Contents/MacOS/AgentIMEInputSourceCtl" status
if [[ "$CAPTURED_STATUS" -eq 0 ]]; then
  rg -qx 'com\.agentinputlayer\.inputmethod\.agent\.pinyin enabled=true unique=true parent=true' "$TEMP_ROOT/tis-status.txt"
  printf 'current_login_tis_status=visible-enabled-unique parent_registration=true exit=0 source_id=com.hongye.facai.inputmethod.zhizi.pinyin\n'
else
  sed "s#$HOME#~#g" "$TEMP_ROOT/tis-status.txt" >&2
  echo "current login TIS gate is pending or failed: exit=$CAPTURED_STATUS" >&2
  exit 84
fi

restore_state_snapshot
[[ "$(directory_checksum "$INSTALL_STATE")" == "$ORIGINAL_STATE_CHECKSUM" ]]
[[ "$(directory_checksum "$INSTALLED")" == "$INPUT_CHECKSUM" ]]
restore_real_host_state
verify_support_tool_pins
printf 'original_install_state_checksum=%s restored_exactly=true support_tool_pins=PASS\n' \
  "$ORIGINAL_STATE_CHECKSUM"
[[ "$($ROOT_DIR/script/artifact_checksum.sh "$INPUT_BUNDLE")" == "$EXPECTED_INPUT_CHECKSUM" ]]
[[ "$($ROOT_DIR/script/artifact_checksum.sh "$MANAGER_BUNDLE")" == "$EXPECTED_MANAGER_CHECKSUM" ]]
printf 'end_artifact_checksum=%s end_manager_checksum=%s pointer_checksum=%s immutable=true\n' \
  "$EXPECTED_INPUT_CHECKSUM" "$EXPECTED_MANAGER_CHECKSUM" "$EXPECTED_POINTER_CHECKSUM"
printf '\ncompleted=%s\nautomated_fault_subset=PASS\njoint_fault_gate=PASS_AUTOMATED_SUBSET\npost_fault_joint_pointer_refreeze_required=false\n' \
  "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
INSTALL_CYCLE_STARTED=0
FAULT_COMPLETED=1
