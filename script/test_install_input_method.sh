#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE_BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
INSTALLER="$ROOT_DIR/script/install_input_method.sh"
TEMP_ROOT="$(mktemp -d /tmp/agent-ime-installer-test.XXXXXX)"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT

INSTALL_DIR="$TEMP_ROOT/Input Methods"
STATE_DIR="$TEMP_ROOT/InstallState"
DESTINATION="$INSTALL_DIR/Agent Input Method.app"
LEGACY="$INSTALL_DIR/Agent Input Method.inputmethod"
BACKUP="$STATE_DIR/previous.inputmethod"
PREVIOUS_CHECKSUM="$STATE_DIR/previous.bundle.sha256"
CURRENT_CHECKSUM="$STATE_DIR/current.bundle.sha256"
REGISTER_LOG="$TEMP_ROOT/register.log"
REGISTER_FAIL_FLAG="$TEMP_ROOT/register-failed-once"
REGISTER_OK="$TEMP_ROOT/register-ok.sh"
REGISTER_FAIL_ONCE="$TEMP_ROOT/register-fail-once.sh"
PROCESS_HELPER="$TEMP_ROOT/process-helper.sh"
PROCESS_STATE="$TEMP_ROOT/owned-process-active"
PROCESS_UNRESOLVED_STATE="$TEMP_ROOT/unresolved-process-active"
PROCESS_SIGNAL_LOG="$TEMP_ROOT/process-signal.log"
PROCESS_EVENT_LOG="$TEMP_ROOT/process-event.log"
PROCESS_RESTART_MODE=""
PROCESS_OLD_RESTART_STATE="$TEMP_ROOT/old-restart-active"
PROCESS_NEW_RESTART_STATE="$TEMP_ROOT/new-restart-active"
UNRELATED_EXECUTABLE="$TEMP_ROOT/unrelated/AgentInputMethod"
RUNTIME_DIR="$STATE_DIR/test-runtime"
RUNTIME_MARKER="$RUNTIME_DIR/running.marker"
RUNTIME_LOCK_HELPER="$TEMP_ROOT/runtime-lock-helper"
NEW_BUNDLE="$TEMP_ROOT/new.inputmethod"
OLD_BUNDLE="$TEMP_ROOT/old.inputmethod"

printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "%s\n" "$2" >> "$AIM_TEST_REGISTER_LOG"' \
  'exit 0' > "$REGISTER_OK"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "%s\n" "$2" >> "$AIM_TEST_REGISTER_LOG"' \
  'if [[ ! -e "$AIM_TEST_REGISTER_FAIL_FLAG" ]]; then' \
  '  : > "$AIM_TEST_REGISTER_FAIL_FLAG"' \
  '  exit 9' \
  'fi' \
  'exit 0' > "$REGISTER_FAIL_ONCE"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  'case "$1" in' \
  '  list)' \
  '    [[ ! -e "$AIM_TEST_PROCESS_STATE" ]] || printf "41001\n"' \
  '    printf "41002\n"' \
  '    [[ ! -e "$AIM_TEST_PROCESS_UNRESOLVED_STATE" ]] || printf "41003\n"' \
  '    [[ ! -e "$AIM_TEST_PROCESS_OLD_RESTART_STATE" ]] || printf "42002\n"' \
  '    [[ ! -e "$AIM_TEST_PROCESS_NEW_RESTART_STATE" ]] || printf "42003\n"' \
  '    ;;' \
  '  executable)' \
  '    case "$2" in' \
  '      41001) printf "%s\n" "$AIM_TEST_TARGET_EXECUTABLE" ;;' \
  '      41002) printf "%s\n" "$AIM_TEST_UNRELATED_EXECUTABLE" ;;' \
  '      41003) exit 2 ;;' \
  '      42002)' \
  '        find "$AIM_TEST_INSTALL_DIR" -path "*/.agent-ime-transaction.*/original.inputmethod/Contents/MacOS/AgentInputMethod" -print -quit' \
  '        ;;' \
  '      42003) printf "%s\n" "$AIM_TEST_TARGET_EXECUTABLE" ;;' \
  '      *) exit 2 ;;' \
  '    esac' \
  '    ;;' \
  '  alive)' \
  '    case "$2" in' \
  '      41001) [[ -e "$AIM_TEST_PROCESS_STATE" ]] ;;' \
  '      41002) exit 0 ;;' \
  '      41003) [[ -e "$AIM_TEST_PROCESS_UNRESOLVED_STATE" ]] ;;' \
  '      42002) [[ -e "$AIM_TEST_PROCESS_OLD_RESTART_STATE" ]] ;;' \
  '      42003) [[ -e "$AIM_TEST_PROCESS_NEW_RESTART_STATE" ]] ;;' \
  '      *) exit 1 ;;' \
  '    esac' \
  '    ;;' \
  '  signal)' \
  '    printf "%s %s\n" "$2" "$3" >> "$AIM_TEST_PROCESS_SIGNAL_LOG"' \
  '    case "$3" in' \
  '      41001) rm -f "$AIM_TEST_PROCESS_STATE" ;;' \
  '      42002) rm -f "$AIM_TEST_PROCESS_OLD_RESTART_STATE" ;;' \
  '      42003) rm -f "$AIM_TEST_PROCESS_NEW_RESTART_STATE" ;;' \
  '      *) exit 91 ;;' \
  '    esac' \
  '    ;;' \
  '  event)' \
  '    printf "%s\n" "$2" >> "$AIM_TEST_PROCESS_EVENT_LOG"' \
  '    if [[ "$AIM_TEST_PROCESS_RESTART_MODE" == "1" ]]; then' \
  '      case "$2" in' \
  '        old_moved) : > "$AIM_TEST_PROCESS_OLD_RESTART_STATE" ;;' \
  '        new_moved) : > "$AIM_TEST_PROCESS_NEW_RESTART_STATE" ;;' \
  '      esac' \
  '    fi' \
  '    ;;' \
  '  *) exit 2 ;;' \
  'esac' > "$PROCESS_HELPER"
chmod 700 "$REGISTER_OK" "$REGISTER_FAIL_ONCE" "$PROCESS_HELPER"
mkdir -p "$TEMP_ROOT/swift-module-cache"
xcrun swiftc -D AIM_INPUT_SOURCE_CTL_SELECTION_TEST "$ROOT_DIR/script/input_source_ctl.swift" \
  -framework Carbon -module-cache-path "$TEMP_ROOT/swift-module-cache" \
  -o "$TEMP_ROOT/input-source-inventory-tests"
