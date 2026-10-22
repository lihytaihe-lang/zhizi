#!/usr/bin/env bash
# Guards the Manager <-> Helper Replay Gate contract.
#
# The Manager reaches AgentIMEPersonalReplayHarness only from a Tauri command, so
# no automated test ever executed that path. On 2026-07-28 the audit found it had
# been broken in two independent ways at once: the Manager passed six arguments
# where the helper demanded ten, and the helper had moved its report to schema 2
# while the Manager still validated schema 1. Both are static facts, so a static
# gate catches them without a built bundle or the owner's real corpus.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

HARNESS="native/macos/Sources/AgentIMEPersonalReplayHarness/main.swift"
GATE_KIT="native/macos/Sources/AgentInputMethodHostKit/PersonalReplayGate.swift"
MANAGER="src-tauri/src/replay_gate.rs"
RUNNER="script/run_bound_personal_replay.sh"

for file in "$HARNESS" "$GATE_KIT" "$MANAGER" "$RUNNER"; do
  [[ -f "$file" ]] || { echo "replay gate contract: missing $file" >&2; exit 2; }
done

fail() { echo "replay gate contract: $1" >&2; exit 1; }

# 1. Argument count. The helper counts argv including argv[0]; callers pass one
#    fewer.
HARNESS_ARGC="$(grep -Eo 'arguments\.count == [0-9]+' "$HARNESS" | grep -Eo '[0-9]+' | head -1)"
[[ -n "$HARNESS_ARGC" ]] || fail "cannot read the helper argument count from $HARNESS"
EXPECTED_ARGS=$((HARNESS_ARGC - 1))

MANAGER_ARGS="$(
  awk '/Command::new\(input\.helper\)/{inside=1} inside{print} /\.output\(\)/{if(inside) exit}' \
    "$MANAGER" | grep -c '^[[:space:]]*\.arg('
)"
[[ "$MANAGER_ARGS" -eq "$EXPECTED_ARGS" ]] || fail \
  "Manager passes $MANAGER_ARGS arguments but the helper requires $EXPECTED_ARGS"

RUNNER_ARGS="$(
  awk '
    # The invocation, not the executable-bit guard on the same variable.
    /"\$HARNESS"/ && !/^\[\[/ { inside = 1 }
    inside {
      line = $0
      sub(/\\$/, "", line)
      invocation = invocation " " line
      if ($0 !~ /\\$/) exit
    }
    END {
      sub(/^.*"\$HARNESS"/, "", invocation)
      count = split(invocation, tokens, /[[:space:]]+/)
      total = 0
      for (index_ = 1; index_ <= count; index_++) {
        if (tokens[index_] != "") total++
      }
      print total
    }
  ' "$RUNNER"
)"
[[ "$RUNNER_ARGS" -eq "$EXPECTED_ARGS" ]] || fail \
  "CLI runner passes $RUNNER_ARGS arguments but the helper requires $EXPECTED_ARGS"

# 2. Report schema. The helper stamps it; the Manager validates it.
HELPER_REPORT_SCHEMA="$(grep -Eo 'schemaVersion: [0-9]+' "$GATE_KIT" | grep -Eo '[0-9]+' | tail -1)"
MANAGER_REPORT_SCHEMA="$(grep -Eo 'const REPORT_SCHEMA_VERSION: u64 = [0-9]+' "$MANAGER" | grep -Eo '[0-9]+$')"
[[ -n "$HELPER_REPORT_SCHEMA" && -n "$MANAGER_REPORT_SCHEMA" ]] || fail "cannot read the report schema versions"
[[ "$HELPER_REPORT_SCHEMA" == "$MANAGER_REPORT_SCHEMA" ]] || fail \
  "helper emits report schema $HELPER_REPORT_SCHEMA but the Manager validates $MANAGER_REPORT_SCHEMA"

# 3. Golden schema. It is a separate contract and must not drift onto the report
#    version — the owner corpus is frozen at 1 and is never re-frozen to make a
#    report parse.
HARNESS_GOLDEN_SCHEMA="$(grep -Eo 'golden\.schemaVersion == [0-9]+' "$HARNESS" | grep -Eo '[0-9]+' | head -1)"
MANAGER_GOLDEN_SCHEMA="$(grep -Eo 'const GOLDEN_SCHEMA_VERSION: u64 = [0-9]+' "$MANAGER" | grep -Eo '[0-9]+$')"
[[ -n "$HARNESS_GOLDEN_SCHEMA" && -n "$MANAGER_GOLDEN_SCHEMA" ]] || fail "cannot read the golden schema versions"
[[ "$HARNESS_GOLDEN_SCHEMA" == "$MANAGER_GOLDEN_SCHEMA" ]] || fail \
  "helper accepts golden schema $HARNESS_GOLDEN_SCHEMA but the Manager freezes $MANAGER_GOLDEN_SCHEMA"
[[ "$MANAGER_GOLDEN_SCHEMA" != "$MANAGER_REPORT_SCHEMA" ]] || fail \
  "golden and report schema versions collapsed onto $MANAGER_GOLDEN_SCHEMA; they are independent contracts"

# 4. The Manager must never hand the helper the live user Rime directory.
grep -q 'arg(&user_db_snapshot)' "$MANAGER" || fail \
  "the Manager no longer passes an immutable UserDB snapshot to the helper"

echo "replay_gate_contract=PASS helper_argv=$HARNESS_ARGC caller_args=$EXPECTED_ARGS report_schema=$MANAGER_REPORT_SCHEMA golden_schema=$MANAGER_GOLDEN_SCHEMA"
