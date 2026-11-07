#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
SHARED_RIME="$BUNDLE/Contents/Resources/Rime"
CORPUS="$ROOT_DIR/quality/corpus/short-input-factory-top1-v1.json"
SECONDARY_FIXTURE="$ROOT_DIR/quality/fixtures/learning-gesture-secondary-v1.json"
FROZEN_CORPUS_SHA="2d27435e62f42331b12e636a747ff5c120f657b6d759042ca30b2e64c9f7fc03"
USE_ITERATIONS=12

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || {
  echo "missing release input bundle: $BUNDLE" >&2
  exit 2
}
ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "learning-degradation corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}

cargo build --manifest-path "$ROOT_DIR/native/core/agent-ime-core/Cargo.toml" \
  --release >/dev/null
touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEUserDBHarness >/dev/null
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --show-bin-path)"
USERDB_HARNESS="$BIN_DIR/AgentIMEUserDBHarness"
REPLAY_HARNESS="$BIN_DIR/AgentIMEReplayHarness"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-learning-degradation.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT

run_learning_period() {
  local arm="$1" gesture="$2" root_variable="$3" output_variable="$4"
  local attempt root output status
  for attempt in 1 2 3; do
    root="$TEMP_ROOT/$arm-attempt-$attempt"
    output="$TEMP_ROOT/$arm-attempt-$attempt.json"
    mkdir -p "$root/Rime"
    set +e
    "$USERDB_HARNESS" --learning-period "$SHARED_RIME" "$root" \
      "$gesture" "$output" 9300000000 "$SECONDARY_FIXTURE" "$USE_ITERATIONS" \
      >/dev/null
    status=$?
    set -e
    if [[ "$status" -eq 0 ]]; then
      printf -v "$root_variable" '%s' "$root"
      printf -v "$output_variable" '%s' "$output"
      return 0
    fi
    if [[ "$status" -ne 139 ]]; then
      echo "learning_degradation_harness=FAIL arm=$arm exit=$status" >&2
      return "$status"
    fi
    echo "learning_degradation_harness_restart=$attempt arm=$arm signal=SIGSEGV isolated_root_discarded=true" >&2
    sleep "$attempt"
  done
  echo "learning_degradation_harness=FAIL arm=$arm sigsegv_attempts=3" >&2
  return 139
}

CONTROL_ROOT=""
USED_ROOT=""
CONTROL_OUTPUT=""
USED_OUTPUT=""
run_learning_period control multi-segment-prefix-control CONTROL_ROOT CONTROL_OUTPUT
run_learning_period used multi-segment-nolearn USED_ROOT USED_OUTPUT

# Zhixu learning store invariant (the successor of the old UserDB logical diff):
# committing the auto-completed remainder (Space after the confirmed prefix)
# must not add or change learned entries beyond the deliberate prefix
# confirmation the control arm already performs. Compared fields are the
# learning entries (reading/text/count/lastMs); choices record per-app context
# that legitimately differs between the arms.
learning_entries() {
  local store="$1/Rime/agent_zhixu_learning_v1.json"
  [[ -f "$store" && ! -L "$store" ]] || { echo "[]"; return; }
  jq -S '[.entries[] | {reading, text, count, lastMs}] | sort_by(.reading, .text)' "$store"
}
learning_entries "$CONTROL_ROOT" > "$TEMP_ROOT/control-entries.json"
learning_entries "$USED_ROOT" > "$TEMP_ROOT/used-entries.json"

if ! jq -e --slurpfile control "$TEMP_ROOT/control-entries.json" \
  --slurpfile used "$TEMP_ROOT/used-entries.json" '
  ($control[0] | length) == ($used[0] | length) and
  ($control[0] == $used[0])
' <<< '{}' >/dev/null; then
  echo "learning_degradation_gate=FAIL unauthorized_persistent_learning control_count=$(jq 'length' "$TEMP_ROOT/control-entries.json") used_count=$(jq 'length' "$TEMP_ROOT/used-entries.json")" >&2
  diff <(jq -S . "$TEMP_ROOT/control-entries.json") <(jq -S . "$TEMP_ROOT/used-entries.json") >&2 || true
  exit 1
fi

jq -e '
  .gesture == "multi-segment-nolearn" and
  .iterations == 12 and
  .commitText == "你好吗"
' "$USED_OUTPUT" >/dev/null

REPORT="$TEMP_ROOT/after-use-report.json"
ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
"$REPLAY_HARNESS" "$SHARED_RIME" "$USED_ROOT/Rime" \
  "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" >/dev/null
jq -e '
  .cases == 20 and .attemptedSessions == 100 and
  .createdSessions == 100 and .executedSessions == 100 and
  .top1Hits == 20 and .mustPassTop1Hits == 20 and
  .mustPassTop1Cases == 20 and .stableCases == 20 and
  (.failures | length) == 0
' "$REPORT" >/dev/null

echo "learning_degradation_gate=PASS legal_prefix_selections=$USE_ITERATIONS required_segment_commits=$USE_ITERATIONS zhixu_entries_diff=0 factory_top1=20/20 sessions=100 corpus_checksum=$ACTUAL_CORPUS_SHA"