"$TEMP_ROOT/input-source-inventory-tests"
xcrun swiftc "$ROOT_DIR/script/input_source_ctl.swift" -framework Carbon \
  -module-cache-path "$TEMP_ROOT/swift-module-cache" -o "$RUNTIME_LOCK_HELPER"

wait_for_runtime_lock_ready() {
  local ready_file="$1" error_file="$2" process_id="$3" label="$4"
  local attempt
  for ((attempt = 0; attempt < 100; attempt++)); do
    if rg -qx 'runtime_lock=acquired' "$ready_file"; then
      return 0
    fi
    if ! kill -0 "$process_id" 2>/dev/null; then
      cat "$error_file" >&2 || true
      echo "$label runtime lock helper exited before acquiring the lock" >&2
      return 1
    fi
    sleep 0.05
  done
  cat "$error_file" >&2 || true
  echo "$label runtime lock helper did not acquire the lock within 5 seconds" >&2
  return 1
}

# A newly launched host uses this same persistent lock and must block until the
# installer generation releases it.
LOCK_PROTOCOL_DIR="$TEMP_ROOT/runtime-lock-protocol"
HOLDER_CONTROL="$TEMP_ROOT/holder-control"
CONTENDER_CONTROL="$TEMP_ROOT/contender-control"
HOLDER_READY="$TEMP_ROOT/holder-ready.txt"
CONTENDER_READY="$TEMP_ROOT/contender-ready.txt"
mkfifo "$HOLDER_CONTROL" "$CONTENDER_CONTROL"
"$RUNTIME_LOCK_HELPER" runtime-lock "$LOCK_PROTOCOL_DIR" \
  < "$HOLDER_CONTROL" > "$HOLDER_READY" 2> "$TEMP_ROOT/holder-error.txt" &
HOLDER_PID=$!
exec 7> "$HOLDER_CONTROL"
wait_for_runtime_lock_ready "$HOLDER_READY" "$TEMP_ROOT/holder-error.txt" "$HOLDER_PID" holder
"$RUNTIME_LOCK_HELPER" runtime-lock "$LOCK_PROTOCOL_DIR" \
  7>&- < "$CONTENDER_CONTROL" > "$CONTENDER_READY" 2> "$TEMP_ROOT/contender-error.txt" &
CONTENDER_PID=$!
exec 8> "$CONTENDER_CONTROL"
sleep 0.2
kill -0 "$CONTENDER_PID"
[[ ! -s "$CONTENDER_READY" ]]
exec 7>&-
wait "$HOLDER_PID"
wait_for_runtime_lock_ready "$CONTENDER_READY" "$TEMP_ROOT/contender-error.txt" "$CONTENDER_PID" contender
exec 8>&-
wait "$CONTENDER_PID"
printf 'runtime_lock_blocking_test=PASS\n'

SOURCE_CHECKSUM="$($ROOT_DIR/script/artifact_checksum.sh "$SOURCE_BUNDLE")"
ditto "$SOURCE_BUNDLE" "$NEW_BUNDLE"
chmod -R u+w "$NEW_BUNDLE"
codesign --force --deep --sign - --timestamp=none "$NEW_BUNDLE" >/dev/null
chmod -R a-w "$NEW_BUNDLE"
ditto "$NEW_BUNDLE" "$OLD_BUNDLE"
chmod -R u+w "$OLD_BUNDLE"
printf 'signed installer fixture\n' > "$OLD_BUNDLE/Contents/Resources/installer-old-fixture.txt"
codesign --force --deep --sign - --timestamp=none "$OLD_BUNDLE" >/dev/null
chmod -R a-w "$OLD_BUNDLE"

checksum() { "$ROOT_DIR/script/artifact_checksum.sh" "$1"; }
NEW_CHECKSUM="$(checksum "$NEW_BUNDLE")"
OLD_CHECKSUM="$(checksum "$OLD_BUNDLE")"
[[ "$NEW_CHECKSUM" != "$OLD_CHECKSUM" ]]

reset_sandbox() {
  chmod -R u+w "$INSTALL_DIR" "$STATE_DIR" 2>/dev/null || true
  rm -rf "$INSTALL_DIR" "$STATE_DIR"
  mkdir -p "$INSTALL_DIR" "$STATE_DIR"
  rm -f "$REGISTER_LOG" "$REGISTER_FAIL_FLAG" "$PROCESS_STATE" \
    "$PROCESS_UNRESOLVED_STATE" "$PROCESS_SIGNAL_LOG" "$PROCESS_EVENT_LOG" \
    "$PROCESS_OLD_RESTART_STATE" "$PROCESS_NEW_RESTART_STATE"
}

invoke() {
  local helper="$1" fail_at="$2"
  shift 2
  AIM_INSTALL_TEST_MODE=1 \
  AIM_INSTALL_DIR="$INSTALL_DIR" \
  AIM_INSTALL_STATE_DIR="$STATE_DIR" \
  AIM_INSTALL_SKIP_SERVICE_REFRESH=1 \
  AIM_INSTALL_SKIP_HOST_STOP="${INSTALL_SKIP_HOST_STOP:-1}" \
  AIM_INSTALL_REGISTER_HELPER="$helper" \
  AIM_INSTALL_PROCESS_HELPER="${INSTALL_PROCESS_HELPER:-}" \
  AIM_INSTALL_RUNTIME_DIR="$RUNTIME_DIR" \
  AIM_INSTALL_RUNTIME_LOCK_HELPER="$RUNTIME_LOCK_HELPER" \
  AIM_INSTALL_FAIL_AT="$fail_at" \
  AIM_INSTALL_CRASH_AT="${INSTALL_CRASH_AT:-}" \
  AIM_INSTALL_FAULT_LEASE_TOKEN="${INSTALL_FAULT_LEASE_TOKEN:-}" \
  AIM_TEST_REGISTER_LOG="$REGISTER_LOG" \
  AIM_TEST_REGISTER_FAIL_FLAG="$REGISTER_FAIL_FLAG" \
  AIM_TEST_PROCESS_STATE="$PROCESS_STATE" \
  AIM_TEST_PROCESS_UNRESOLVED_STATE="$PROCESS_UNRESOLVED_STATE" \
  AIM_TEST_PROCESS_SIGNAL_LOG="$PROCESS_SIGNAL_LOG" \
  AIM_TEST_PROCESS_EVENT_LOG="$PROCESS_EVENT_LOG" \
  AIM_TEST_PROCESS_RESTART_MODE="${PROCESS_RESTART_MODE:-}" \
  AIM_TEST_PROCESS_OLD_RESTART_STATE="$PROCESS_OLD_RESTART_STATE" \
  AIM_TEST_PROCESS_NEW_RESTART_STATE="$PROCESS_NEW_RESTART_STATE" \
  AIM_TEST_INSTALL_DIR="$INSTALL_DIR" \
  AIM_TEST_TARGET_EXECUTABLE="$DESTINATION/Contents/MacOS/AgentInputMethod" \
  AIM_TEST_UNRELATED_EXECUTABLE="$UNRELATED_EXECUTABLE" \
  "$INSTALLER" "$@"
}

