#!/usr/bin/env bash
# Shared gate runner for the regression entries.  Source it, never execute it.
#
# Why this helper exists (allowed by the 2026-10-17 gates task): both
# script/test_all.sh and scripts/tools/full_regression_cycle.sh need the same
# recording, dependency-blocking, logging and summary semantics.  Keeping one
# implementation here means the gate list lives only in script/test_all.sh and
# neither entry can drift into duplicate or missing steps.
#
# Semantics:
#   * Every gate runs to completion unless a prerequisite resource it declared
#     (comma-separated "needs") was not produced OK.  A failed gate never stops
#     later gates that do not depend on it; dependents become BLOCKED / NOT_RUN
#     and are never reported PASS.  A stale on-disk artifact cannot fake a pass:
#     bundle consumers are blocked by the failed build gate itself, not by
#     looking at what the build left behind.
#   * Any FAIL or BLOCKED gate makes the whole run exit non-zero.  Exit codes:
#     0 = all gates PASS and cleanup OK; 1 = gate failure/blocked;
#     2 = harness error (bad gate list, log directory failure, cleanup
#     failure); 130/143 = interrupted by SIGINT/SIGTERM.
#   * Judgement uses only what this run observed: the per-gate exit code of the
#     command executed now.  No exit code is special-cased (EXIT=6 is a
#     failure like any other) and no historical log is parsed.
#   * Per-gate evidence: id, exit code, wall-clock seconds, per-gate log file,
#     declared needs/provides and the exact command, in summary.tsv and in the
#     concise stdout summary.
#   * Stub/synthetic provenance is persistent evidence, not just a stdout
#     note: every run writes metadata.tsv next to summary.tsv (suite, stub,
#     synthetic, stub_root, root, started) and the REGRESSION RESULT line ends
#     with stub=<0|1> synthetic=<0|1>, so a synthetic green run can never be
#     mistaken for real-toolchain evidence by reading only the saved files.
#
# Self-test hook (scripts/tools/test_full_regression_cycle.py only): when
# AIM_REGRESSION_STUB_DIR is set, $STUB/bin is prepended to PATH (stub node,
# npm, cargo, swift, python3, ...) and script/*.sh gates resolve against
# $STUB/script, so synthetic stub commands replace every real build/test tool
# and the python unittest gate itself is stubbed too (this is what prevents
# self-recursion during the self-test).
#
# Bash 3.2 compatible (macOS /bin/bash): no associative arrays, no mapfile.

regression_env_init() {
  local lib_dir
  lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  REG_ROOT="$(cd "$lib_dir/../.." && pwd)"
  REG_SUITE="${1:-regression}"
  REG_STUB_MODE=0
  REG_SCRIPT_DIR="$REG_ROOT/script"
  REG_RUNS_BASE="$REG_ROOT/tmp/regression-runs"
  if [[ -n "${AIM_REGRESSION_STUB_DIR:-}" ]]; then
    REG_STUB_MODE=1
    PATH="$AIM_REGRESSION_STUB_DIR/bin:$PATH"
    export PATH
    REG_SCRIPT_DIR="$AIM_REGRESSION_STUB_DIR/script"
    REG_RUNS_BASE="$AIM_REGRESSION_STUB_DIR/runs"
  fi
  export REG_ROOT REG_SCRIPT_DIR
  REG_RUN_DIR=""
  REG_SUMMARY=""
  REG_METADATA=""
  REG_INTERRUPTED=""
  REG_HARNESS_ERROR=0
  REG_CLEANUP_STATUS=""
  GATE_IDS=()
  GATE_NEEDS=()
  GATE_PROVIDES=()
  GATE_CMDS=()
  GATE_STATUS=()
  GATE_EXIT=()
  GATE_SECS=()
  GATE_LOGS=()
  GATE_NOTES=()
  REG_RES_NAMES=()
  REG_RES_STATES=()
}

reg_harness_error() {
  echo "REGRESSION HARNESS ERROR: $*" >&2
  REG_HARNESS_ERROR=1
  exit 2
}

gate_add() {
  # gate_add <id> <needs:csv|empty> <provides:csv|empty> <command string>
  GATE_IDS+=("$1")
  GATE_NEEDS+=("$2")
  GATE_PROVIDES+=("$3")
  GATE_CMDS+=("$4")
  GATE_STATUS+=("")
  GATE_EXIT+=("")
  GATE_SECS+=("")
  GATE_LOGS+=("")
  GATE_NOTES+=("")
}

