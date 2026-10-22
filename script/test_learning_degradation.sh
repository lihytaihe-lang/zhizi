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

LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib" \
  cargo build --manifest-path "$ROOT_DIR/native/core/agent-ime-core/Cargo.toml" \
  --release --features librime >/dev/null
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
USERDB_CTL="$BUNDLE/Contents/MacOS/AgentIMEUserDBCtl"

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
    DYLD_LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib" \
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

list_entries() {
  local user_directory="$1" output="$2"
  DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$USERDB_CTL" list \
    "$SHARED_RIME" "$user_directory" |
    jq -S '[.items[] | {pinyin, text, entryLine}] | sort_by(.pinyin, .text)' > "$output"
}
list_entries "$CONTROL_ROOT/Rime" "$TEMP_ROOT/control-entries.json"
list_entries "$USED_ROOT/Rime" "$TEMP_ROOT/used-entries.json"

jq -n --slurpfile control "$TEMP_ROOT/control-entries.json" \
  --slurpfile used "$TEMP_ROOT/used-entries.json" '
  def keyed: map({key: (.pinyin + "\u0000" + .text), value: .}) | from_entries;
  ($control[0] | keyed) as $c |
  ($used[0] | keyed) as $u |
  {
    added: [($u | keys[]) as $key | select($c[$key] == null) | $u[$key]],
    removed: [($c | keys[]) as $key | select($u[$key] == null) | $c[$key]],
    valueChanged: [
      ($u | keys[]) as $key |
      select($c[$key] != null and $c[$key].entryLine != $u[$key].entryLine) |
      {pinyin: $u[$key].pinyin, text: $u[$key].text,
       before: $c[$key].entryLine, after: $u[$key].entryLine}
    ]
  } |
  . + {
    addedCount: (.added | length),
    removedCount: (.removed | length),
    valueChangedCount: (.valueChanged | length)
  }
' > "$TEMP_ROOT/incremental-diff.json"

if ! jq -e '
  .addedCount == 0 and .removedCount == 0 and .valueChangedCount == 0
' "$TEMP_ROOT/incremental-diff.json" >/dev/null; then
  counts="$(jq -r '"added=\(.addedCount) removed=\(.removedCount) value_changed=\(.valueChangedCount)"' \
    "$TEMP_ROOT/incremental-diff.json")"
  echo "learning_degradation_gate=FAIL unauthorized_persistent_learning $counts" >&2
  jq -c '{added, removed, valueChanged}' "$TEMP_ROOT/incremental-diff.json" >&2
  exit 1
fi

jq -e '
  .selectionProbe.reinforce == false and
  .selectionProbe.requiresRegularSelection == true and
  .commitText == "你好吗"
' "$USED_OUTPUT" >/dev/null

REPORT="$TEMP_ROOT/after-use-report.json"
ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" \
  "$REPLAY_HARNESS" "$SHARED_RIME" "$USED_ROOT/Rime" \
  "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" >/dev/null
jq -e '
  .cases == 20 and .attemptedSessions == 100 and
  .createdSessions == 100 and .executedSessions == 100 and
  .top1Hits == 20 and .mustPassTop1Hits == 20 and
  .mustPassTop1Cases == 20 and .stableCases == 20 and
  (.failures | length) == 0
' "$REPORT" >/dev/null

echo "learning_degradation_gate=PASS legal_prefix_selections=$USE_ITERATIONS required_segment_commits=$USE_ITERATIONS unauthorized_diff=0 factory_top1=20/20 sessions=100 corpus_checksum=$ACTUAL_CORPUS_SHA"