run_ok() { invoke "$REGISTER_OK" "" "$@"; }

expect_status() {
  local expected="$1" helper="$2" fail_at="$3" label="$4"
  shift 4
  local output="$TEMP_ROOT/expected-$label.txt" status
  set +e
  invoke "$helper" "$fail_at" "$@" > "$output" 2>&1
  status=$?
  set -e
  if [[ "$status" -ne "$expected" ]]; then
    cat "$output" >&2
    echo "unexpected status for $label: expected=$expected actual=$status" >&2
    exit 1
  fi
}

assert_installed() {
  local expected="$1"
  [[ -d "$DESTINATION" ]]
  [[ "$(checksum "$DESTINATION")" == "$expected" ]]
  [[ -f "$CURRENT_CHECKSUM" && ! -L "$CURRENT_CHECKSUM" ]]
  [[ "$(awk 'NR == 1 { print $1; exit }' "$CURRENT_CHECKSUM")" == "$expected" ]]
  codesign --verify --deep --strict "$DESTINATION"
}

assert_uninstalled() {
  [[ ! -e "$DESTINATION" && ! -L "$DESTINATION" ]]
  [[ ! -e "$LEGACY" && ! -L "$LEGACY" ]]
  [[ ! -e "$CURRENT_CHECKSUM" && ! -L "$CURRENT_CHECKSUM" ]]
}

assert_backup_pair() {
  local expected="$1"
  [[ -d "$BACKUP" && ! -L "$BACKUP" && -f "$PREVIOUS_CHECKSUM" && ! -L "$PREVIOUS_CHECKSUM" ]]
  [[ "$(checksum "$BACKUP")" == "$expected" ]]
  [[ "$(awk 'NR == 1 { print $1; exit }' "$PREVIOUS_CHECKSUM")" == "$expected" ]]
}

assert_no_backup_pair() {
  [[ ! -e "$BACKUP" && ! -L "$BACKUP" ]]
  [[ ! -e "$PREVIOUS_CHECKSUM" && ! -L "$PREVIOUS_CHECKSUM" ]]
}

assert_no_debris() {
  local debris
  debris="$(find "$INSTALL_DIR" "$STATE_DIR" -maxdepth 1 \
    \( -name '.agent-ime-*' -o -name '.checksum.*' -o -name '.transaction.journal.*' \
       -o -name '.stale-install-lock.*' -o -name '.runtime-lock-session.*' \) -print -quit 2>/dev/null || true)"
  [[ -z "$debris" ]] || { echo "installer debris remained: $debris" >&2; exit 1; }
  [[ ! -e "$STATE_DIR/install.lock" && ! -L "$STATE_DIR/install.lock" ]]
  [[ ! -e "$STATE_DIR/transaction.journal" && ! -L "$STATE_DIR/transaction.journal" ]]
  [[ ! -e "$STATE_DIR/recovery-required" && ! -L "$STATE_DIR/recovery-required" ]]
}

reset_fail_once() { rm -f "$REGISTER_LOG" "$REGISTER_FAIL_FLAG"; }

REJECTED_INSTALL="$TEMP_ROOT/rejected-install"
REJECTED_STATE="$TEMP_ROOT/rejected-state"
set +e
AIM_INSTALL_TEST_MODE=1 AIM_INSTALL_DIR="$REJECTED_INSTALL" AIM_INSTALL_STATE_DIR="$REJECTED_STATE" \
  AIM_INSTALL_RUNTIME_DIR="$TEMP_ROOT/rejected-runtime" AIM_INSTALL_RUNTIME_LOCK_HELPER="$RUNTIME_LOCK_HELPER" \
  AIM_INSTALL_SKIP_HOST_STOP=1 AIM_INSTALL_REGISTER_HELPER="$REGISTER_OK" \
  "$INSTALLER" --install "$NEW_BUNDLE" > "$TEMP_ROOT/rejected-missing-skip.txt" 2>&1
REJECTED_STATUS=$?
set -e
[[ "$REJECTED_STATUS" -eq 2 ]]
[[ ! -e "$REJECTED_INSTALL" && ! -L "$REJECTED_INSTALL" && ! -e "$REJECTED_STATE" && ! -L "$REJECTED_STATE" ]]

set +e
AIM_INSTALL_TEST_MODE=1 AIM_INSTALL_DIR="$REJECTED_INSTALL" AIM_INSTALL_STATE_DIR="$REJECTED_STATE" \
  AIM_INSTALL_RUNTIME_DIR="$TEMP_ROOT/rejected-runtime" AIM_INSTALL_RUNTIME_LOCK_HELPER="$RUNTIME_LOCK_HELPER" \
  AIM_INSTALL_SKIP_SERVICE_REFRESH=1 AIM_INSTALL_SKIP_HOST_STOP=1 AIM_INSTALL_REGISTER_HELPER="$INSTALLER" \
  "$INSTALLER" --install "$NEW_BUNDLE" > "$TEMP_ROOT/rejected-helper-outside-tmp.txt" 2>&1
REJECTED_STATUS=$?
set -e
[[ "$REJECTED_STATUS" -eq 2 ]]
[[ ! -e "$REJECTED_INSTALL" && ! -L "$REJECTED_INSTALL" && ! -e "$REJECTED_STATE" && ! -L "$REJECTED_STATE" ]]

set +e
AIM_INSTALL_DIR="$REJECTED_INSTALL" AIM_INSTALL_STATE_DIR="$REJECTED_STATE" \
  "$INSTALLER" --recover > "$TEMP_ROOT/rejected-override-without-test-mode.txt" 2>&1
REJECTED_STATUS=$?
set -e
[[ "$REJECTED_STATUS" -eq 2 ]]
[[ ! -e "$REJECTED_INSTALL" && ! -L "$REJECTED_INSTALL" && ! -e "$REJECTED_STATE" && ! -L "$REJECTED_STATE" ]]