gates_validate() {
  local i j id token candidate duplicate=0 found=0
  local -a all_provides=()
  for i in "${!GATE_IDS[@]}"; do
    id="${GATE_IDS[$i]}"
    [[ "$id" =~ ^[a-z0-9_]+$ ]] || reg_harness_error "gate id must be [a-z0-9_]: $id"
    for j in "${!GATE_IDS[@]}"; do
      if [[ "$j" -lt "$i" && "${GATE_IDS[$j]}" == "$id" ]]; then
        reg_harness_error "duplicate gate id: $id"
      fi
    done
    [[ -n "${GATE_CMDS[$i]}" ]] || reg_harness_error "gate $id has an empty command"
    local OLDIFS="$IFS"
    IFS=','
    for token in ${GATE_PROVIDES[$i]}; do
      [[ -z "$token" ]] && continue
      duplicate=0
      for candidate in ${all_provides[@]+"${all_provides[@]}"}; do
        [[ "$candidate" == "$token" ]] && duplicate=1
      done
      [[ "$duplicate" -eq 0 ]] || reg_harness_error "resource $token provided twice"
      all_provides+=("$token")
    done
    IFS="$OLDIFS"
  done
  for i in "${!GATE_IDS[@]}"; do
    local OLDIFS="$IFS"
    IFS=','
    for token in ${GATE_NEEDS[$i]}; do
      [[ -z "$token" ]] && continue
      found=0
      for candidate in ${all_provides[@]+"${all_provides[@]}"}; do
        [[ "$candidate" == "$token" ]] && found=1
      done
      [[ "$found" -eq 1 ]] || reg_harness_error "gate ${GATE_IDS[$i]} needs unknown resource: $token"
    done
    IFS="$OLDIFS"
  done
}

gates_list() {
  local i
  printf 'id\tneeds\tprovides\tcommand\n'
  for i in "${!GATE_IDS[@]}"; do
    printf '%s\t%s\t%s\t%s\n' "${GATE_IDS[$i]}" "${GATE_NEEDS[$i]}" "${GATE_PROVIDES[$i]}" "${GATE_CMDS[$i]}"
  done
}

reg_res_set() {
  local i name="$1" state="$2"
  for i in "${!REG_RES_NAMES[@]}"; do
    if [[ "${REG_RES_NAMES[$i]}" == "$name" ]]; then
      REG_RES_STATES[$i]="$state"
      return 0
    fi
  done
  REG_RES_NAMES+=("$name")
  REG_RES_STATES+=("$state")
}

reg_res_get() {
  local i name="$1"
  for i in "${!REG_RES_NAMES[@]}"; do
    if [[ "${REG_RES_NAMES[$i]}" == "$name" ]]; then
      printf '%s' "${REG_RES_STATES[$i]}"
      return 0
    fi
  done
  return 1
}

reg_provide_all() {
  # reg_provide_all <provides csv> <state>; empty tokens skipped
  local token state="$2" OLDIFS="$IFS"
  IFS=','
  for token in $1; do
    [[ -z "$token" ]] && continue
    reg_res_set "$token" "$state"
  done
  IFS="$OLDIFS"
}

reg_kill_tree() {
  # bash 3.2 has no usable job-control process groups in scripts, so collect
  # the gate's descendants recursively and TERM them (children first).
  local parent="$1" child
  for child in $(pgrep -P "$parent" 2>/dev/null); do
    reg_kill_tree "$child"
  done
  kill -TERM "$parent" 2>/dev/null || true
}

reg_on_signal() {
  REG_INTERRUPTED="$1"
  if [[ -n "${REG_CHILD_PID:-}" ]]; then
    reg_kill_tree "$REG_CHILD_PID"
    REG_CHILD_PID=""
  fi
}

reg_run_cmd() {
  local cmd="$1" log="$2" rc=0
  REG_CHILD_PID=""
  bash -c "$cmd" >"$log" 2>&1 &
  REG_CHILD_PID=$!
  wait "$REG_CHILD_PID" || rc=$?
  REG_CHILD_PID=""
  return "$rc"
}

