#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ACTION="${1:---install}"
REQUESTED_ACTION="$ACTION"
SOURCE_BUNDLE="${2:-}"
RC_POINTER="${3:-$ROOT_DIR/docs/current/evidence/v0.2-v0.3/RC_ARTIFACT.sha256}"
OFFICIAL_VALIDATOR="$ROOT_DIR/script/validate_official_input_bundle.sh"
INSTALL_DIR="${AIM_INSTALL_DIR:-$HOME/Library/Input Methods}"
DESTINATION="$INSTALL_DIR/Agent Input Method.app"
LEGACY_DESTINATION="$INSTALL_DIR/Agent Input Method.inputmethod"
STATE_DIR="${AIM_INSTALL_STATE_DIR:-$HOME/Library/Application Support/AgentInputMethod/InstallState}"
STATE_PARENT="$(dirname "$STATE_DIR")"
BACKUP="$STATE_DIR/previous.inputmethod"
FAILED_CURRENT="$STATE_DIR/failed-current.inputmethod"
PREVIOUS_CHECKSUM="$STATE_DIR/previous.bundle.sha256"
CURRENT_CHECKSUM="$STATE_DIR/current.bundle.sha256"
LEGACY_PREVIOUS_CHECKSUM="$STATE_DIR/previous.sha256"
LEGACY_CURRENT_CHECKSUM="$STATE_DIR/current.sha256"
LOCK_DIR="$STATE_DIR/install.lock"
RECOVERY_REQUIRED="$STATE_DIR/recovery-required"
TRANSACTION_JOURNAL="$STATE_DIR/transaction.journal"
FAULT_LEASE_DIR="$STATE_PARENT/.install-fault-drill.lock"
FAULT_LEASE_TOKEN="${AIM_INSTALL_FAULT_LEASE_TOKEN:-}"
LSREGISTER="${AIM_LSREGISTER:-/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister}"
RUNTIME_DIR="${AIM_INSTALL_RUNTIME_DIR:-$HOME/Library/Application Support/AgentInputMethod/Runtime}"
TEST_MODE="${AIM_INSTALL_TEST_MODE:-0}"
INPUT_SOURCE_BUNDLE_ID="com.hongye.facai.inputmethod.zhizi"
PROCESS_HELPER="${AIM_INSTALL_PROCESS_HELPER:-}"
RUNTIME_LOCK_HELPER_OVERRIDE="${AIM_INSTALL_RUNTIME_LOCK_HELPER:-}"
OFFICIAL_SOURCE_CHECKSUM=""

LOCK_HELD=0
RUNTIME_LOCK_HELD=0
RUNTIME_LOCK_HELPER=""
RUNTIME_LOCK_SESSION=""
RUNTIME_LOCK_PID=""
RUNTIME_LOCK_MONITOR_PID=""
TRANSACTION_PHASE="none"
TRANSACTION_ORIGINAL=""
TRANSACTION_OLD=""
TRANSACTION_ROOT=""
TRANSACTION_STAGE_ROOT=""
TRANSACTION_BACKUP_STAGE=""
TRANSACTION_BACKUP_STAGE_ROOT=""
STAGED_ROOT=""
STAGED_BUNDLE=""
STAGE_ROOT_A=""
STAGE_ROOT_B=""
TRANSACTION_ACTION="$ACTION"
RECOVERY_PRESERVE=0
BACKUP_COMMIT_ROOT=""
BACKUP_CHECKSUM_NEW=""
BACKUP_NEW_CHECKSUM=""
BACKUP_OLD_CHECKSUM=""
TRANSACTION_ORIGINAL_CHECKSUM=""
TRANSACTION_NEW_CHECKSUM=""

die() { echo "$1" >&2; exit "${2:-2}"; }

bundle_checksum() { "$ROOT_DIR/script/artifact_checksum.sh" "$1"; }