reset_sandbox
HARDLINK_BUNDLE="$TEMP_ROOT/hardlink.inputmethod"
HARDLINK_SENTINEL="$TEMP_ROOT/hardlink-sentinel"
ditto "$NEW_BUNDLE" "$HARDLINK_BUNDLE"
chmod u+w "$HARDLINK_BUNDLE/Contents/Resources"
printf 'hardlink-sentinel-do-not-modify\n' > "$HARDLINK_SENTINEL"
chmod 444 "$HARDLINK_SENTINEL"
ln "$HARDLINK_SENTINEL" "$HARDLINK_BUNDLE/Contents/Resources/hardlink-sentinel"
chmod a-w "$HARDLINK_BUNDLE/Contents/Resources"
HARDLINK_SENTINEL_MODE="$(stat -f '%Lp' "$HARDLINK_SENTINEL")"
expect_status 2 "$REGISTER_OK" "" hardlink-bundle --install "$HARDLINK_BUNDLE"
if ! rg -q 'refusing hard-linked input method payload' "$TEMP_ROOT/expected-hardlink-bundle.txt"; then
  cat "$TEMP_ROOT/expected-hardlink-bundle.txt" >&2
  echo "hard-linked bundle rejection did not report the expected reason" >&2
  exit 1
fi
[[ "$(< "$HARDLINK_SENTINEL")" == 'hardlink-sentinel-do-not-modify' ]]
[[ "$(stat -f '%Lp' "$HARDLINK_SENTINEL")" == "$HARDLINK_SENTINEL_MODE" ]]

reset_sandbox
STATE_HARDLINK_SENTINEL="$TEMP_ROOT/state-hardlink-sentinel"
printf 'state-hardlink-sentinel-do-not-modify\n' > "$STATE_HARDLINK_SENTINEL"
chmod 444 "$STATE_HARDLINK_SENTINEL"
ln "$STATE_HARDLINK_SENTINEL" "$STATE_DIR/recovery-required"
STATE_HARDLINK_SENTINEL_MODE="$(stat -f '%Lp' "$STATE_HARDLINK_SENTINEL")"
expect_status 2 "$REGISTER_OK" "" hardlink-state --recover
if ! rg -q 'refusing hard-linked installer state artifact' "$TEMP_ROOT/expected-hardlink-state.txt"; then
  cat "$TEMP_ROOT/expected-hardlink-state.txt" >&2
  echo "hard-linked state rejection did not report the expected reason" >&2
  exit 1
fi
[[ "$(< "$STATE_HARDLINK_SENTINEL")" == 'state-hardlink-sentinel-do-not-modify' ]]
[[ "$(stat -f '%Lp' "$STATE_HARDLINK_SENTINEL")" == "$STATE_HARDLINK_SENTINEL_MODE" ]]

reset_sandbox
expect_status 9 "$REGISTER_FAIL_ONCE" "" first-install-register --install "$NEW_BUNDLE"
assert_uninstalled
[[ "$(wc -l < "$REGISTER_LOG" | tr -d ' ')" == "1" ]]
assert_no_debris

expect_status 72 "$REGISTER_OK" after-new-moved first-install-after-new --install "$NEW_BUNDLE"
assert_uninstalled
assert_no_debris

set +e
INSTALL_CRASH_AT=new_moved invoke "$REGISTER_OK" "" --install "$NEW_BUNDLE" \
  > "$TEMP_ROOT/crash-first-install-recovery-seed.txt" 2>&1
CRASH_STATUS=$?
set -e
[[ "$CRASH_STATUS" -eq 137 ]]
set +e
INSTALL_CRASH_AT=recovery_new_displaced invoke "$REGISTER_OK" "" --recover \
  > "$TEMP_ROOT/crash-first-install-recovery-displaced.txt" 2>&1
RECOVERY_CRASH_STATUS=$?
set -e
[[ "$RECOVERY_CRASH_STATUS" -eq 137 ]]
run_ok --recover
assert_uninstalled
assert_no_debris

run_ok --install "$OLD_BUNDLE"
assert_installed "$OLD_CHECKSUM"

for phase in prepared old_moved new_moved; do
  set +e
  INSTALL_CRASH_AT="$phase" invoke "$REGISTER_OK" "" --install "$NEW_BUNDLE" > "$TEMP_ROOT/crash-install-$phase.txt" 2>&1
  CRASH_STATUS=$?
  set -e
  [[ "$CRASH_STATUS" -eq 137 ]]
  run_ok --recover
  assert_installed "$OLD_CHECKSUM"
  assert_no_debris
done

set +e
INSTALL_CRASH_AT=new_moved invoke "$REGISTER_OK" "" --install "$NEW_BUNDLE" \
  > "$TEMP_ROOT/crash-recovery-register-failure-seed.txt" 2>&1
CRASH_STATUS=$?
set -e
[[ "$CRASH_STATUS" -eq 137 ]]
reset_fail_once
expect_status 70 "$REGISTER_FAIL_ONCE" "" recovery-register-failure --recover
assert_installed "$OLD_CHECKSUM"
[[ -e "$STATE_DIR/recovery-required" && -e "$STATE_DIR/transaction.journal" && -e "$STATE_DIR/install.lock" ]]
run_ok --recover
assert_installed "$OLD_CHECKSUM"
assert_no_debris

for recovery_point in recovery_new_displaced recovery_original_restored \
  recovery_registered recovery_current_checksum_written; do
  set +e
  INSTALL_CRASH_AT=new_moved invoke "$REGISTER_OK" "" --install "$NEW_BUNDLE" \
    > "$TEMP_ROOT/crash-recovery-seed-$recovery_point.txt" 2>&1
  CRASH_STATUS=$?
  set -e
  [[ "$CRASH_STATUS" -eq 137 ]]
  set +e
  INSTALL_CRASH_AT="$recovery_point" invoke "$REGISTER_OK" "" --recover \
    > "$TEMP_ROOT/crash-$recovery_point.txt" 2>&1
  RECOVERY_CRASH_STATUS=$?
  set -e
  [[ "$RECOVERY_CRASH_STATUS" -eq 137 ]]
  run_ok --recover
  assert_installed "$OLD_CHECKSUM"
  assert_no_debris
done

for point in backup_prepared backup_new_moved backup_new_checksum_moved; do
  set +e
  INSTALL_CRASH_AT="$point" invoke "$REGISTER_OK" "" --install "$NEW_BUNDLE" \
    > "$TEMP_ROOT/crash-no-prior-backup-$point.txt" 2>&1
  CRASH_STATUS=$?
  set -e
  [[ "$CRASH_STATUS" -eq 137 ]]
  run_ok --recover
  assert_installed "$OLD_CHECKSUM"
  assert_no_backup_pair
  assert_no_debris
done