reg_record_gate() {
  local i="$1" note="${GATE_NOTES[$i]}"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "${GATE_IDS[$i]}" "${GATE_STATUS[$i]}" "${GATE_EXIT[$i]}" "${GATE_SECS[$i]}" \
    "${GATE_LOGS[$i]}" "${GATE_NEEDS[$i]}" "${GATE_PROVIDES[$i]}" "${GATE_CMDS[$i]}" \
    >>"$REG_SUMMARY" || reg_harness_error "cannot append summary: $REG_SUMMARY"
  echo "GATE $(printf '%03d' $((i + 1))) ${GATE_IDS[$i]} ${GATE_STATUS[$i]} exit=${GATE_EXIT[$i]} ${GATE_SECS[$i]}s log=${GATE_LOGS[$i]}${note:+ :: $note}"
}

regression_open_run_dir() {
  # AIM_REGRESSION_RUN_DIR (like the default base) is a *parent*: every run,
  # including nested suites, gets its own fresh child directory, so evidence
  # of an earlier run can never be overwritten or mixed.  A collision is a
  # harness error, never a truncation.
  local base="$REG_RUNS_BASE"
  if [[ -n "${AIM_REGRESSION_RUN_DIR:-}" ]]; then
    base="$AIM_REGRESSION_RUN_DIR"
  fi
  mkdir -p "$base" 2>/dev/null || reg_harness_error "cannot create log base: $base"
  REG_RUN_DIR="$base/$(date +%Y%m%d-%H%M%S)-$$-$REG_SUITE"
  [[ ! -e "$REG_RUN_DIR" ]] || reg_harness_error "run dir already exists: $REG_RUN_DIR"
  mkdir "$REG_RUN_DIR" 2>/dev/null || reg_harness_error "cannot create log dir: $REG_RUN_DIR"
  REG_SUMMARY="$REG_RUN_DIR/summary.tsv"
  printf 'id\tstatus\texit\tseconds\tlog\tneeds\tprovides\tcommand\n' >"$REG_SUMMARY" 2>/dev/null \
    || reg_harness_error "cannot write summary: $REG_SUMMARY"
  # Persistent provenance: whether this run executed synthetic stub commands
  # must survive in the saved evidence, not only in the stdout START line.
  REG_METADATA="$REG_RUN_DIR/metadata.tsv"
  {
    printf 'key\tvalue\n'
    printf 'suite\t%s\n' "$REG_SUITE"
    printf 'stub\t%s\n' "$REG_STUB_MODE"
    printf 'synthetic\t%s\n' "$REG_STUB_MODE"
    printf 'stub_root\t%s\n' "${AIM_REGRESSION_STUB_DIR:--}"
    printf 'root\t%s\n' "$REG_ROOT"
    printf 'started\t%s\n' "$(date +%Y-%m-%dT%H:%M:%S)"
  } >"$REG_METADATA" 2>/dev/null || reg_harness_error "cannot write metadata: $REG_METADATA"
  if ! : >"$REG_RUN_DIR/.writable" 2>/dev/null; then
    reg_harness_error "log dir is not writable: $REG_RUN_DIR"
  fi
  rm -f "$REG_RUN_DIR/.writable"
}