validate_cli() {
  case "$ACTION" in
    --install)
      [[ $# -le 3 ]] || die "usage: $0 --install [frozen-bundle [frozen-rc-pointer]]"
      ;;
    --validate-official)
      [[ $# -ge 2 && $# -le 3 ]] || die "usage: $0 --validate-official <frozen-bundle> [frozen-rc-pointer]"
      ;;
    --rollback|--uninstall|--recover)
      [[ $# -eq 1 ]] || die "usage: $0 $ACTION"
      ;;
    *)
      die "usage: $0 [--install [frozen-bundle [frozen-rc-pointer]]|--validate-official <bundle> [pointer]|--rollback|--uninstall|--recover]"
      ;;
  esac
}

resolve_and_validate_official_source() {
  [[ -x "$OFFICIAL_VALIDATOR" ]] || die "missing official input validator: $OFFICIAL_VALIDATOR"
  [[ -f "$RC_POINTER" && ! -L "$RC_POINTER" ]] || die "missing regular frozen RC pointer: $RC_POINTER" 3
  if [[ -z "$SOURCE_BUNDLE" ]]; then
    SOURCE_BUNDLE="$(awk '/^[0-9a-f]{64}  / { print substr($0, 67); exit }' "$RC_POINTER")"
  fi
  [[ "$SOURCE_BUNDLE" == /* ]] || die "official install source must be an absolute frozen bundle path" 3
  "$OFFICIAL_VALIDATOR" "$SOURCE_BUNDLE" "$RC_POINTER"
  SOURCE_BUNDLE="$(cd "$SOURCE_BUNDLE" && pwd -P)"
  OFFICIAL_SOURCE_CHECKSUM="$(awk '/^[0-9a-f]{64}  / { print $1; exit }' "$RC_POINTER")"
  [[ "$OFFICIAL_SOURCE_CHECKSUM" =~ ^[0-9a-f]{64}$ ]] || die "frozen RC pointer checksum is invalid" 3
  [[ "$(bundle_checksum "$SOURCE_BUNDLE")" == "$OFFICIAL_SOURCE_CHECKSUM" ]] \
    || die "frozen RC artifact changed after validation" 3
}

remove_tree() {
  local path failed=0
  for path in "$@"; do
    [[ -n "$path" && ( -e "$path" || -L "$path" ) ]] || continue
    if [[ -L "$path" ]]; then
      rm -f "$path" || failed=1
    else
      if [[ -d "$path" ]]; then
        find "$path" -type d -exec chmod u+rwx {} + 2>/dev/null || true
      fi
      rm -rf "$path" || failed=1
    fi
  done
  return "$failed"
}

move_bundle() {
  local source="$1" destination="$2" mode
  [[ -e "$source" && ! -L "$source" ]] || { echo "refusing to move missing or symlink bundle: $source" >&2; return 2; }
  [[ ! -e "$destination" && ! -L "$destination" ]] || { echo "refusing occupied or symlink bundle destination: $destination" >&2; return 2; }
  mode="$(stat -f '%Lp' "$source")"
  chmod u+w "$source" || return 1
  if mv "$source" "$destination"; then
    chmod "$mode" "$destination" || return 1
    return 0
  fi
  chmod "$mode" "$source" 2>/dev/null || true
  return 1
}

write_checksum() {
  local checksum="$1" label="$2" destination="$3" temporary
  temporary="$(mktemp "$STATE_DIR/.checksum.XXXXXX")" || return 1
  if ! printf '%s  %s\n' "$checksum" "$label" > "$temporary" ||
     ! chmod 600 "$temporary" || ! mv -f "$temporary" "$destination"; then
    rm -f "$temporary"
    return 1
  fi
}

validate_test_overrides() {
  local overrides=0
  if [[ -n "${AIM_INSTALL_DIR+x}" || -n "${AIM_INSTALL_STATE_DIR+x}" ||
        -n "${AIM_INSTALL_SKIP_SERVICE_REFRESH+x}" || -n "${AIM_INSTALL_SKIP_HOST_STOP+x}" ||
        -n "${AIM_INSTALL_REGISTER_HELPER+x}" || -n "${AIM_LSREGISTER+x}" ||
        -n "${AIM_INSTALL_PROCESS_HELPER+x}" || -n "${AIM_INSTALL_FAIL_AT+x}" ||
        -n "${AIM_INSTALL_CRASH_AT+x}" || -n "${AIM_INSTALL_RUNTIME_DIR+x}" ||
        -n "${AIM_INSTALL_ENABLE_CTL_HELPER+x}" || -n "${AIM_INSTALL_ENABLED_LIST_HELPER+x}" ||
        -n "${AIM_INSTALL_RUNTIME_LOCK_HELPER+x}" ]]; then overrides=1; fi
  if [[ "$overrides" -eq 1 && "$TEST_MODE" != "1" ]]; then
    die "installer overrides require AIM_INSTALL_TEST_MODE=1"
  fi
  if [[ "$TEST_MODE" == "1" ]]; then
    local install_parent state_parent helper_parent install_resolved state_resolved helper_resolved
    local process_helper_parent process_helper_resolved runtime_parent runtime_resolved
    local runtime_helper_parent runtime_helper_resolved
    install_parent="$(cd "$(dirname "$INSTALL_DIR")" && pwd -P)"
    state_parent="$(cd "$(dirname "$STATE_DIR")" && pwd -P)"
    install_resolved="$install_parent/$(basename "$INSTALL_DIR")"
    state_resolved="$state_parent/$(basename "$STATE_DIR")"
    case "$install_resolved" in /private/tmp/*) ;; *) die "test install directory must resolve under /private/tmp" ;; esac
    case "$state_resolved" in /private/tmp/*) ;; *) die "test state directory must resolve under /private/tmp" ;; esac
    runtime_parent="$(cd "$(dirname "$RUNTIME_DIR")" && pwd -P)"
    runtime_resolved="$runtime_parent/$(basename "$RUNTIME_DIR")"
    case "$runtime_resolved" in /private/tmp/*) ;; *) die "test runtime directory must resolve under /private/tmp" ;; esac
    [[ "${AIM_INSTALL_SKIP_SERVICE_REFRESH:-}" == "1" ]] || die "test mode requires AIM_INSTALL_SKIP_SERVICE_REFRESH=1"
    if [[ "${AIM_INSTALL_SKIP_HOST_STOP:-}" != "1" ]]; then
      [[ -n "$PROCESS_HELPER" && -f "$PROCESS_HELPER" && ! -L "$PROCESS_HELPER" && -x "$PROCESS_HELPER" ]] \
        || die "test host-stop execution requires a regular executable process helper"
      process_helper_parent="$(cd "$(dirname "$PROCESS_HELPER")" && pwd -P)"
      process_helper_resolved="$process_helper_parent/$(basename "$PROCESS_HELPER")"
      case "$process_helper_resolved" in /private/tmp/*) ;; *) die "test process helper must resolve under /private/tmp" ;; esac
    fi
    [[ -n "${AIM_INSTALL_REGISTER_HELPER:-}" && -f "$AIM_INSTALL_REGISTER_HELPER" && \
       ! -L "$AIM_INSTALL_REGISTER_HELPER" && -x "$AIM_INSTALL_REGISTER_HELPER" ]] \
      || die "test mode requires a regular executable registration helper"
    helper_parent="$(cd "$(dirname "$AIM_INSTALL_REGISTER_HELPER")" && pwd -P)"
    helper_resolved="$helper_parent/$(basename "$AIM_INSTALL_REGISTER_HELPER")"
    case "$helper_resolved" in /private/tmp/*) ;; *) die "test registration helper must resolve under /private/tmp" ;; esac
    [[ -n "$RUNTIME_LOCK_HELPER_OVERRIDE" && -f "$RUNTIME_LOCK_HELPER_OVERRIDE" && \
       ! -L "$RUNTIME_LOCK_HELPER_OVERRIDE" && -x "$RUNTIME_LOCK_HELPER_OVERRIDE" ]] \
      || die "test mode requires a regular executable runtime lock helper"
    runtime_helper_parent="$(cd "$(dirname "$RUNTIME_LOCK_HELPER_OVERRIDE")" && pwd -P)"
    runtime_helper_resolved="$runtime_helper_parent/$(basename "$RUNTIME_LOCK_HELPER_OVERRIDE")"
    case "$runtime_helper_resolved" in /private/tmp/*) ;; *) die "test runtime lock helper must resolve under /private/tmp" ;; esac
    local optional_helper optional_parent optional_resolved
    for optional_helper in "${AIM_INSTALL_ENABLE_CTL_HELPER:-}" "${AIM_INSTALL_ENABLED_LIST_HELPER:-}"; do
      [[ -n "$optional_helper" ]] || continue
      [[ -f "$optional_helper" && ! -L "$optional_helper" && -x "$optional_helper" ]] \
        || die "test enable helpers must be regular executables"
      optional_parent="$(cd "$(dirname "$optional_helper")" && pwd -P)"
      optional_resolved="$optional_parent/$(basename "$optional_helper")"
      case "$optional_resolved" in /private/tmp/*) ;; *) die "test enable helpers must resolve under /private/tmp" ;; esac
    done
    echo "installer_test_mode=true"
  fi
}

reject_symlink_ancestors() {
  local path="$1" current
  [[ "$path" == /* ]] || return 1
  current="$path"
  if [[ "$TEST_MODE" == "1" && "$current" == /tmp/* ]]; then
    current="/private$current"
  fi
  while [[ "$current" != "/" ]]; do
    [[ ! -L "$current" ]] || return 1
    current="$(dirname "$current")"
  done
}

prepare_directories() {
  reject_symlink_ancestors "$INSTALL_DIR" || die "refusing symlink in install directory path"
  reject_symlink_ancestors "$STATE_DIR" || die "refusing symlink in state directory path"
  reject_symlink_ancestors "$RUNTIME_DIR" || die "refusing symlink in runtime directory path"
  mkdir -p "$INSTALL_DIR" "$STATE_DIR"
  [[ -d "$INSTALL_DIR" && -d "$STATE_DIR" ]] || die "installer directories unavailable"
  chmod 700 "$STATE_DIR"
}

validate_state_artifacts() {
  local path
  for path in "$BACKUP" "$FAILED_CURRENT" "$PREVIOUS_CHECKSUM" "$CURRENT_CHECKSUM" \
    "$RECOVERY_REQUIRED" "$TRANSACTION_JOURNAL" "$LEGACY_PREVIOUS_CHECKSUM" \
    "$LEGACY_CURRENT_CHECKSUM"; do
    [[ ! -L "$path" ]] || die "refusing symlink installer state artifact: $path"
    if [[ -f "$path" ]]; then
      [[ "$(stat -f '%l' "$path")" -eq 1 ]] \
        || die "refusing hard-linked installer state artifact: $path"
    fi
  done
}

validate_fault_lease() {
  [[ ! -L "$FAULT_LEASE_DIR" ]] || die "refusing symlink fault-drill lease" 6
  if [[ -e "$FAULT_LEASE_DIR" ]]; then
    [[ -d "$FAULT_LEASE_DIR" ]] || die "invalid fault-drill lease" 6
    [[ -f "$FAULT_LEASE_DIR/token" && ! -L "$FAULT_LEASE_DIR/token" ]] \
      || die "fault-drill lease is missing its regular token" 6
    [[ "$(stat -f '%l' "$FAULT_LEASE_DIR/token")" -eq 1 ]] \
      || die "fault-drill lease token must not be hard-linked" 6
    [[ -f "$FAULT_LEASE_DIR/pid" && ! -L "$FAULT_LEASE_DIR/pid" && \
       "$(stat -f '%l' "$FAULT_LEASE_DIR/pid")" -eq 1 ]] \
      || die "fault-drill lease is missing its unique regular pid" 6
    [[ -n "$FAULT_LEASE_TOKEN" ]] || die "a fault drill owns the installer state" 6
    local lease_token
    lease_token="$(awk 'NR == 1 { print; exit }' "$FAULT_LEASE_DIR/token")"
    [[ -n "$lease_token" && "$lease_token" == "$FAULT_LEASE_TOKEN" ]] \
      || die "fault-drill lease token mismatch" 6
  elif [[ -n "$FAULT_LEASE_TOKEN" ]]; then
    die "fault-drill lease token supplied without an active lease" 6
  fi
}

acquire_lock() {
  validate_fault_lease
  [[ ! -L "$LOCK_DIR" ]] || die "refusing symlink installer lock"
  if [[ -e "$RECOVERY_REQUIRED" && "$ACTION" != "--recover" ]]; then
    die "installer recovery is required before another transaction" 70
  fi
  if mkdir "$LOCK_DIR" 2>/dev/null; then
    printf '%s\n' "$$" > "$LOCK_DIR/pid"
    chmod 600 "$LOCK_DIR/pid"
    LOCK_HELD=1
    return 0
  fi
  local owner=""
  if [[ -e "$LOCK_DIR/pid" || -L "$LOCK_DIR/pid" ]]; then
    [[ -f "$LOCK_DIR/pid" && ! -L "$LOCK_DIR/pid" && "$(stat -f '%l' "$LOCK_DIR/pid")" -eq 1 ]] \
      || die "installer lock pid must be a unique regular file" 6
    owner="$(awk 'NR == 1 { print $1 }' "$LOCK_DIR/pid")"
  fi
  if [[ "$owner" =~ ^[0-9]+$ ]] && kill -0 "$owner" 2>/dev/null; then
    die "another Agent Input Method install transaction is active" 6
  fi
  local stale_lock
  stale_lock="$(mktemp -d "$STATE_DIR/.stale-install-lock.XXXXXX")"
  rmdir "$stale_lock"
  if ! mv "$LOCK_DIR" "$stale_lock" 2>/dev/null; then
    die "installer lock changed during stale-lock takeover" 6
  fi
  if ! mkdir "$LOCK_DIR" 2>/dev/null; then
    remove_tree "$stale_lock"
    die "another Agent Input Method install transaction won the lock" 6
  fi
  remove_tree "$stale_lock"
  printf '%s\n' "$$" > "$LOCK_DIR/pid"
  chmod 600 "$LOCK_DIR/pid"
  LOCK_HELD=1
}

remove_stale_runtime_lock_sessions() {
  local path base
  for path in "$STATE_DIR"/.runtime-lock-session.*; do
    [[ -e "$path" || -L "$path" ]] || continue
    base="$(basename "$path")"
    [[ "$base" =~ ^\.runtime-lock-session\.[A-Za-z0-9]+$ && -d "$path" && ! -L "$path" ]] \
      || die "invalid stale runtime lock session artifact" 70
    remove_tree "$path" || die "unable to remove stale runtime lock session" 70
  done
}

runtime_lock_helper_from_bundle() {
  local bundle="$1" helper
  [[ -n "$bundle" && -d "$bundle" && ! -L "$bundle" ]] || return 1
  helper="$bundle/Contents/MacOS/AgentIMEInputSourceCtl"
  [[ -f "$helper" && ! -L "$helper" && -x "$helper" ]] || return 1
  validate_bundle "$bundle" >/dev/null 2>&1 || return 1
  printf '%s\n' "$helper"
}

select_runtime_lock_helper() {
  local candidate helper
  local -a candidates
  if [[ -n "$RUNTIME_LOCK_HELPER_OVERRIDE" ]]; then
    RUNTIME_LOCK_HELPER="$RUNTIME_LOCK_HELPER_OVERRIDE"
    return 0
  fi
  candidates=("$SOURCE_BUNDLE" "$DESTINATION" "$LEGACY_DESTINATION" "$BACKUP")
  [[ -z "$TRANSACTION_OLD" ]] || candidates+=("$TRANSACTION_OLD")
  if [[ -n "$TRANSACTION_ROOT" ]]; then
    candidates+=("$TRANSACTION_ROOT/recovery-displaced.inputmethod")
    candidates+=("$TRANSACTION_ROOT/recovery-copy.inputmethod")
  fi
  [[ -z "$TRANSACTION_STAGE_ROOT" ]] || candidates+=("$TRANSACTION_STAGE_ROOT/Agent Input Method.app")
  [[ -z "$TRANSACTION_BACKUP_STAGE" ]] || candidates+=("$TRANSACTION_BACKUP_STAGE")
  for candidate in "${candidates[@]}"; do
    if helper="$(runtime_lock_helper_from_bundle "$candidate")"; then
      RUNTIME_LOCK_HELPER="$helper"
      return 0
    fi
  done
  if [[ ! -e "$TRANSACTION_JOURNAL" && ! -e "$DESTINATION" && ! -e "$LEGACY_DESTINATION" &&
        ! -e "$BACKUP" && ( "$ACTION" == "--recover" || "$ACTION" == "--uninstall" ) ]]; then
    return 1
  fi
  return 2
}

acquire_runtime_lock() {
  local selection_status control ready_log error_log owner_pid
  if select_runtime_lock_helper; then
    :
  else
    selection_status=$?
    [[ "$selection_status" -eq 1 ]] && return 0
    die "no validated runtime lock helper is available; refusing bundle transaction" 70
  fi
  remove_stale_runtime_lock_sessions
  RUNTIME_LOCK_SESSION="$(mktemp -d "$STATE_DIR/.runtime-lock-session.XXXXXX")"
  chmod 700 "$RUNTIME_LOCK_SESSION"
  control="$RUNTIME_LOCK_SESSION/control"
  ready_log="$RUNTIME_LOCK_SESSION/ready.log"
  error_log="$RUNTIME_LOCK_SESSION/error.log"
  mkfifo "$control"
  chmod 600 "$control"
  : > "$ready_log"
  : > "$error_log"
  chmod 600 "$ready_log" "$error_log"
  owner_pid=$$
  (
    trap 'exit 0' TERM INT HUP
    while kill -0 "$owner_pid" 2>/dev/null; do sleep 0.1; done
  ) > "$control" &
  RUNTIME_LOCK_MONITOR_PID=$!
  "$RUNTIME_LOCK_HELPER" runtime-lock "$RUNTIME_DIR" \
    < "$control" > "$ready_log" 2> "$error_log" &
  RUNTIME_LOCK_PID=$!
  while ! grep -qx 'runtime_lock=acquired' "$ready_log" 2>/dev/null; do
    if ! kill -0 "$RUNTIME_LOCK_PID" 2>/dev/null; then
      kill "$RUNTIME_LOCK_MONITOR_PID" 2>/dev/null || true
      wait "$RUNTIME_LOCK_MONITOR_PID" 2>/dev/null || true
      wait "$RUNTIME_LOCK_PID" 2>/dev/null || true
      [[ ! -s "$error_log" ]] || cat "$error_log" >&2
      die "unable to acquire runtime recovery lock" 70
    fi
    sleep 0.05
  done
  RUNTIME_LOCK_HELD=1
}

release_runtime_lock() {
  local status=0
  [[ "$RUNTIME_LOCK_HELD" -eq 1 ]] || return 0
  kill "$RUNTIME_LOCK_MONITOR_PID" 2>/dev/null || status=1
  wait "$RUNTIME_LOCK_MONITOR_PID" 2>/dev/null || true
  wait "$RUNTIME_LOCK_PID" || status=$?
  RUNTIME_LOCK_HELD=0
  RUNTIME_LOCK_PID=""
  RUNTIME_LOCK_MONITOR_PID=""
  remove_tree "$RUNTIME_LOCK_SESSION" || status=1
  RUNTIME_LOCK_SESSION=""
  return "$status"
}

refresh_services() {
  local bundle="${1:-$DESTINATION}"
  if [[ "${AIM_INSTALL_SKIP_SERVICE_REFRESH:-0}" == "1" ]]; then return 0; fi
  if [[ -n "$bundle" && -e "$bundle" ]]; then "$LSREGISTER" -f "$bundle" 2>/dev/null || true; fi
  killall TextInputMenuAgent 2>/dev/null || true
}

host_process_list() {
  if [[ -n "$PROCESS_HELPER" ]]; then
    "$PROCESS_HELPER" list
  else
    pgrep -U "$(id -u)" -x AgentInputMethod
  fi
}

host_executable_path() {
  local pid="$1" output path
  if [[ -n "$PROCESS_HELPER" ]]; then
    "$PROCESS_HELPER" executable "$pid"
    return
  fi
  [[ -x /usr/sbin/lsof ]] || return 2
  output="$(/usr/sbin/lsof -a -p "$pid" -d txt -Fn 2>/dev/null)" || return 2
  path="$(printf '%s\n' "$output" | awk 'substr($0, 1, 1) == "n" { print substr($0, 2); exit }')"
  [[ -n "$path" ]] || return 2
  printf '%s\n' "$path"
}

host_process_alive() {
  local pid="$1"
  if [[ -n "$PROCESS_HELPER" ]]; then
    "$PROCESS_HELPER" alive "$pid"
  else
    kill -0 "$pid" 2>/dev/null
  fi
}

canonical_existing_file() {
  local path="$1" parent
  [[ "$path" == /* && -f "$path" && ! -L "$path" ]] || return 1
  parent="$(cd "$(dirname "$path")" && pwd -P)" || return 1
  printf '%s/%s\n' "$parent" "$(basename "$path")"
}

pid_matches_installed_host() {
  local pid="$1" actual status bundle expected
  local -a bundles
  if actual="$(host_executable_path "$pid")"; then
    :
  else
    host_process_alive "$pid" || return 1
    return 2
  fi
  actual="$(canonical_existing_file "$actual")" || {
    host_process_alive "$pid" || return 1
    return 2
  }
  bundles=("$DESTINATION" "$LEGACY_DESTINATION" "$BACKUP" "$FAILED_CURRENT")
  [[ -z "$TRANSACTION_OLD" ]] || bundles+=("$TRANSACTION_OLD")
  if [[ -n "$TRANSACTION_ROOT" ]]; then
    bundles+=("$TRANSACTION_ROOT/recovery-displaced.inputmethod")
    bundles+=("$TRANSACTION_ROOT/recovery-copy.inputmethod")
  fi
  [[ -z "$TRANSACTION_STAGE_ROOT" ]] || bundles+=("$TRANSACTION_STAGE_ROOT/Agent Input Method.app")
  [[ -z "$TRANSACTION_BACKUP_STAGE" ]] || bundles+=("$TRANSACTION_BACKUP_STAGE")
  for bundle in "${bundles[@]}"; do
    expected="$(canonical_existing_file "$bundle/Contents/MacOS/AgentInputMethod" 2>/dev/null)" || continue
    [[ "$actual" == "$expected" ]] && return 0
  done
  return 1
}

collect_installed_host_pids() {
  local pids status pid found=0
  if pids="$(host_process_list 2>/dev/null)"; then
    :
  else
    status=$?
    [[ "$status" -eq 1 ]] && return 1
    return 2
  fi
  while IFS= read -r pid; do
    [[ -n "$pid" ]] || continue
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 2
    if pid_matches_installed_host "$pid"; then
      printf '%s\n' "$pid"
      found=1
    else
      status=$?
      [[ "$status" -eq 1 ]] || return 2
    fi
  done <<< "$pids"
  [[ "$found" -eq 1 ]]
}

signal_installed_host() {
  local signal="$1" pid="$2" status
  if pid_matches_installed_host "$pid"; then
    :
  else
    status=$?
    if [[ "$status" -eq 1 ]] && ! host_process_alive "$pid"; then return 0; fi
    return 2
  fi
  if [[ -n "$PROCESS_HELPER" ]]; then
    "$PROCESS_HELPER" signal "$signal" "$pid"
  else
    kill "-$signal" "$pid"
  fi
}

stop_running_host() {
  if [[ "${AIM_INSTALL_SKIP_HOST_STOP:-0}" == "1" ]]; then return 0; fi
  local query_status=0 pids pid stopped=0
  if pids="$(collect_installed_host_pids)"; then
    while IFS= read -r pid; do signal_installed_host TERM "$pid" || return 2; done <<< "$pids"
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
      if pids="$(collect_installed_host_pids)"; then
        sleep 0.1
      else
        query_status=$?
        [[ "$query_status" -eq 1 ]] || return 2
        stopped=1
        break
      fi
    done
    if [[ "$stopped" -eq 0 ]]; then
      if pids="$(collect_installed_host_pids)"; then
        while IFS= read -r pid; do signal_installed_host KILL "$pid" || return 2; done <<< "$pids"
        for _ in 1 2 3 4 5 6 7 8 9 10; do
          if pids="$(collect_installed_host_pids)"; then
            sleep 0.1
          else
            query_status=$?
            [[ "$query_status" -eq 1 ]] || return 2
            stopped=1
            break
          fi
        done
      else
        query_status=$?
        [[ "$query_status" -eq 1 ]] || return 2
        stopped=1
      fi
    fi
    [[ "$stopped" -eq 1 ]] || { echo "input method host did not stop" >&2; return 2; }
  else
    query_status=$?
    [[ "$query_status" -eq 1 ]] || { echo "unable to query installed input method host process state" >&2; return 2; }
  fi
}

validate_bundle() {
  local bundle="$1" identifier version executable file_path architectures mach_count=0
  [[ -d "$bundle" ]] || { echo "missing input method bundle: $bundle" >&2; return 2; }
  [[ ! -L "$bundle" ]] || { echo "refusing symlink bundle: $bundle" >&2; return 2; }
  [[ -z "$(find "$bundle" ! -type d ! -type f -print -quit)" ]] \
    || { echo "refusing bundle containing symlinks or special entries: $bundle" >&2; return 2; }
  [[ -f "$bundle/Contents/Info.plist" && ! -L "$bundle/Contents/Info.plist" ]] \
    || { echo "missing regular input method Info.plist" >&2; return 2; }
  identifier="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$bundle/Contents/Info.plist" 2>/dev/null)" \
    || { echo "unable to read input method bundle identifier" >&2; return 2; }
  version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$bundle/Contents/Info.plist" 2>/dev/null)" \
    || { echo "unable to read input method bundle version" >&2; return 2; }
  [[ "$identifier" == "com.hongye.facai.inputmethod.zhizi" ]] || { echo "unexpected bundle id: $identifier" >&2; return 2; }
  # the current VERSION, or the 0.3.0 internal version line that earlier installs and backups carry (rollback must still validate)
  [[ "$version" == "$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")" || "$version" == "0.3.0" ]] \
    || { echo "unexpected version: $version" >&2; return 2; }
  for executable in AgentInputMethod AgentIMEInputSourceCtl AgentIMEPersonalReplayHarness AgentIMEUserDBCtl; do
    [[ -f "$bundle/Contents/MacOS/$executable" && -x "$bundle/Contents/MacOS/$executable" ]] \
      || { echo "missing executable input method payload: $executable" >&2; return 2; }
  done
  while IFS= read -r file_path; do
    [[ "$(stat -f '%l' "$file_path")" -eq 1 ]] || {
      echo "refusing hard-linked input method payload: ${file_path#"$bundle"/}" >&2
      return 2
    }
    file "$file_path" | grep -q 'Mach-O' || continue
    mach_count=$((mach_count + 1))
    architectures="$(lipo -archs "$file_path")" || return 2
    [[ "$architectures" == "arm64" ]] || {
      echo "input method payload is not arm64-only: ${file_path#"$bundle"/} ($architectures)" >&2
      return 2
    }
  done < <(find "$bundle/Contents" -type f -print | LC_ALL=C sort)
  [[ "$mach_count" -gt 0 ]] || { echo "input method bundle contains no Mach-O payload" >&2; return 2; }
  codesign --verify --deep --strict "$bundle"
}

validate_bundle_checksum() {
  local bundle="$1" expected="$2" actual
  [[ "$expected" =~ ^[0-9a-f]{64}$ ]] || return 2
  validate_bundle "$bundle" >/dev/null 2>&1 || return 2
  actual="$(bundle_checksum "$bundle")" || return 2
  [[ "$actual" == "$expected" ]]
}

bundle_build_number() {
  local bundle="$1" build
  build="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$bundle/Contents/Info.plist" 2>/dev/null)" \
    || return 2
  [[ "$build" =~ ^[1-9][0-9]{0,8}$ ]] || return 2
  printf '%s\n' "$build"
}

enforce_official_upgrade_policy() {
  local current="$1" source="$2" current_build source_build current_sha source_sha
  [[ "$TEST_MODE" != "1" ]] || return 0
  [[ -n "$current" ]] || return 0
  current_build="$(bundle_build_number "$current")" \
    || { echo "installed input method has an invalid build number" >&2; return 3; }
  source_build="$(bundle_build_number "$source")" \
    || { echo "frozen RC has an invalid build number" >&2; return 3; }
  current_sha="$(bundle_checksum "$current")" || return 3
  source_sha="$(bundle_checksum "$source")" || return 3
  if (( 10#$source_build < 10#$current_build )); then
    echo "refusing input method downgrade: installed=$current_build frozen_rc=$source_build" >&2
    return 3
  fi
  if [[ "$source_build" == "$current_build" && "$source_sha" != "$current_sha" ]]; then
    echo "refusing a different artifact with the installed build number: $source_build" >&2
    return 3
  fi
}

current_bundle() {
  local has_current=0 has_legacy=0
  [[ -e "$DESTINATION" || -L "$DESTINATION" ]] && has_current=1
  [[ -e "$LEGACY_DESTINATION" || -L "$LEGACY_DESTINATION" ]] && has_legacy=1
  if [[ "$has_current" -eq 1 && "$has_legacy" -eq 1 ]]; then
    echo "refusing ambiguous install state: both current and legacy bundles exist" >&2
    return 2
  fi
  if [[ "$has_current" -eq 1 ]]; then printf '%s\n' "$DESTINATION"; return 0; fi
  if [[ "$has_legacy" -eq 1 ]]; then printf '%s\n' "$LEGACY_DESTINATION"; return 0; fi
  return 1
}

resolve_current() {
  RESOLVED_CURRENT=""
  local status
  if RESOLVED_CURRENT="$(current_bundle)"; then
    return 0
  else
    status=$?
  fi
  [[ "$status" -eq 1 ]] && return 1
  return "$status"
}

register_bundle() {
  local bundle="$1"
  if [[ -n "${AIM_INSTALL_REGISTER_HELPER:-}" ]]; then
    [[ -x "$AIM_INSTALL_REGISTER_HELPER" ]] || { echo "test registration helper is not executable" >&2; return 2; }
    "$AIM_INSTALL_REGISTER_HELPER" register "$bundle"
  else
    "$bundle/Contents/MacOS/AgentIMEInputSourceCtl" register "$bundle"
  fi
}

# The 2026-07-17 incident: after a bundle replacement the input source silently
# vanished from the session lists — the host process kept running, but macOS
# stopped delivering keys and the owner had no way to summon the IME. TIS exit
# codes alone were not trustworthy, so enablement is cross-checked against the
# login session's HIToolbox state. Calibrated on a real 2026-07-18 install:
# on current macOS a healthy, actively-selected input method can be absent
# from AppleEnabledInputSources, so presence in EITHER the enabled list OR
# AppleSelectedInputSources counts; in the incident state it was in neither.
enabled_input_source_listed() {
  local listed
  if [[ -n "${AIM_INSTALL_ENABLED_LIST_HELPER:-}" ]]; then
    listed="$("$AIM_INSTALL_ENABLED_LIST_HELPER" 2>/dev/null)" || return 1
  else
    listed="$({ defaults read com.apple.HIToolbox AppleEnabledInputSources 2>/dev/null;
                defaults read com.apple.HIToolbox AppleSelectedInputSources 2>/dev/null; })"
    [[ -n "$listed" ]] || return 1
  fi
  [[ "$listed" == *"$INPUT_SOURCE_BUNDLE_ID"* ]]
}

ensure_input_source_enabled() {
  local bundle="$1"
  local -a ctl
  if [[ -n "${AIM_INSTALL_ENABLE_CTL_HELPER:-}" ]]; then
    [[ -x "$AIM_INSTALL_ENABLE_CTL_HELPER" ]] || { echo "test enable helper is not executable" >&2; return 2; }
    ctl=("$AIM_INSTALL_ENABLE_CTL_HELPER")
  elif [[ "$TEST_MODE" == "1" ]]; then
    # Fixture sessions have no TIS inventory; dedicated e2e cases exercise
    # this path through AIM_INSTALL_ENABLE_CTL_HELPER.
    return 0
  else
    ctl=("$bundle/Contents/MacOS/AgentIMEInputSourceCtl")
  fi
  local attempt
  for attempt in 1 2; do
    "${ctl[@]}" enable >/dev/null 2>&1 || true
    if "${ctl[@]}" status >/dev/null 2>&1 && enabled_input_source_listed; then
      return 0
    fi
    if [[ "$attempt" -eq 1 ]]; then
      # Mirror the manual recovery that resolved the incident, once.
      refresh_services "$bundle"
      register_bundle "$bundle" || true
    fi
  done
  die "input method files are installed, but the input source did not return to the session's enabled/selected lists; run: \"$LSREGISTER\" -f \"$bundle\" && \"$bundle/Contents/MacOS/AgentIMEInputSourceCtl\" register \"$bundle\" && \"$bundle/Contents/MacOS/AgentIMEInputSourceCtl\" select (or add it from the macOS input menu; first-time installs may additionally need a log out/in)" 8
}

test_fail() {
  local point="$1"
  if [[ "$TEST_MODE" == "1" && "${AIM_INSTALL_FAIL_AT:-}" == "$point" ]]; then
    echo "injected installer failure: $point" >&2
    return 72
  fi
}

test_crash() {
  local point="$1"
  if [[ "$TEST_MODE" == "1" && "${AIM_INSTALL_CRASH_AT:-}" == "$point" ]]; then
    echo "injected installer SIGKILL: $point" >&2
    kill -9 "$$"
  fi
}

test_host_event() {
  local event="$1"
  if [[ "$TEST_MODE" == "1" && -n "$PROCESS_HELPER" ]]; then
    "$PROCESS_HELPER" event "$event"
  fi
}

persist_journal() {
  local temporary
  temporary="$(mktemp "$STATE_DIR/.transaction.journal.XXXXXX")" || return 1
  if ! printf '%s\n' \
    'format=2' \
    "phase=$TRANSACTION_PHASE" \
    "action=$TRANSACTION_ACTION" \
    "original=$TRANSACTION_ORIGINAL" \
    "originalChecksum=$TRANSACTION_ORIGINAL_CHECKSUM" \
    "newChecksum=$TRANSACTION_NEW_CHECKSUM" \
    "root=$TRANSACTION_ROOT" \
    "old=$TRANSACTION_OLD" \
    "stageRoot=$TRANSACTION_STAGE_ROOT" \
    "backupStage=$TRANSACTION_BACKUP_STAGE" \
    "backupStageRoot=$TRANSACTION_BACKUP_STAGE_ROOT" \
    "backupCommitRoot=$BACKUP_COMMIT_ROOT" \
    "backupChecksumNew=$BACKUP_CHECKSUM_NEW" \
    "backupOldChecksum=$BACKUP_OLD_CHECKSUM" \
    "backupNewChecksum=$BACKUP_NEW_CHECKSUM" > "$temporary" ||
     ! chmod 600 "$temporary" || ! mv -f "$temporary" "$TRANSACTION_JOURNAL"; then
    rm -f "$temporary"
    return 1
  fi
}

set_transaction_phase() {
  TRANSACTION_PHASE="$1"
  persist_journal || return 1
  test_host_event "$1" || return 1
  test_crash "$1"
}

write_recovery_marker() {
  printf 'phase=%s\ntransaction_root=%s\nbackup_stage_root=%s\n' \
    "$TRANSACTION_PHASE" "$TRANSACTION_ROOT" "$TRANSACTION_BACKUP_STAGE_ROOT" > "$RECOVERY_REQUIRED" || return 1
  chmod 600 "$RECOVERY_REQUIRED" || return 1
}

validate_generated_path() {
  local path="$1" parent="$2" prefix="$3" base suffix
  [[ -n "$path" && "$(dirname "$path")" == "$parent" ]] || return 1
  base="$(basename "$path")"
  [[ "$base" == "$prefix"* ]] || return 1
  suffix="${base#"$prefix"}"
  [[ "$suffix" =~ ^[A-Za-z0-9]+$ ]]
}

load_journal() {
  local key value format="" phase="" action="" original="" original_checksum="" new_checksum="" root="" old="" stage_root="" backup_stage="" backup_stage_root="" backup_commit_root="" backup_checksum_new="" backup_old_checksum="" backup_new_checksum=""
  local seen='|' field_count=0
  [[ -f "$TRANSACTION_JOURNAL" && ! -L "$TRANSACTION_JOURNAL" ]] || return 2
  while IFS='=' read -r key value; do
    [[ -n "$key" && "$seen" != *"|$key|"* ]] || return 2
    seen+="$key|"
    field_count=$((field_count + 1))
    case "$key" in
      format) format="$value" ;;
      phase) phase="$value" ;;
      action) action="$value" ;;
      original) original="$value" ;;
      originalChecksum) original_checksum="$value" ;;
      newChecksum) new_checksum="$value" ;;
      root) root="$value" ;;
      old) old="$value" ;;
      stageRoot) stage_root="$value" ;;
      backupStage) backup_stage="$value" ;;
      backupStageRoot) backup_stage_root="$value" ;;
      backupCommitRoot) backup_commit_root="$value" ;;
      backupChecksumNew) backup_checksum_new="$value" ;;
      backupOldChecksum) backup_old_checksum="$value" ;;
      backupNewChecksum) backup_new_checksum="$value" ;;
      *) return 2 ;;
    esac
  done < "$TRANSACTION_JOURNAL"
  [[ "$field_count" -eq 15 ]] || return 2
  [[ "$format" == "2" ]] || return 2
  case "$phase" in prepared|old_moved|new_moved|committed|recovered) ;; *) return 2 ;; esac
  case "$action" in --install|--rollback|--uninstall) ;; *) return 2 ;; esac
  case "$original" in ""|"$DESTINATION"|"$LEGACY_DESTINATION") ;; *) return 2 ;; esac
  if [[ -z "$original" ]]; then
    [[ -z "$original_checksum" ]] || return 2
  else
    [[ "$original_checksum" =~ ^[0-9a-f]{64}$ ]] || return 2
  fi
  if [[ "$action" == "--uninstall" ]]; then
    [[ -z "$new_checksum" ]] || return 2
  else
    [[ "$new_checksum" =~ ^[0-9a-f]{64}$ ]] || return 2
  fi
  validate_generated_path "$root" "$INSTALL_DIR" '.agent-ime-transaction.' || return 2
  [[ "$old" == "$root/original.inputmethod" && ! -L "$root" ]] || return 2
  if [[ "$phase" != "committed" && "$phase" != "recovered" ]]; then [[ -d "$root" ]] || return 2; fi
  if [[ -e "$root" ]]; then [[ -d "$root" ]] || return 2; fi
  if [[ -n "$stage_root" ]]; then
    validate_generated_path "$stage_root" "$INSTALL_DIR" '.agent-ime-stage.' || return 2
    [[ ! -L "$stage_root" ]] || return 2
  fi
  if [[ -n "$backup_stage_root" ]]; then
    validate_generated_path "$backup_stage_root" "$STATE_DIR" '.agent-ime-stage.' || return 2
    [[ ! -L "$backup_stage_root" ]] || return 2
    [[ -z "$backup_stage" || "$backup_stage" == "$backup_stage_root/previous.inputmethod" ]] || return 2
  fi
  if [[ -n "$backup_commit_root" ]]; then
    validate_generated_path "$backup_commit_root" "$STATE_DIR" '.agent-ime-backup-commit.' || return 2
    [[ ! -L "$backup_commit_root" ]] || return 2
  fi
  if [[ -n "$backup_checksum_new" ]]; then
    validate_generated_path "$backup_checksum_new" "$STATE_DIR" '.previous.bundle.sha256.' || return 2
    [[ ! -L "$backup_checksum_new" ]] || return 2
    if [[ -f "$backup_checksum_new" ]]; then [[ "$(stat -f '%l' "$backup_checksum_new")" -eq 1 ]] || return 2; fi
  fi
  [[ -z "$backup_old_checksum" || "$backup_old_checksum" =~ ^[0-9a-f]{64}$ ]] || return 2
  [[ -z "$backup_new_checksum" || "$backup_new_checksum" =~ ^[0-9a-f]{64}$ ]] || return 2
  if [[ -n "$backup_commit_root" ]]; then
    [[ "$backup_new_checksum" =~ ^[0-9a-f]{64}$ ]] || return 2
  fi
  TRANSACTION_PHASE="$phase"
  TRANSACTION_ACTION="$action"
  TRANSACTION_ORIGINAL="$original"
  TRANSACTION_ORIGINAL_CHECKSUM="$original_checksum"
  TRANSACTION_NEW_CHECKSUM="$new_checksum"
  TRANSACTION_ROOT="$root"
  TRANSACTION_OLD="$old"
  TRANSACTION_STAGE_ROOT="$stage_root"
  TRANSACTION_BACKUP_STAGE="$backup_stage"
  TRANSACTION_BACKUP_STAGE_ROOT="$backup_stage_root"
  BACKUP_COMMIT_ROOT="$backup_commit_root"
  BACKUP_CHECKSUM_NEW="$backup_checksum_new"
  BACKUP_OLD_CHECKSUM="$backup_old_checksum"
  BACKUP_NEW_CHECKSUM="$backup_new_checksum"
}

backup_pair_is_valid() {
  if [[ ! -e "$BACKUP" && ! -e "$PREVIOUS_CHECKSUM" ]]; then return 0; fi
  [[ -d "$BACKUP" && ! -L "$BACKUP" && -f "$PREVIOUS_CHECKSUM" && ! -L "$PREVIOUS_CHECKSUM" ]] || return 1
  validate_bundle "$BACKUP" >/dev/null 2>&1 || return 1
  [[ "$(awk 'NR == 1 { print $1 }' "$PREVIOUS_CHECKSUM")" == "$(bundle_checksum "$BACKUP")" ]]
}

migrate_legacy_state() {
  local legacy_expected current=""
  if [[ -d "$BACKUP" && ! -e "$PREVIOUS_CHECKSUM" ]]; then
    [[ -f "$LEGACY_PREVIOUS_CHECKSUM" && ! -L "$LEGACY_PREVIOUS_CHECKSUM" ]] || return 70
    validate_bundle "$BACKUP" >/dev/null 2>&1 || return 70
    legacy_expected="$(awk 'NR == 1 { print $1 }' "$LEGACY_PREVIOUS_CHECKSUM")"
    [[ "$legacy_expected" == "$(shasum -a 256 "$BACKUP/Contents/MacOS/AgentInputMethod" | awk '{ print $1 }')" ]] || return 70
    write_checksum "$(bundle_checksum "$BACKUP")" "previous.inputmethod" "$PREVIOUS_CHECKSUM" || return 70
    echo "migrated legacy rollback checksum state"
  fi
  if [[ ! -e "$CURRENT_CHECKSUM" ]]; then
    if resolve_current; then current="$RESOLVED_CURRENT"; else current_status=$?; [[ "$current_status" -eq 1 ]] || return "$current_status"; fi
    if [[ -n "$current" ]]; then
      validate_bundle "$current" >/dev/null 2>&1 || return 70
      if [[ -e "$LEGACY_CURRENT_CHECKSUM" || -L "$LEGACY_CURRENT_CHECKSUM" ]]; then
        [[ -f "$LEGACY_CURRENT_CHECKSUM" && ! -L "$LEGACY_CURRENT_CHECKSUM" ]] || return 70
        legacy_expected="$(awk 'NR == 1 { print $1 }' "$LEGACY_CURRENT_CHECKSUM")"
        if [[ "$legacy_expected" != "$(shasum -a 256 "$current/Contents/MacOS/AgentInputMethod" | awk '{ print $1 }')" ]]; then
          [[ -d "$BACKUP" && -f "$PREVIOUS_CHECKSUM" &&
             "$(bundle_checksum "$current")" == "$(bundle_checksum "$BACKUP")" &&
             "$(awk 'NR == 1 { print $1 }' "$PREVIOUS_CHECKSUM")" == "$(bundle_checksum "$BACKUP")" ]] || return 70
          echo "detected legacy rollback state with stale current.sha256; recomputing full current checksum"
        fi
      fi
      write_checksum "$(bundle_checksum "$current")" "$(basename "$current")" "$CURRENT_CHECKSUM" || return 70
      echo "migrated legacy current checksum state"
    fi
  fi
}

current_pair_is_valid() {
  local current="" status expected actual
  if resolve_current; then
    current="$RESOLVED_CURRENT"
  else
    status=$?
    if [[ "$status" -eq 1 ]]; then
      [[ ! -e "$CURRENT_CHECKSUM" && ! -L "$CURRENT_CHECKSUM" ]]
      return
    fi
    return "$status"
  fi
  validate_bundle "$current" >/dev/null 2>&1 || return 1
  [[ -f "$CURRENT_CHECKSUM" && ! -L "$CURRENT_CHECKSUM" ]] || return 1
  expected="$(awk 'NR == 1 { print $1; exit }' "$CURRENT_CHECKSUM")"
  [[ "$expected" =~ ^[0-9a-f]{64}$ ]] || return 1
  actual="$(bundle_checksum "$current")" || return 1
  [[ "$actual" == "$expected" ]]
}

recover_backup_commit() {
  [[ -n "$BACKUP_COMMIT_ROOT" ]] || return 0
  local old_backup="$BACKUP_COMMIT_ROOT/previous.inputmethod"
  local old_checksum="$BACKUP_COMMIT_ROOT/previous.bundle.sha256"
  local live_checksum="" old_file_checksum="" live_backup_checksum=""
  [[ ! -L "$old_backup" && ! -L "$old_checksum" ]] || return 1
  if [[ -e "$PREVIOUS_CHECKSUM" || -L "$PREVIOUS_CHECKSUM" ]]; then
    [[ -f "$PREVIOUS_CHECKSUM" && ! -L "$PREVIOUS_CHECKSUM" ]] || return 1
    live_checksum="$(awk 'NR == 1 { print $1; exit }' "$PREVIOUS_CHECKSUM")"
    [[ "$live_checksum" =~ ^[0-9a-f]{64}$ ]] || return 1
  fi
  if [[ -e "$BACKUP" || -L "$BACKUP" ]]; then
    [[ -d "$BACKUP" && ! -L "$BACKUP" ]] || return 1
    validate_bundle "$BACKUP" >/dev/null 2>&1 || return 1
    live_backup_checksum="$(bundle_checksum "$BACKUP")" || return 1
  fi
  if [[ -e "$old_checksum" ]]; then
    [[ -f "$old_checksum" && "$(stat -f '%l' "$old_checksum")" -eq 1 ]] || return 1
    old_file_checksum="$(awk 'NR == 1 { print $1; exit }' "$old_checksum")"
    [[ "$old_file_checksum" =~ ^[0-9a-f]{64}$ ]] || return 1
  fi
  if [[ "$TRANSACTION_PHASE" == "committed" ]]; then
    [[ -n "$BACKUP_NEW_CHECKSUM" && -d "$BACKUP" && ! -L "$BACKUP" &&
       "$live_checksum" == "$BACKUP_NEW_CHECKSUM" && "$live_backup_checksum" == "$BACKUP_NEW_CHECKSUM" ]] || return 1
    remove_tree "$BACKUP_COMMIT_ROOT" "$BACKUP_CHECKSUM_NEW" || return 1
  else
    if [[ -n "$BACKUP_OLD_CHECKSUM" ]]; then
      [[ "$BACKUP_OLD_CHECKSUM" =~ ^[0-9a-f]{64}$ ]] || return 1
      if [[ -e "$old_backup" ]]; then
        validate_bundle_checksum "$old_backup" "$BACKUP_OLD_CHECKSUM" || return 1
        if [[ -n "$old_file_checksum" ]]; then
          [[ "$old_file_checksum" == "$BACKUP_OLD_CHECKSUM" ]] || return 1
        else
          [[ "$live_checksum" == "$BACKUP_OLD_CHECKSUM" ]] || return 1
        fi
        if [[ -n "$live_backup_checksum" && "$live_backup_checksum" != "$BACKUP_NEW_CHECKSUM" &&
              "$live_backup_checksum" != "$BACKUP_OLD_CHECKSUM" ]]; then return 1; fi
        remove_tree "$BACKUP" || return 1
        move_bundle "$old_backup" "$BACKUP" || return 1
        test_crash "recovery_backup_restored"
      else
        [[ "$live_backup_checksum" == "$BACKUP_OLD_CHECKSUM" ]] || return 1
      fi
      if [[ -n "$old_file_checksum" ]]; then
        [[ "$old_file_checksum" == "$BACKUP_OLD_CHECKSUM" ]] || return 1
        if [[ -n "$live_checksum" && "$live_checksum" != "$BACKUP_NEW_CHECKSUM" &&
              "$live_checksum" != "$BACKUP_OLD_CHECKSUM" ]]; then return 1; fi
        rm -f "$PREVIOUS_CHECKSUM" || return 1
        mv "$old_checksum" "$PREVIOUS_CHECKSUM" || return 1
        test_crash "recovery_checksum_restored"
      else
        [[ "$live_checksum" == "$BACKUP_OLD_CHECKSUM" ]] || return 1
      fi
    else
      [[ ! -e "$old_backup" && ! -e "$old_checksum" ]] || return 1
      if [[ -n "$live_backup_checksum" ]]; then
        [[ "$live_backup_checksum" == "$BACKUP_NEW_CHECKSUM" ]] || return 1
        remove_tree "$BACKUP" || return 1
      fi
      if [[ -n "$live_checksum" ]]; then
        [[ "$live_checksum" == "$BACKUP_NEW_CHECKSUM" ]] || return 1
        rm -f "$PREVIOUS_CHECKSUM" || return 1
      fi
    fi
    remove_tree "$BACKUP_COMMIT_ROOT" "$BACKUP_CHECKSUM_NEW" || return 1
  fi
  BACKUP_COMMIT_ROOT=""
  BACKUP_CHECKSUM_NEW=""
  BACKUP_OLD_CHECKSUM=""
  BACKUP_NEW_CHECKSUM=""
  persist_journal || return 1
  backup_pair_is_valid
}

prepare_recovery_copy() {
  local expected="$1" source="" candidate_checksum recovery_copy
  RECOVERY_COPY=""
  if [[ -n "$TRANSACTION_BACKUP_STAGE" ]] &&
     validate_bundle_checksum "$TRANSACTION_BACKUP_STAGE" "$expected"; then
    source="$TRANSACTION_BACKUP_STAGE"
  elif [[ -d "$BACKUP" && ! -L "$BACKUP" && -f "$PREVIOUS_CHECKSUM" && ! -L "$PREVIOUS_CHECKSUM" ]]; then
    candidate_checksum="$(awk 'NR == 1 { print $1; exit }' "$PREVIOUS_CHECKSUM")"
    if [[ "$candidate_checksum" == "$expected" ]] && validate_bundle_checksum "$BACKUP" "$expected"; then
      source="$BACKUP"
    fi
  fi
  [[ -n "$source" ]] || return 1
  recovery_copy="$TRANSACTION_ROOT/recovery-copy.inputmethod"
  [[ ! -e "$recovery_copy" && ! -L "$recovery_copy" ]] || return 1
  ditto "$source" "$recovery_copy" || return 1
  validate_bundle_checksum "$recovery_copy" "$expected" || { remove_tree "$recovery_copy"; return 1; }
  RECOVERY_COPY="$recovery_copy"
}

current_checksum_matches() {
  local expected="$1"
  [[ -f "$CURRENT_CHECKSUM" && ! -L "$CURRENT_CHECKSUM" ]] || return 1
  [[ "$(awk 'NR == 1 { print $1; exit }' "$CURRENT_CHECKSUM")" == "$expected" ]]
}

recovered_state_is_valid() {
  if [[ -z "$TRANSACTION_ORIGINAL" ]]; then
    [[ ! -e "$DESTINATION" && ! -L "$DESTINATION" &&
       ! -e "$LEGACY_DESTINATION" && ! -L "$LEGACY_DESTINATION" &&
       ! -e "$CURRENT_CHECKSUM" && ! -L "$CURRENT_CHECKSUM" ]]
    return
  fi
  local other="$DESTINATION"
  [[ "$TRANSACTION_ORIGINAL" == "$DESTINATION" ]] && other="$LEGACY_DESTINATION"
  validate_bundle_checksum "$TRANSACTION_ORIGINAL" "$TRANSACTION_ORIGINAL_CHECKSUM" &&
    [[ ! -e "$other" && ! -L "$other" ]] &&
    current_checksum_matches "$TRANSACTION_ORIGINAL_CHECKSUM"
}

committed_state_is_valid() {
  if [[ "$TRANSACTION_ACTION" == "--uninstall" ]]; then
    [[ ! -e "$DESTINATION" && ! -L "$DESTINATION" &&
       ! -e "$LEGACY_DESTINATION" && ! -L "$LEGACY_DESTINATION" &&
       ! -e "$CURRENT_CHECKSUM" && ! -L "$CURRENT_CHECKSUM" ]]
    return
  fi
  validate_bundle_checksum "$DESTINATION" "$TRANSACTION_NEW_CHECKSUM" &&
    [[ ! -e "$LEGACY_DESTINATION" && ! -L "$LEGACY_DESTINATION" ]] &&
    current_checksum_matches "$TRANSACTION_NEW_CHECKSUM"
}

attempt_transaction_recovery() {
  local source="" restored="" other="$DESTINATION" live_kind="absent"
  local displaced="$TRANSACTION_ROOT/recovery-displaced.inputmethod" failed=0 displaced_present=0
  echo "recovering installer transaction phase=$TRANSACTION_PHASE" >&2

  # Validate every recovery source and every live bundle before stopping the
  # host or moving anything. A corrupt/symlinked old bundle therefore cannot
  # cause a valid current installation to be deleted first.
  # A prior recovery attempt may already have quarantined the exact new bundle
  # before registration or checksum persistence failed. Treat that quarantine
  # as durable transaction state so a later --recover can resume safely.
  if [[ -e "$displaced" || -L "$displaced" ]]; then
    [[ -n "$TRANSACTION_NEW_CHECKSUM" ]] || return 1
    validate_bundle_checksum "$displaced" "$TRANSACTION_NEW_CHECKSUM" || return 1
    displaced_present=1
  fi
  if [[ -z "$TRANSACTION_ORIGINAL" ]]; then
    [[ ! -e "$TRANSACTION_OLD" && ! -L "$TRANSACTION_OLD" ]] || return 1
    if [[ -e "$LEGACY_DESTINATION" || -L "$LEGACY_DESTINATION" ]]; then return 1; fi
    if [[ -e "$DESTINATION" || -L "$DESTINATION" ]]; then
      [[ "$displaced_present" -eq 0 ]] || return 1
      validate_bundle_checksum "$DESTINATION" "$TRANSACTION_NEW_CHECKSUM" || return 1
      live_kind="new"
    fi
  else
    [[ "$TRANSACTION_ORIGINAL" == "$DESTINATION" ]] && other="$LEGACY_DESTINATION"
    if [[ -e "$TRANSACTION_OLD" || -L "$TRANSACTION_OLD" ]]; then
      validate_bundle_checksum "$TRANSACTION_OLD" "$TRANSACTION_ORIGINAL_CHECKSUM" || return 1
      source="$TRANSACTION_OLD"
    elif [[ -e "$TRANSACTION_ORIGINAL" || -L "$TRANSACTION_ORIGINAL" ]]; then
      validate_bundle_checksum "$TRANSACTION_ORIGINAL" "$TRANSACTION_ORIGINAL_CHECKSUM" || return 1
      [[ ! -e "$other" && ! -L "$other" ]] || return 1
      restored="$TRANSACTION_ORIGINAL"
      live_kind="original"
    else
      prepare_recovery_copy "$TRANSACTION_ORIGINAL_CHECKSUM" || return 1
      source="$RECOVERY_COPY"
    fi

    if [[ -n "$source" ]]; then
      if [[ -e "$DESTINATION" || -L "$DESTINATION" ]]; then
        [[ "$displaced_present" -eq 0 ]] || return 1
        validate_bundle_checksum "$DESTINATION" "$TRANSACTION_NEW_CHECKSUM" || return 1
        live_kind="new"
      fi
      if [[ -e "$LEGACY_DESTINATION" || -L "$LEGACY_DESTINATION" ]]; then
        # The legacy path may only be occupied when it is already the exact
        # original, which was handled above without selecting a source.
        return 1
      fi
    fi
  fi

  stop_running_host || return 1
  if [[ "$live_kind" == "new" ]]; then
    [[ "$displaced_present" -eq 0 ]] || return 1
    move_bundle "$DESTINATION" "$displaced" || return 1
    validate_bundle_checksum "$displaced" "$TRANSACTION_NEW_CHECKSUM" || return 1
    displaced_present=1
    test_crash "recovery_new_displaced"
  fi

  if [[ -n "$source" ]]; then
    if ! move_bundle "$source" "$TRANSACTION_ORIGINAL"; then
      [[ "$live_kind" != "new" ]] || move_bundle "$displaced" "$DESTINATION" || true
      return 1
    fi
    if ! validate_bundle_checksum "$TRANSACTION_ORIGINAL" "$TRANSACTION_ORIGINAL_CHECKSUM"; then
      failed=1
    else
      restored="$TRANSACTION_ORIGINAL"
      test_crash "recovery_original_restored"
    fi
  fi

  if [[ "$failed" -eq 0 && -n "$restored" ]]; then
    register_bundle "$restored" || failed=1
    [[ "$failed" -ne 0 ]] || test_crash "recovery_registered"
    refresh_services "$restored" || failed=1
    write_checksum "$TRANSACTION_ORIGINAL_CHECKSUM" "$(basename "$restored")" "$CURRENT_CHECKSUM" || failed=1
    [[ "$failed" -ne 0 ]] || test_crash "recovery_current_checksum_written"
  elif [[ "$failed" -eq 0 && -z "$TRANSACTION_ORIGINAL" ]]; then
    rm -f "$CURRENT_CHECKSUM" || failed=1
    refresh_services "" || failed=1
  fi

  if [[ "$failed" -eq 0 && "$displaced_present" -eq 1 ]]; then
    remove_tree "$displaced" || failed=1
  fi
  [[ "$failed" -eq 0 ]]
}

finish_recovery() {
  # Keep transaction-moved executable paths available until every old/new
  # host generation observed under the runtime lock has been drained.
  stop_running_host || return 1
  remove_tree "$TRANSACTION_STAGE_ROOT" "$TRANSACTION_BACKUP_STAGE_ROOT" "$TRANSACTION_ROOT" || return 1
  rm -f "$TRANSACTION_JOURNAL" "$RECOVERY_REQUIRED" || return 1
  TRANSACTION_PHASE="none"
  TRANSACTION_ORIGINAL=""
  TRANSACTION_OLD=""
  TRANSACTION_ROOT=""
  TRANSACTION_STAGE_ROOT=""
  TRANSACTION_BACKUP_STAGE=""
  TRANSACTION_BACKUP_STAGE_ROOT=""
  BACKUP_COMMIT_ROOT=""
  BACKUP_CHECKSUM_NEW=""
  BACKUP_OLD_CHECKSUM=""
  BACKUP_NEW_CHECKSUM=""
  TRANSACTION_ORIGINAL_CHECKSUM=""
  TRANSACTION_NEW_CHECKSUM=""
}

complete_rollback_recovery() {
  set_transaction_phase "recovered" || return 1
  finish_recovery || return 1
}

recover_durable_transaction() {
  if [[ ! -e "$TRANSACTION_JOURNAL" ]]; then
    if [[ -e "$RECOVERY_REQUIRED" ]]; then
      RECOVERY_PRESERVE=1
      echo "recovery marker exists without a usable journal; manual recovery is required" >&2
      return 70
    fi
    return 0
  fi
  if ! load_journal; then
    RECOVERY_PRESERVE=1
    TRANSACTION_PHASE="invalid-journal"
    write_recovery_marker || true
    echo "invalid installer transaction journal; recovery data preserved" >&2
    return 70
  fi
  if ! recover_backup_commit; then
    RECOVERY_PRESERVE=1
    write_recovery_marker || true
    echo "backup commit recovery failed closed; artifacts preserved" >&2
    return 70
  fi
  if [[ "$TRANSACTION_PHASE" == "recovered" ]]; then
    if recovered_state_is_valid && finish_recovery; then
      echo "completed previously recovered installer transaction"
      return 0
    fi
  elif [[ "$TRANSACTION_PHASE" == "committed" ]]; then
    if committed_state_is_valid && finish_recovery; then
      echo "recovered committed installer transaction"
      return 0
    fi
  fi
  if attempt_transaction_recovery && complete_rollback_recovery; then
    echo "recovered interrupted installer transaction"
    return 0
  fi
  RECOVERY_PRESERVE=1
  write_recovery_marker || true
  echo "durable installer recovery failed closed; artifacts preserved" >&2
  return 70
}

stage_bundle() {
  local source="$1" parent="$2" name="$3" expected="${4:-}"
  STAGED_ROOT="$(mktemp -d "$parent/.agent-ime-stage.XXXXXX")"
  STAGED_BUNDLE="$STAGED_ROOT/$name"
  ditto "$source" "$STAGED_BUNDLE"
  validate_bundle "$STAGED_BUNDLE"
  if [[ -n "$expected" ]]; then
    [[ "$(bundle_checksum "$STAGED_BUNDLE")" == "$expected" ]] || {
      echo "staged input method checksum mismatch" >&2
      return 2
    }
  fi
}

begin_replacement() {
  local original="$1" stage="$2" stage_root="$3" original_checksum="$4" new_checksum="$5"
  TRANSACTION_ROOT="$(mktemp -d "$INSTALL_DIR/.agent-ime-transaction.XXXXXX")"
  TRANSACTION_OLD="$TRANSACTION_ROOT/original.inputmethod"
  TRANSACTION_ORIGINAL="$original"
  TRANSACTION_ORIGINAL_CHECKSUM="$original_checksum"
  TRANSACTION_NEW_CHECKSUM="$new_checksum"
  TRANSACTION_STAGE_ROOT="$stage_root"
  set_transaction_phase "prepared" || return 1
  if [[ -n "$original" ]]; then
    test_fail "before-old-move"
    move_bundle "$original" "$TRANSACTION_OLD"
    set_transaction_phase "old_moved" || return 1
    test_fail "after-old-moved"
  fi
  move_bundle "$stage" "$DESTINATION"
  set_transaction_phase "new_moved" || return 1
  test_fail "after-new-moved"
}

begin_removal() {
  local original="$1" original_checksum="$2"
  TRANSACTION_ROOT="$(mktemp -d "$INSTALL_DIR/.agent-ime-transaction.XXXXXX")"
  TRANSACTION_OLD="$TRANSACTION_ROOT/original.inputmethod"
  TRANSACTION_ORIGINAL="$original"
  TRANSACTION_ORIGINAL_CHECKSUM="$original_checksum"
  TRANSACTION_NEW_CHECKSUM=""
  set_transaction_phase "prepared" || return 1
  move_bundle "$original" "$TRANSACTION_OLD"
  set_transaction_phase "old_moved" || return 1
  test_fail "after-old-moved"
}

commit_previous_backup() {
  local stage="$1" stage_root="$2" checksum="$3"
  local commit_root checksum_new old_backup old_checksum
  if [[ -d "$BACKUP" && ! -L "$BACKUP" && -f "$PREVIOUS_CHECKSUM" && ! -L "$PREVIOUS_CHECKSUM" ]]; then
    BACKUP_OLD_CHECKSUM="$(awk 'NR == 1 { print $1; exit }' "$PREVIOUS_CHECKSUM")"
    validate_bundle_checksum "$BACKUP" "$BACKUP_OLD_CHECKSUM" || return 1
  elif [[ ! -e "$BACKUP" && ! -L "$BACKUP" && ! -e "$PREVIOUS_CHECKSUM" && ! -L "$PREVIOUS_CHECKSUM" ]]; then
    BACKUP_OLD_CHECKSUM=""
  else
    return 1
  fi
  commit_root="$(mktemp -d "$STATE_DIR/.agent-ime-backup-commit.XXXXXX")"
  if ! checksum_new="$(mktemp "$STATE_DIR/.previous.bundle.sha256.XXXXXX")"; then
    remove_tree "$commit_root"
    return 1
  fi
  BACKUP_COMMIT_ROOT="$commit_root"
  BACKUP_CHECKSUM_NEW="$checksum_new"
  BACKUP_NEW_CHECKSUM="$checksum"
  persist_journal || return 1
  test_crash "backup_prepared"
  old_backup="$commit_root/previous.inputmethod"
  old_checksum="$commit_root/previous.bundle.sha256"
  printf '%s  previous.inputmethod\n' "$checksum" > "$checksum_new"
  chmod 600 "$checksum_new"

  if [[ -e "$BACKUP" || -L "$BACKUP" ]]; then
    if ! move_bundle "$BACKUP" "$old_backup"; then remove_tree "$commit_root" "$checksum_new"; return 1; fi
    test_crash "backup_old_moved"
  fi
  if [[ -e "$PREVIOUS_CHECKSUM" || -L "$PREVIOUS_CHECKSUM" ]]; then
    if ! mv "$PREVIOUS_CHECKSUM" "$old_checksum"; then
      [[ ! -e "$old_backup" && ! -L "$old_backup" ]] || move_bundle "$old_backup" "$BACKUP"
      remove_tree "$commit_root" "$checksum_new"
      return 1
    fi
    test_crash "backup_checksum_moved"
  fi
  if ! move_bundle "$stage" "$BACKUP"; then
    [[ ! -e "$old_backup" && ! -L "$old_backup" ]] || move_bundle "$old_backup" "$BACKUP"
    [[ ! -e "$old_checksum" && ! -L "$old_checksum" ]] || mv "$old_checksum" "$PREVIOUS_CHECKSUM"
    remove_tree "$commit_root" "$checksum_new"
    return 1
  fi
  test_crash "backup_new_moved"
  TRANSACTION_BACKUP_STAGE=""
  if ! mv "$checksum_new" "$PREVIOUS_CHECKSUM"; then
    remove_tree "$BACKUP"
    [[ ! -e "$old_backup" && ! -L "$old_backup" ]] || move_bundle "$old_backup" "$BACKUP"
    [[ ! -e "$old_checksum" && ! -L "$old_checksum" ]] || mv "$old_checksum" "$PREVIOUS_CHECKSUM"
    remove_tree "$commit_root"
    return 1
  fi
  test_crash "backup_new_checksum_moved"
  if ! remove_tree "$stage_root"; then echo "warning: stale backup stage retained at $stage_root" >&2; fi
  TRANSACTION_BACKUP_STAGE_ROOT=""
  persist_journal || return 1
}

commit_transaction() {
  local preserve_old="${1:-}" root="$TRANSACTION_ROOT" old="$TRANSACTION_OLD"
  set_transaction_phase "committed" || return 1
  recover_backup_commit || return 1
  # The old executable can still be reported at TRANSACTION_OLD while a
  # blocked IMK relaunch waits on .runtime-recovery.lock. Drain it before the
  # transaction root is removed, then cleanup performs one final drain before
  # releasing the lock.
  stop_running_host || return 1
  if ! remove_tree "$TRANSACTION_STAGE_ROOT"; then
    echo "unable to remove committed install stage: $TRANSACTION_STAGE_ROOT" >&2
    return 1
  fi
  if [[ -n "$preserve_old" && ( -e "$old" || -L "$old" ) ]]; then
    if ! remove_tree "$preserve_old"; then
      echo "prior failed-current bundle could not be replaced" >&2
      return 1
    elif ! move_bundle "$old" "$preserve_old"; then
      echo "prior bundle retained at $old" >&2
      return 1
    fi
  fi
  if [[ -n "$root" ]] && ! remove_tree "$root"; then
    echo "transaction debris retained at $root" >&2
    return 1
  fi
  rm -f "$TRANSACTION_JOURNAL" || return 1
  TRANSACTION_ROOT=""
  TRANSACTION_OLD=""
  TRANSACTION_ORIGINAL=""
  TRANSACTION_ORIGINAL_CHECKSUM=""
  TRANSACTION_NEW_CHECKSUM=""
  TRANSACTION_STAGE_ROOT=""
  BACKUP_OLD_CHECKSUM=""
  TRANSACTION_PHASE="none"
}

cleanup() {
  local status=$? recovery_failed=0 runtime_failed=0
  trap - EXIT
  set +e
  if [[ "$RECOVERY_PRESERVE" -eq 1 ]]; then
    recovery_failed=1
  elif [[ "$TRANSACTION_PHASE" == "recovered" ]]; then
    if recovered_state_is_valid && finish_recovery; then recovery_failed=0; else recovery_failed=1; fi
  elif [[ "$TRANSACTION_PHASE" == "committed" ]]; then
    if recover_backup_commit && committed_state_is_valid && finish_recovery; then
      recovery_failed=0
    elif attempt_transaction_recovery && complete_rollback_recovery; then
      recovery_failed=0
    else
      recovery_failed=1
    fi
  elif [[ "$TRANSACTION_PHASE" != "none" ]]; then
    if recover_backup_commit && attempt_transaction_recovery && complete_rollback_recovery; then recovery_failed=0; else recovery_failed=1; fi
  fi

  if [[ "$RUNTIME_LOCK_HELD" -eq 1 ]]; then
    stop_running_host || runtime_failed=1
    release_runtime_lock || runtime_failed=1
  elif [[ -n "$RUNTIME_LOCK_SESSION" ]]; then
    remove_tree "$RUNTIME_LOCK_SESSION" || runtime_failed=1
    RUNTIME_LOCK_SESSION=""
  fi
  if [[ "$runtime_failed" -ne 0 ]]; then
    echo "runtime host drain or lock release failed" >&2
    [[ "$status" -ne 0 ]] || status=70
  fi

  if [[ "$recovery_failed" -eq 0 ]]; then
    remove_tree "$STAGED_ROOT" "$STAGE_ROOT_A" "$STAGE_ROOT_B" \
      "$TRANSACTION_STAGE_ROOT" "$TRANSACTION_BACKUP_STAGE_ROOT"
    remove_tree "$TRANSACTION_ROOT"
    [[ "$status" -ne 0 ]] || rm -f "$RECOVERY_REQUIRED"
    if [[ "$LOCK_HELD" -eq 1 ]]; then remove_tree "$LOCK_DIR"; fi
  else
    write_recovery_marker || echo "failed to persist recovery marker; installer lock retained" >&2
  fi
  if [[ "$recovery_failed" -ne 0 ]]; then
    echo "installation recovery failed closed; transaction data retained at $TRANSACTION_ROOT" >&2
    [[ "$status" -ne 0 ]] || status=70
  fi
  exit "$status"
}

validate_cli "$@"
if [[ "$ACTION" == "--validate-official" ]]; then
  [[ -x "$OFFICIAL_VALIDATOR" ]] || die "missing official input validator: $OFFICIAL_VALIDATOR"
  exec "$OFFICIAL_VALIDATOR" "$SOURCE_BUNDLE" "$RC_POINTER"
fi
validate_test_overrides
if [[ "$ACTION" == "--install" ]]; then
  if [[ "$TEST_MODE" == "1" ]]; then
    [[ -n "$SOURCE_BUNDLE" ]] || die "test install requires an explicit fixture bundle"
    validate_bundle "$SOURCE_BUNDLE"
  else
    resolve_and_validate_official_source
  fi
fi

trap cleanup EXIT
prepare_directories
validate_state_artifacts
acquire_lock
if [[ -e "$TRANSACTION_JOURNAL" ]]; then load_journal >/dev/null 2>&1 || true; fi
acquire_runtime_lock
test_host_event "runtime-lock-acquired"
stop_running_host || die "unable to drain input method host while holding runtime lock" 70
if [[ ! -e "$TRANSACTION_JOURNAL" ]]; then migrate_legacy_state || die "legacy installer state migration failed closed" 70; fi
if recover_durable_transaction; then
  :
else
  recovery_status=$?
  exit "$recovery_status"
fi
TRANSACTION_ACTION="$REQUESTED_ACTION"
backup_pair_is_valid || die "rollback backup state is inconsistent; refusing transaction" 70
current_pair_is_valid || die "current installation checksum state is inconsistent; refusing transaction" 70

case "$ACTION" in
  --recover)
    echo "installer_recovery=clean"
    ;;
  --install)
    if [[ "$TEST_MODE" == "1" ]]; then
      source_checksum="$(bundle_checksum "$SOURCE_BUNDLE")"
    else
      source_checksum="$OFFICIAL_SOURCE_CHECKSUM"
      [[ "$(bundle_checksum "$SOURCE_BUNDLE")" == "$source_checksum" ]] \
        || die "frozen RC artifact changed before staging" 3
    fi
    stage_bundle "$SOURCE_BUNDLE" "$INSTALL_DIR" "Agent Input Method.app" "$source_checksum"
    install_stage="$STAGED_BUNDLE"
    install_stage_root="$STAGED_ROOT"
    STAGE_ROOT_A="$install_stage_root"

    original=""
    if resolve_current; then
      original="$RESOLVED_CURRENT"
      validate_bundle "$original"
      original_checksum="$(bundle_checksum "$original")"
      enforce_official_upgrade_policy "$original" "$SOURCE_BUNDLE"
      stage_bundle "$original" "$STATE_DIR" "previous.inputmethod" "$original_checksum"
      TRANSACTION_BACKUP_STAGE="$STAGED_BUNDLE"
      TRANSACTION_BACKUP_STAGE_ROOT="$STAGED_ROOT"
      STAGE_ROOT_B="$TRANSACTION_BACKUP_STAGE_ROOT"
      previous_checksum="$(bundle_checksum "$TRANSACTION_BACKUP_STAGE")"
    else
      current_status=$?
      [[ "$current_status" -eq 1 ]] || exit "$current_status"
      original_checksum=""
    fi

    stop_running_host
    begin_replacement "$original" "$install_stage" "$install_stage_root" "$original_checksum" "$source_checksum"
    validate_bundle_checksum "$DESTINATION" "$source_checksum"
    register_bundle "$DESTINATION"
    test_fail "after-register"
    refresh_services "$DESTINATION"
    write_checksum "$source_checksum" "Agent Input Method.app" "$CURRENT_CHECKSUM"
    if [[ -n "$original" ]]; then
      commit_previous_backup "$TRANSACTION_BACKUP_STAGE" "$TRANSACTION_BACKUP_STAGE_ROOT" "$previous_checksum"
      test_fail "after-backup"
    fi
    commit_transaction
    # After the transaction is durable: a bundle that installed but never
    # re-enabled is the silent-death mode — verify loudly instead.
    ensure_input_source_enabled "$DESTINATION"
    echo "installed: $DESTINATION"
    ;;
  --rollback)
    validate_bundle "$BACKUP"
    [[ -f "$PREVIOUS_CHECKSUM" && ! -L "$PREVIOUS_CHECKSUM" ]] || die "missing rollback checksum: $PREVIOUS_CHECKSUM"
    expected_previous="$(awk 'NR == 1 { print $1 }' "$PREVIOUS_CHECKSUM")"
    actual_previous="$(bundle_checksum "$BACKUP")"
    [[ "$expected_previous" == "$actual_previous" ]] || die "rollback checksum mismatch"

    original=""
    if resolve_current; then
      original="$RESOLVED_CURRENT"
      validate_bundle "$original"
      original_checksum="$(bundle_checksum "$original")"
    else
      current_status=$?
      [[ "$current_status" -eq 1 ]] || exit "$current_status"
      original_checksum=""
    fi
    stage_bundle "$BACKUP" "$INSTALL_DIR" "Agent Input Method.app" "$actual_previous"
    rollback_stage="$STAGED_BUNDLE"
    rollback_stage_root="$STAGED_ROOT"
    STAGE_ROOT_A="$rollback_stage_root"

    stop_running_host
    begin_replacement "$original" "$rollback_stage" "$rollback_stage_root" "$original_checksum" "$actual_previous"
    validate_bundle_checksum "$DESTINATION" "$actual_previous"
    register_bundle "$DESTINATION"
    test_fail "after-register"
    refresh_services "$DESTINATION"
    write_checksum "$actual_previous" "Agent Input Method.app" "$CURRENT_CHECKSUM"
    commit_transaction "$FAILED_CURRENT"
    ensure_input_source_enabled "$DESTINATION"
    echo "rolled back: $DESTINATION"
    ;;
  --uninstall)
    original=""
    if resolve_current; then
      original="$RESOLVED_CURRENT"
    else
      current_status=$?
      if [[ "$current_status" -eq 1 ]]; then echo "already uninstalled: $DESTINATION"; exit 0; fi
      exit "$current_status"
    fi
    validate_bundle "$original"
    original_checksum="$(bundle_checksum "$original")"
    stage_bundle "$original" "$STATE_DIR" "previous.inputmethod" "$original_checksum"
    TRANSACTION_BACKUP_STAGE="$STAGED_BUNDLE"
    TRANSACTION_BACKUP_STAGE_ROOT="$STAGED_ROOT"
    STAGE_ROOT_B="$TRANSACTION_BACKUP_STAGE_ROOT"
    previous_checksum="$(bundle_checksum "$TRANSACTION_BACKUP_STAGE")"

    stop_running_host
    begin_removal "$original" "$original_checksum"
    commit_previous_backup "$TRANSACTION_BACKUP_STAGE" "$TRANSACTION_BACKUP_STAGE_ROOT" "$previous_checksum"
    test_fail "after-backup"
    rm -f "$CURRENT_CHECKSUM"
    refresh_services ""
    commit_transaction
    echo "uninstalled: $DESTINATION"
    ;;
  *)
    die "usage: $0 [--install [bundle]|--rollback|--uninstall|--recover]"
    ;;
esac