# The installer never owns running.marker. While holding the shared runtime
# lock it drains the original generation, then drains both a moved-old
# generation and a new destination generation before deleting transaction
# paths and releasing the lock.
mkdir -p "$(dirname "$UNRELATED_EXECUTABLE")"
printf 'unrelated same-name process fixture\n' > "$UNRELATED_EXECUTABLE"
mkdir -p "$RUNTIME_DIR"
chmod 700 "$RUNTIME_DIR"
printf 'unclean-generation-must-survive-installer\n' > "$RUNTIME_MARKER"
chmod 600 "$RUNTIME_MARKER"
: > "$PROCESS_STATE"
: > "$PROCESS_SIGNAL_LOG"
: > "$PROCESS_EVENT_LOG"
PROCESS_RESTART_MODE=1 INSTALL_PROCESS_HELPER="$PROCESS_HELPER" INSTALL_SKIP_HOST_STOP=0 \
  run_ok --install "$NEW_BUNDLE"
PROCESS_RESTART_MODE=""
assert_installed "$NEW_CHECKSUM"
[[ "$(< "$RUNTIME_MARKER")" == 'unclean-generation-must-survive-installer' ]]
rg -qx 'TERM 41001' "$PROCESS_SIGNAL_LOG"
rg -qx 'TERM 42002' "$PROCESS_SIGNAL_LOG"
rg -qx 'TERM 42003' "$PROCESS_SIGNAL_LOG"
rg -qx 'old_moved' "$PROCESS_EVENT_LOG"
rg -qx 'new_moved' "$PROCESS_EVENT_LOG"
[[ ! -e "$PROCESS_OLD_RESTART_STATE" && ! -e "$PROCESS_NEW_RESTART_STATE" ]]
run_ok --rollback
assert_installed "$OLD_CHECKSUM"
[[ "$(< "$RUNTIME_MARKER")" == 'unclean-generation-must-survive-installer' ]]
assert_no_debris

mkdir -p "$(dirname "$UNRELATED_EXECUTABLE")"
printf 'unrelated same-name process fixture\n' > "$UNRELATED_EXECUTABLE"
: > "$PROCESS_STATE"
: > "$PROCESS_SIGNAL_LOG"
INSTALL_PROCESS_HELPER="$PROCESS_HELPER" INSTALL_SKIP_HOST_STOP=0 run_ok --uninstall
assert_uninstalled
rg -qx 'TERM 41001' "$PROCESS_SIGNAL_LOG"
! rg -q '41002' "$PROCESS_SIGNAL_LOG"
run_ok --rollback
assert_installed "$OLD_CHECKSUM"
assert_no_debris
: > "$PROCESS_UNRESOLVED_STATE"
: > "$PROCESS_SIGNAL_LOG"
INSTALL_PROCESS_HELPER="$PROCESS_HELPER" INSTALL_SKIP_HOST_STOP=0 \
  expect_status 70 "$REGISTER_OK" "" unresolvable-same-name-host --uninstall
assert_installed "$OLD_CHECKSUM"
[[ ! -s "$PROCESS_SIGNAL_LOG" ]]
rm -f "$PROCESS_UNRESOLVED_STATE"
assert_no_debris

for old_failure in corrupt symlink; do
  set +e
  INSTALL_CRASH_AT=new_moved invoke "$REGISTER_OK" "" --install "$NEW_BUNDLE" \
    > "$TEMP_ROOT/crash-invalid-old-$old_failure.txt" 2>&1
  CRASH_STATUS=$?
  set -e
  [[ "$CRASH_STATUS" -eq 137 ]]
  TRANSACTION_ROOT="$(awk -F= '$1 == "root" { print $2; exit }' "$STATE_DIR/transaction.journal")"
  TRANSACTION_OLD="$TRANSACTION_ROOT/original.inputmethod"
  [[ -d "$TRANSACTION_OLD" && ! -L "$TRANSACTION_OLD" ]]
  if [[ "$old_failure" == "corrupt" ]]; then
    chmod u+w "$TRANSACTION_OLD/Contents/Info.plist"
    printf 'corrupt-recovery-source\n' >> "$TRANSACTION_OLD/Contents/Info.plist"
  else
    chmod -R u+w "$TRANSACTION_OLD"
    rm -rf "$TRANSACTION_OLD"
    ln -s "$OLD_BUNDLE" "$TRANSACTION_OLD"
  fi
  expect_status 70 "$REGISTER_OK" "" "recovery-invalid-old-$old_failure" --recover
  [[ -d "$DESTINATION" && ! -L "$DESTINATION" ]]
  [[ "$(checksum "$DESTINATION")" == "$NEW_CHECKSUM" ]]
  [[ -e "$STATE_DIR/recovery-required" && -e "$STATE_DIR/install.lock" ]]
  if [[ "$old_failure" == "symlink" ]]; then
    [[ -L "$TRANSACTION_OLD" ]]
    rm -f "$TRANSACTION_OLD"
  else
    [[ -d "$TRANSACTION_OLD" ]]
  fi
  reset_sandbox
  run_ok --install "$OLD_BUNDLE"
  assert_installed "$OLD_CHECKSUM"
done

set +e
INSTALL_CRASH_AT=committed invoke "$REGISTER_OK" "" --install "$NEW_BUNDLE" > "$TEMP_ROOT/crash-install-committed.txt" 2>&1
CRASH_STATUS=$?
set -e
[[ "$CRASH_STATUS" -eq 137 ]]
run_ok --recover
assert_installed "$NEW_CHECKSUM"
assert_no_debris
run_ok --rollback
assert_installed "$OLD_CHECKSUM"

run_ok --install "$NEW_BUNDLE"
run_ok --install "$OLD_BUNDLE"
assert_installed "$OLD_CHECKSUM"
assert_backup_pair "$NEW_CHECKSUM"
for point in backup_prepared backup_old_moved backup_checksum_moved backup_new_moved backup_new_checksum_moved; do
  set +e
  INSTALL_CRASH_AT="$point" invoke "$REGISTER_OK" "" --install "$NEW_BUNDLE" > "$TEMP_ROOT/crash-$point.txt" 2>&1
  CRASH_STATUS=$?
  set -e
  [[ "$CRASH_STATUS" -eq 137 ]]
  run_ok --recover
  assert_installed "$OLD_CHECKSUM"
  assert_backup_pair "$NEW_CHECKSUM"
  assert_no_debris
done

