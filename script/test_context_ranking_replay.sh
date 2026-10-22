#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
[[ -f "$BUNDLE/Contents/Resources/ContextDisambig.json" ]] || {
  echo "missing context disambiguation table in release bundle" >&2
  exit 2
}
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c release \
  --disable-automatic-resolution --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c release \
  --disable-automatic-resolution --show-bin-path)"
TEMP_ROOT="$(mktemp -d /tmp/agent-ime-context-q4.XXXXXX)"
echo "temporary_cleanup_scope=$TEMP_ROOT"
MODEL_REPORT="$TEMP_ROOT/context-model.json"
MISSING_REPORT="$TEMP_ROOT/context-missing.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$BIN_DIR/AgentIMEReplayHarness" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/model-user" \
  "$ROOT_DIR/quality/corpus/context-q4-regression-v1.json" "$MODEL_REPORT" 5 \
  "$ARTIFACT_SHA" >/dev/null
jq -e '
  .cases == 4 and .sessions == 20 and .top1Hits == 4 and
  .mustPassTop1Hits == 4 and .stableCases == 4 and .failures == []
' "$MODEL_REPORT" >/dev/null

mkdir -p "$TEMP_ROOT/missing-resources"
cp -R "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/missing-resources/Rime"
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$BIN_DIR/AgentIMEReplayHarness" \
  "$TEMP_ROOT/missing-resources/Rime" "$TEMP_ROOT/missing-user" \
  "$ROOT_DIR/quality/corpus/context-q4-degraded-v1.json" "$MISSING_REPORT" 5 \
  "$ARTIFACT_SHA" >/dev/null
jq -e '
  .cases == 4 and .sessions == 20 and .top1Hits == 4 and
  .mustPassTop1Hits == 4 and .stableCases == 4 and .failures == []
' "$MISSING_REPORT" >/dev/null

jq -r '
  "context_q4_replay=PASS top1=\(.top1Hits)/\(.cases) stable=\(.stableCases)/\(.cases) observations=\(.observations | map(.id + ":" + (.actual | join("|"))) | join(","))"
' "$MODEL_REPORT"
echo "context_q4_missing_table=PASS sessions=20 top1=4/4 stable=4/4"

if [[ -n "${AIM_CONTEXT_Q4_ARTIFACT_DIR:-}" ]]; then
  mkdir -p "$AIM_CONTEXT_Q4_ARTIFACT_DIR"
  cp "$MODEL_REPORT" "$AIM_CONTEXT_Q4_ARTIFACT_DIR/context-table-replay.json"
  cp "$MISSING_REPORT" "$AIM_CONTEXT_Q4_ARTIFACT_DIR/context-missing-table-replay.json"
fi
