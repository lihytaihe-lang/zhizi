#!/usr/bin/env bash
# Replay all unique owner readings on private UserDB copies before and after the
# evidence-backed cleanup group. The live UserDB is copied once and never passed
# to librime or the maintenance helper.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

EVENTS="${1:-${HOME}/Library/Application Support/AgentInputMethod/Quality/Frozen/events-2026-07-24.jsonl}"
BUNDLE="$ROOT_DIR/build/native/release/Agent Input Method.inputmethod"
EVIDENCE="$ROOT_DIR/quality/corpus/learning-cleanup-evidence-v3.json"
LIVE_USER_RIME="${HOME}/Library/Application Support/AgentInputMethod/Rime"
WORK_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-cleanup-impact.XXXXXX")"
trap 'chmod -R u+rwX "$WORK_ROOT" 2>/dev/null || true; rm -rf "$WORK_ROOT"' EXIT

[[ -f "$EVENTS" ]] || { echo "missing private quality history: $EVENTS" >&2; exit 2; }
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release bundle: $BUNDLE" >&2; exit 2; }
[[ -f "$EVIDENCE" ]] || { echo "missing cleanup evidence: $EVIDENCE" >&2; exit 2; }
[[ -d "$LIVE_USER_RIME" ]] || { echo "missing owner user rime: $LIVE_USER_RIME" >&2; exit 2; }

mkdir -p "$WORK_ROOT/before" "$WORK_ROOT/after"
cp -R "$LIVE_USER_RIME"/. "$WORK_ROOT/before/"
cp -R "$WORK_ROOT/before"/. "$WORK_ROOT/after/"
chmod -R u+rwX "$WORK_ROOT"

touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
if [[ "${AIM_SWIFTPM_SANDBOX_WORKAROUND:-0}" == "1" ]]; then
  SWIFT_MODULE_CACHE="${TMPDIR:-/tmp}/agent-ime-cleanup-impact-swift-module-cache"
  SWIFT_SCRATCH_PATH="${TMPDIR:-/tmp}/agent-ime-cleanup-impact-swift-build"
  mkdir -p "$SWIFT_MODULE_CACHE" "$SWIFT_SCRATCH_PATH"
  CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" \
    SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" \
    AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --disable-sandbox --scratch-path "$SWIFT_SCRATCH_PATH" \
    --product AgentIMEReplayHarness >/dev/null
  BIN_DIR="$(CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" \
    SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" \
    swift build --package-path "$ROOT_DIR/native/macos" -c debug \
    --disable-automatic-resolution --disable-sandbox --scratch-path "$SWIFT_SCRATCH_PATH" \
    --show-bin-path)"
else
  AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --product AgentIMEReplayHarness >/dev/null
  BIN_DIR="$(swift build --package-path "$ROOT_DIR/native/macos" -c debug \
    --disable-automatic-resolution --show-bin-path)"
fi
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
USERDB_CTL="$BUNDLE/Contents/MacOS/AgentIMEUserDBCtl"
SHARED="$BUNDLE/Contents/Resources/Rime"
export DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release"

entry_digest() {
  "$USERDB_CTL" list "$SHARED" "$1" \
    | jq -S -c '[.items[] | {pinyin,text,entryLine}] | sort' \
    | shasum -a 256 | awk '{print $1}'
}
entry_count() {
  "$USERDB_CTL" list "$SHARED" "$1" | jq '.items | length'
}

BEFORE_DIGEST="$(entry_digest "$WORK_ROOT/before")"
BEFORE_COUNT="$(entry_count "$WORK_ROOT/before")"
"$USERDB_CTL" list "$SHARED" "$WORK_ROOT/after" \
  | jq --slurpfile evidence "$EVIDENCE" -c '
      [.items[] as $item
       | $evidence[0].cases[]
       | select(
           .batchCleanupEligible
           and ((.query | gsub("[^a-z]"; "")) == ($item.pinyin | gsub("[^a-z]"; "")))
           and .observedOwnerTop1 == $item.text
         )
       | {id:("impact-" + .query),text:$item.text,pinyin:$item.pinyin,entryLine:null}]
    ' > "$WORK_ROOT/delete-request.json"
DELETE_COUNT="$(jq 'length' "$WORK_ROOT/delete-request.json")"
[[ "$DELETE_COUNT" -gt 0 ]] || { echo "private copy has no eligible cleanup entries" >&2; exit 3; }
"$USERDB_CTL" delete "$SHARED" "$WORK_ROOT/after" \
  < "$WORK_ROOT/delete-request.json" > "$WORK_ROOT/deleted.json"
AFTER_CLEANUP_COUNT="$(entry_count "$WORK_ROOT/after")"

jq -s '{
  kind:"short-input-learning-probe",
  cases:([.[].pinyin] | unique | sort | to_entries | map({
    id:("reading-" + (.key|tostring)),
    category:"owner-cleanup-impact",
    query:.value,
    allowed:[],
    mustPassTop1:false
  }))
}' "$EVENTS" > "$WORK_ROOT/all-readings.json"

ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
EVIDENCE_SHA="$(shasum -a 256 "$EVIDENCE" | awk '{print $1}')"
ELIGIBLE_READINGS="$(jq '.round5Cohort.eligibleReadings' "$EVIDENCE")"
ELIGIBLE_INCIDENTS="$(jq '.round5Cohort.eligibleIncidents' "$EVIDENCE")"
for arm in before after; do
  "$HARNESS" "$SHARED" "$WORK_ROOT/$arm" "$WORK_ROOT/all-readings.json" \
    "$WORK_ROOT/$arm-report.json" 1 "$ARTIFACT_SHA" >/dev/null
done

jq -c '[.items[]]' "$WORK_ROOT/deleted.json" \
  | "$USERDB_CTL" restore "$SHARED" "$WORK_ROOT/after" >/dev/null
AFTER_UNDO_DIGEST="$(entry_digest "$WORK_ROOT/after")"
UNDO_RESTORED=false
[[ "$BEFORE_DIGEST" == "$AFTER_UNDO_DIGEST" ]] && UNDO_RESTORED=true

node "$ROOT_DIR/scripts/summarize_learning_cleanup_impact.mjs" \
  --events "$EVENTS" \
  --before "$WORK_ROOT/before-report.json" \
  --after "$WORK_ROOT/after-report.json" \
  --deleted "$DELETE_COUNT" \
  --eligible-readings "$ELIGIBLE_READINGS" \
  --eligible-incidents "$ELIGIBLE_INCIDENTS" \
  --entries-before "$BEFORE_COUNT" \
  --entries-after-cleanup "$AFTER_CLEANUP_COUNT" \
  --undo-restored "$UNDO_RESTORED" \
  --owner-entry-digest-before "$BEFORE_DIGEST" \
  --owner-entry-digest-after-undo "$AFTER_UNDO_DIGEST" \
  --evidence-sha256 "$EVIDENCE_SHA" \
  ${AIM_LEARNING_CLEANUP_IMPACT_MODE:+--${AIM_LEARNING_CLEANUP_IMPACT_MODE}} \
  ${AIM_LEARNING_CLEANUP_IMPACT_REPORT:+--report "$AIM_LEARNING_CLEANUP_IMPACT_REPORT"}