for recovery_point in recovery_backup_restored recovery_checksum_restored; do
  set +e
  INSTALL_CRASH_AT=backup_checksum_moved invoke "$REGISTER_OK" "" --install "$NEW_BUNDLE" > "$TEMP_ROOT/crash-seed-$recovery_point.txt" 2>&1
  CRASH_STATUS=$?
  set -e
  [[ "$CRASH_STATUS" -eq 137 ]]
  set +e
  INSTALL_CRASH_AT="$recovery_point" invoke "$REGISTER_OK" "" --recover > "$TEMP_ROOT/crash-$recovery_point.txt" 2>&1
  RECOVERY_CRASH_STATUS=$?
  set -e
  [[ "$RECOVERY_CRASH_STATUS" -eq 137 ]]
  run_ok --recover
  assert_installed "$OLD_CHECKSUM"
  assert_backup_pair "$NEW_CHECKSUM"
  assert_no_debris
done

set +e
INSTALL_CRASH_AT=recovered invoke "$REGISTER_OK" after-new-moved --install "$NEW_BUNDLE" > "$TEMP_ROOT/crash-recovered-cleanup.txt" 2>&1
RECOVERED_CRASH_STATUS=$?
set -e
[[ "$RECOVERED_CRASH_STATUS" -eq 137 ]]
run_ok --recover
assert_installed "$OLD_CHECKSUM"
assert_no_debris

reset_fail_once
expect_status 9 "$REGISTER_FAIL_ONCE" "" overwrite-register --install "$NEW_BUNDLE"
assert_installed "$OLD_CHECKSUM"
[[ "$(wc -l < "$REGISTER_LOG" | tr -d ' ')" == "2" ]]
assert_no_debris

for point in before-old-move after-old-moved after-new-moved after-register after-backup; do
  expect_status 72 "$REGISTER_OK" "$point" "install-$point" --install "$NEW_BUNDLE"
  assert_installed "$OLD_CHECKSUM"
  assert_no_debris
done

run_ok --install "$NEW_BUNDLE"
assert_installed "$NEW_CHECKSUM"
assert_backup_pair "$OLD_CHECKSUM"

reset_fail_once
expect_status 9 "$REGISTER_FAIL_ONCE" "" rollback-register --rollback
assert_installed "$NEW_CHECKSUM"
[[ "$(wc -l < "$REGISTER_LOG" | tr -d ' ')" == "2" ]]
assert_no_debris

run_ok --rollback
assert_installed "$OLD_CHECKSUM"
[[ "$(checksum "$STATE_DIR/failed-current.inputmethod")" == "$NEW_CHECKSUM" ]]

mkdir "$STATE_DIR/install.lock"
printf '%s\n' "$$" > "$STATE_DIR/install.lock/pid"
expect_status 6 "$REGISTER_OK" "" concurrent-lock --install "$NEW_BUNDLE"
assert_installed "$OLD_CHECKSUM"
rm -rf "$STATE_DIR/install.lock"

mkdir "$STATE_DIR/install.lock"
printf '999999\n' > "$STATE_DIR/install.lock/pid"
run_ok --install "$OLD_BUNDLE"
assert_installed "$OLD_CHECKSUM"
assert_no_debris

LOCK_PID_SENTINEL="$TEMP_ROOT/lock-pid-sentinel"
printf '999999\n' > "$LOCK_PID_SENTINEL"
chmod 444 "$LOCK_PID_SENTINEL"
mkdir "$STATE_DIR/install.lock"
ln "$LOCK_PID_SENTINEL" "$STATE_DIR/install.lock/pid"
LOCK_PID_SENTINEL_MODE="$(stat -f '%Lp' "$LOCK_PID_SENTINEL")"
expect_status 6 "$REGISTER_OK" "" hardlink-lock-pid --recover
[[ "$(< "$LOCK_PID_SENTINEL")" == '999999' ]]
[[ "$(stat -f '%Lp' "$LOCK_PID_SENTINEL")" == "$LOCK_PID_SENTINEL_MODE" ]]
rm -rf "$STATE_DIR/install.lock"

FAULT_LEASE="$TEMP_ROOT/.install-fault-drill.lock"
mkdir "$FAULT_LEASE"
printf 'isolated-test-token\n' > "$FAULT_LEASE/token"
printf '%s\n' "$$" > "$FAULT_LEASE/pid"
chmod 700 "$FAULT_LEASE"
chmod 600 "$FAULT_LEASE/token" "$FAULT_LEASE/pid"
expect_status 6 "$REGISTER_OK" "" fault-lease-without-token --recover
INSTALL_FAULT_LEASE_TOKEN=isolated-test-token run_ok --recover
assert_installed "$OLD_CHECKSUM"
rm -rf "$FAULT_LEASE"

FAULT_TOKEN_SENTINEL="$TEMP_ROOT/fault-token-sentinel"
printf 'isolated-test-token\n' > "$FAULT_TOKEN_SENTINEL"
chmod 444 "$FAULT_TOKEN_SENTINEL"
mkdir "$FAULT_LEASE"
ln "$FAULT_TOKEN_SENTINEL" "$FAULT_LEASE/token"
printf '%s\n' "$$" > "$FAULT_LEASE/pid"
chmod 700 "$FAULT_LEASE"
chmod 600 "$FAULT_LEASE/pid"
FAULT_TOKEN_SENTINEL_MODE="$(stat -f '%Lp' "$FAULT_TOKEN_SENTINEL")"
expect_status 6 "$REGISTER_OK" "" hardlink-fault-token --recover
[[ "$(< "$FAULT_TOKEN_SENTINEL")" == 'isolated-test-token' ]]
[[ "$(stat -f '%Lp' "$FAULT_TOKEN_SENTINEL")" == "$FAULT_TOKEN_SENTINEL_MODE" ]]
rm -rf "$FAULT_LEASE"
set +e
INSTALL_FAULT_LEASE_TOKEN=orphan-token invoke "$REGISTER_OK" "" --recover > "$TEMP_ROOT/orphan-lease-token.txt" 2>&1
ORPHAN_TOKEN_STATUS=$?
set -e
[[ "$ORPHAN_TOKEN_STATUS" -eq 6 ]]
ln -s "$TEMP_ROOT/does-not-exist" "$FAULT_LEASE"
expect_status 6 "$REGISTER_OK" "" symlink-fault-lease --recover
rm -f "$FAULT_LEASE"

printf 'manual-recovery-fixture\n' > "$STATE_DIR/recovery-required"
expect_status 70 "$REGISTER_OK" "" recovery-marker-without-journal --recover
[[ -e "$STATE_DIR/recovery-required" && -e "$STATE_DIR/install.lock" ]]
rm -f "$STATE_DIR/recovery-required"
rm -rf "$STATE_DIR/install.lock"

for point in after-old-moved after-backup; do
  expect_status 72 "$REGISTER_OK" "$point" "uninstall-$point" --uninstall
  assert_installed "$OLD_CHECKSUM"
  assert_no_debris
done