gates_run() {
  local i token state blocked rc start log note interrupted_saw
  regression_open_run_dir
  trap 'reg_on_signal INT' INT
  trap 'reg_on_signal TERM' TERM
  trap 'reg_exit_trap' EXIT
  echo "REGRESSION START suite=$REG_SUITE stub=$REG_STUB_MODE log_dir=$REG_RUN_DIR"
  for i in "${!GATE_IDS[@]}"; do
    if [[ -n "$REG_INTERRUPTED" ]]; then
      break
    fi
    blocked=""
    local OLDIFS="$IFS"
    IFS=','
    for token in ${GATE_NEEDS[$i]}; do
      [[ -z "$token" ]] && continue
      state="$(reg_res_get "$token" 2>/dev/null || true)"
      if [[ "$state" != "OK" ]]; then
        blocked="$token(${state:-NOT_PRODUCED})"
      fi
    done
    IFS="$OLDIFS"
    if [[ -n "$blocked" ]]; then
      GATE_STATUS[$i]=BLOCKED
      GATE_EXIT[$i]="-"
      GATE_SECS[$i]=0
      GATE_LOGS[$i]="-"
      GATE_NOTES[$i]="NOT_RUN: prerequisite $blocked"
      reg_provide_all "${GATE_PROVIDES[$i]}" FAILED
      reg_record_gate "$i"
      continue
    fi
    log="$REG_RUN_DIR/$(printf '%03d' $((i + 1)))-${GATE_IDS[$i]}.log"
    GATE_LOGS[$i]="$log"
    start=$(date +%s)
    rc=0
    reg_run_cmd "${GATE_CMDS[$i]}" "$log" || rc=$?
    GATE_SECS[$i]=$(( $(date +%s) - start ))
    GATE_EXIT[$i]="$rc"
    if [[ -n "$REG_INTERRUPTED" ]]; then
      GATE_STATUS[$i]=INTERRUPTED
      GATE_NOTES[$i]="NOT_RUN: interrupted by SIG$REG_INTERRUPTED"
      reg_provide_all "${GATE_PROVIDES[$i]}" FAILED
      reg_record_gate "$i"
      continue
    elif [[ "$rc" -eq 0 ]]; then
      GATE_STATUS[$i]=PASS
      reg_provide_all "${GATE_PROVIDES[$i]}" OK
    else
      GATE_STATUS[$i]=FAIL
      reg_provide_all "${GATE_PROVIDES[$i]}" FAILED
    fi
    reg_record_gate "$i"
  done
  for i in "${!GATE_IDS[@]}"; do
    [[ -n "${GATE_STATUS[$i]}" ]] && continue
    GATE_STATUS[$i]=BLOCKED
    GATE_EXIT[$i]="-"
    GATE_SECS[$i]=0
    GATE_LOGS[$i]="-"
    if [[ -n "$REG_INTERRUPTED" ]]; then
      GATE_NOTES[$i]="NOT_RUN: interrupted by SIG$REG_INTERRUPTED"
    else
      GATE_NOTES[$i]="NOT_RUN: skipped after earlier stop"
    fi
    reg_record_gate "$i"
  done
  local pass=0 fail=0 blockedn=0 interruptedn=0 result=PASS
  for i in "${!GATE_IDS[@]}"; do
    case "${GATE_STATUS[$i]}" in
      PASS) pass=$((pass + 1)) ;;
      FAIL) fail=$((fail + 1)); result=FAIL ;;
      BLOCKED) blockedn=$((blockedn + 1)); result=FAIL ;;
      INTERRUPTED) interruptedn=$((interruptedn + 1)); result=FAIL ;;
      *) reg_harness_error "gate ${GATE_IDS[$i]} has no status" ;;
    esac
  done
  echo "REGRESSION RESULT: $result pass=$pass fail=$fail blocked=$blockedn interrupted=$interruptedn total=${#GATE_IDS[@]} log_dir=$REG_RUN_DIR stub=$REG_STUB_MODE synthetic=$REG_STUB_MODE"
  [[ "$result" == "PASS" ]]
}

reg_exit_trap() {
  local rc=$?
  trap - EXIT INT TERM
  # Builds and fixtures share the installed input method's bundle identifier;
  # never leave one registered where LaunchServices could start it for the
  # owner's typing.  unregister_dev_bundles.sh only unregisters dev bundles,
  # never the owner's installed copy, so running it defensively is safe.
  if [[ -x "${REG_SCRIPT_DIR:-}/unregister_dev_bundles.sh" ]]; then
    if "${REG_SCRIPT_DIR}/unregister_dev_bundles.sh" >/dev/null 2>&1; then
      REG_CLEANUP_STATUS=0
    else
      REG_CLEANUP_STATUS=$?
    fi
  else
    REG_CLEANUP_STATUS=missing
  fi
  echo "CLEANUP unregister exit=$REG_CLEANUP_STATUS"
  if [[ "${REG_HARNESS_ERROR:-0}" -ne 0 ]]; then
    exit 2
  fi
  if [[ -n "${REG_INTERRUPTED:-}" ]]; then
    if [[ "$REG_INTERRUPTED" == "INT" ]]; then
      exit 130
    fi
    exit 143
  fi
  [[ "$REG_CLEANUP_STATUS" == "0" ]] || exit 2
  exit "$rc"
}