set +e
INSTALL_CRASH_AT=old_moved invoke "$REGISTER_OK" "" --uninstall > "$TEMP_ROOT/crash-uninstall-old-moved.txt" 2>&1
CRASH_STATUS=$?
set -e
[[ "$CRASH_STATUS" -eq 137 ]]
run_ok --recover
assert_installed "$OLD_CHECKSUM"
assert_no_debris

set +e
INSTALL_CRASH_AT=committed invoke "$REGISTER_OK" "" --uninstall > "$TEMP_ROOT/crash-uninstall-committed.txt" 2>&1
CRASH_STATUS=$?
set -e
[[ "$CRASH_STATUS" -eq 137 ]]
run_ok --recover
assert_uninstalled
assert_no_debris
run_ok --rollback
assert_installed "$OLD_CHECKSUM"

run_ok --uninstall
assert_uninstalled
assert_backup_pair "$OLD_CHECKSUM"

cp "$PREVIOUS_CHECKSUM" "$TEMP_ROOT/previous.bundle.sha256.good"
printf '%064d  previous.inputmethod\n' 0 > "$PREVIOUS_CHECKSUM"
expect_status 70 "$REGISTER_OK" "" checksum-mismatch --rollback
assert_uninstalled
cp "$TEMP_ROOT/previous.bundle.sha256.good" "$PREVIOUS_CHECKSUM"

ditto "$BACKUP" "$TEMP_ROOT/backup-good.inputmethod"
chmod u+w "$BACKUP/Contents/Info.plist"
printf 'tamper\n' >> "$BACKUP/Contents/Info.plist"
expect_status 70 "$REGISTER_OK" "" backup-tamper --rollback
assert_uninstalled
chmod -R u+w "$BACKUP"
rm -rf "$BACKUP"
ditto "$TEMP_ROOT/backup-good.inputmethod" "$BACKUP"
run_ok --rollback
assert_installed "$OLD_CHECKSUM"

run_ok --uninstall
ln -s /tmp/agent-ime-does-not-exist "$DESTINATION"
expect_status 70 "$REGISTER_OK" "" broken-destination-symlink --install "$NEW_BUNDLE"
[[ -L "$DESTINATION" ]]
rm -f "$DESTINATION"
run_ok --rollback
assert_installed "$OLD_CHECKSUM"

run_ok --uninstall
ln -s "$OLD_BUNDLE" "$DESTINATION"
expect_status 70 "$REGISTER_OK" "" valid-destination-symlink --install "$NEW_BUNDLE"
[[ -L "$DESTINATION" ]]
rm -f "$DESTINATION"
run_ok --rollback
assert_installed "$OLD_CHECKSUM"

ditto "$NEW_BUNDLE" "$LEGACY"
CURRENT_BEFORE="$(checksum "$DESTINATION")"
LEGACY_BEFORE="$(checksum "$LEGACY")"
expect_status 70 "$REGISTER_OK" "" ambiguous-state --install "$NEW_BUNDLE"
[[ "$(checksum "$DESTINATION")" == "$CURRENT_BEFORE" ]]
[[ "$(checksum "$LEGACY")" == "$LEGACY_BEFORE" ]]
chmod -R u+w "$LEGACY"
rm -rf "$LEGACY"

chmod u-x "$DESTINATION/Contents/MacOS/AgentInputMethod"
expect_status 70 "$REGISTER_OK" "" non-executable-host --uninstall
[[ -e "$DESTINATION" ]]
chmod u+x "$DESTINATION/Contents/MacOS/AgentInputMethod"

REAL_INSTALL_DIR="$TEMP_ROOT/real-install-dir"
LINK_INSTALL_DIR="$TEMP_ROOT/link-install-dir"
mkdir -p "$REAL_INSTALL_DIR"
ln -s "$REAL_INSTALL_DIR" "$LINK_INSTALL_DIR"
set +e
AIM_INSTALL_TEST_MODE=1 AIM_INSTALL_DIR="$LINK_INSTALL_DIR" AIM_INSTALL_STATE_DIR="$TEMP_ROOT/link-test-state" \
  AIM_INSTALL_RUNTIME_DIR="$TEMP_ROOT/link-test-runtime" AIM_INSTALL_RUNTIME_LOCK_HELPER="$RUNTIME_LOCK_HELPER" \
  AIM_INSTALL_SKIP_SERVICE_REFRESH=1 AIM_INSTALL_SKIP_HOST_STOP=1 AIM_INSTALL_REGISTER_HELPER="$REGISTER_OK" \
  "$INSTALLER" --install "$NEW_BUNDLE" >/dev/null 2>&1
LINK_INSTALL_STATUS=$?
set -e
[[ "$LINK_INSTALL_STATUS" -eq 2 ]]

REAL_STATE_DIR="$TEMP_ROOT/real-state-dir"
LINK_STATE_DIR="$TEMP_ROOT/link-state-dir"
mkdir -p "$REAL_STATE_DIR" "$TEMP_ROOT/link-test-install"
ln -s "$REAL_STATE_DIR" "$LINK_STATE_DIR"
set +e
AIM_INSTALL_TEST_MODE=1 AIM_INSTALL_DIR="$TEMP_ROOT/link-test-install" AIM_INSTALL_STATE_DIR="$LINK_STATE_DIR" \
  AIM_INSTALL_RUNTIME_DIR="$TEMP_ROOT/link-test-runtime" AIM_INSTALL_RUNTIME_LOCK_HELPER="$RUNTIME_LOCK_HELPER" \
  AIM_INSTALL_SKIP_SERVICE_REFRESH=1 AIM_INSTALL_SKIP_HOST_STOP=1 AIM_INSTALL_REGISTER_HELPER="$REGISTER_OK" \
  "$INSTALLER" --install "$NEW_BUNDLE" >/dev/null 2>&1
LINK_STATE_STATUS=$?
set -e
[[ "$LINK_STATE_STATUS" -eq 2 ]]

assert_no_debris
[[ "$(checksum "$SOURCE_BUNDLE")" == "$SOURCE_CHECKSUM" ]]

reset_sandbox
ditto "$OLD_BUNDLE" "$DESTINATION"
ditto "$OLD_BUNDLE" "$BACKUP"
shasum -a 256 "$NEW_BUNDLE/Contents/MacOS/AgentInputMethod" > "$STATE_DIR/current.sha256"
shasum -a 256 "$BACKUP/Contents/MacOS/AgentInputMethod" > "$STATE_DIR/previous.sha256"
run_ok --recover
[[ "$(awk 'NR == 1 { print $1 }' "$STATE_DIR/current.bundle.sha256")" == "$OLD_CHECKSUM" ]]
[[ "$(awk 'NR == 1 { print $1 }' "$STATE_DIR/previous.bundle.sha256")" == "$OLD_CHECKSUM" ]]
assert_installed "$OLD_CHECKSUM"
assert_no_debris

printf 'do-not-delete\n' > "$TEMP_ROOT/path-escape-sentinel"
mkdir -p "$INSTALL_DIR/.agent-ime-transaction.safe"
printf '%s\n' \
  'format=2' \
  'phase=old_moved' \
  'action=--install' \
  "original=$DESTINATION" \
  "originalChecksum=$OLD_CHECKSUM" \
  "newChecksum=$NEW_CHECKSUM" \
  "root=$INSTALL_DIR/.agent-ime-transaction.safe/../../path-escape-sentinel" \
  "old=$INSTALL_DIR/.agent-ime-transaction.safe/../../path-escape-sentinel/original.inputmethod" \
  'stageRoot=' \
  'backupStage=' \
  'backupStageRoot=' \
  'backupCommitRoot=' \
  'backupChecksumNew=' \
  'backupOldChecksum=' \
  'backupNewChecksum=' > "$STATE_DIR/transaction.journal"
expect_status 70 "$REGISTER_OK" "" journal-path-escape --recover
[[ "$(< "$TEMP_ROOT/path-escape-sentinel")" == 'do-not-delete' ]]
rm -f "$STATE_DIR/recovery-required" "$STATE_DIR/transaction.journal"
chmod -R u+w "$STATE_DIR/install.lock" "$INSTALL_DIR/.agent-ime-transaction.safe" 2>/dev/null || true
rm -rf "$STATE_DIR/install.lock" "$INSTALL_DIR/.agent-ime-transaction.safe"

RC_POINTER="$ROOT_DIR/docs/current/evidence/v0.2-v0.3/RC_ARTIFACT.sha256"
if [[ -f "$RC_POINTER" && ! -L "$RC_POINTER" ]]; then
  set +e
  "$INSTALLER" --validate-official "$NEW_BUNDLE" "$RC_POINTER" \
    > "$TEMP_ROOT/official-validator-rejects-adhoc.txt" 2>&1
  OFFICIAL_REJECT_STATUS=$?
  set -e
  [[ "$OFFICIAL_REJECT_STATUS" -ne 0 ]]
  [[ "$(checksum "$NEW_BUNDLE")" == "$NEW_CHECKSUM" ]]
fi

# --- enable-verification e2e (2026-07-17 vanished-input-source incident) ---
ENABLE_OK="$TEMP_ROOT/enable-ok.sh"
ENABLE_STATUS_DEAD="$TEMP_ROOT/enable-status-dead.sh"
ENABLE_FAIL_ONCE="$TEMP_ROOT/enable-fail-once.sh"
ENABLE_FAIL_FLAG="$TEMP_ROOT/enable-failed-once"
ENABLED_LIST_YES="$TEMP_ROOT/enabled-list-yes.sh"
ENABLED_LIST_NO="$TEMP_ROOT/enabled-list-no.sh"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'exit 0' > "$ENABLE_OK"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  '[[ "$1" == "status" ]] && exit 7' \
  'exit 0' > "$ENABLE_STATUS_DEAD"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'if [[ "$1" == "status" && ! -e "'"$ENABLE_FAIL_FLAG"'" ]]; then' \
  '  touch "'"$ENABLE_FAIL_FLAG"'"' \
  '  exit 7' \
  'fi' \
  'exit 0' > "$ENABLE_FAIL_ONCE"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'echo "Bundle ID = com.hongye.facai.inputmethod.zhizi"' > "$ENABLED_LIST_YES"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'echo "Bundle ID = com.apple.inputmethod.SCIM"' > "$ENABLED_LIST_NO"
chmod +x "$ENABLE_OK" "$ENABLE_STATUS_DEAD" "$ENABLE_FAIL_ONCE" "$ENABLED_LIST_YES" "$ENABLED_LIST_NO"

reset_install_root() {
  chmod -R u+w "$INSTALL_DIR" "$STATE_DIR" 2>/dev/null || true
  rm -rf "$INSTALL_DIR" "$STATE_DIR"
  mkdir -p "$INSTALL_DIR" "$STATE_DIR"
}

# 1) Happy path: ctl healthy + session list contains the source -> success.
reset_install_root
AIM_INSTALL_ENABLE_CTL_HELPER="$ENABLE_OK" AIM_INSTALL_ENABLED_LIST_HELPER="$ENABLED_LIST_YES" \
  expect_status 0 "$REGISTER_OK" "" enable-verify-pass --install "$NEW_BUNDLE"
assert_installed "$NEW_CHECKSUM"

# 2) The incident shape: TIS status reports healthy but the login-session
#    enabled list does NOT contain the source -> loud exit 8, files kept.
reset_install_root
AIM_INSTALL_ENABLE_CTL_HELPER="$ENABLE_OK" AIM_INSTALL_ENABLED_LIST_HELPER="$ENABLED_LIST_NO" \
  expect_status 8 "$REGISTER_OK" "" enable-verify-session-miss --install "$NEW_BUNDLE"
assert_installed "$NEW_CHECKSUM"
grep -q "did not return to the session's enabled/selected lists" "$TEMP_ROOT/expected-enable-verify-session-miss.txt"

# 3) Disabled source that enable cannot revive -> loud exit 8, files kept.
reset_install_root
AIM_INSTALL_ENABLE_CTL_HELPER="$ENABLE_STATUS_DEAD" AIM_INSTALL_ENABLED_LIST_HELPER="$ENABLED_LIST_YES" \
  expect_status 8 "$REGISTER_OK" "" enable-verify-dead --install "$NEW_BUNDLE"
assert_installed "$NEW_CHECKSUM"

# 4) One flaky status answer recovers through the retry cycle -> success,
#    and the retry re-registered the bundle (register log gained a line).
reset_install_root
rm -f "$ENABLE_FAIL_FLAG" "$REGISTER_LOG"
AIM_INSTALL_ENABLE_CTL_HELPER="$ENABLE_FAIL_ONCE" AIM_INSTALL_ENABLED_LIST_HELPER="$ENABLED_LIST_YES" \
  expect_status 0 "$REGISTER_OK" "" enable-verify-retry --install "$NEW_BUNDLE"
assert_installed "$NEW_CHECKSUM"
[[ "$(wc -l < "$REGISTER_LOG" | tr -d ' ')" == "2" ]]

printf 'installer_transaction_tests=PASS old_checksum=%s new_checksum=%s\n' "$OLD_CHECKSUM" "$NEW_CHECKSUM"
