#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/fuzzy-exact-priority-v1.json"
FROZEN_CORPUS_SHA="2c957566981f7b2facddd96391d988805107170812d8c077143a3464b3e79d6c"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "fuzzy exact-priority checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}

touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-fuzzy-exact-priority.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"

DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 5 and .repetitions == 5 and
  .attemptedSessions == 25 and .createdSessions == 25 and .executedSessions == 25 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 5 and
  .mustPassTop1Hits == 5 and .mustPassTop1Cases == 5 and .stableCases == 5 and
  (.failures | length) == 0 and
  (.observations | map({key: .id, value: .actual}) | from_entries) as $actual |
  $actual["exact-over-fuzzy-congzheli"][0] == "从这里" and
  $actual["exact-over-fuzzy-congzhelikaolv"][0] == "从这里考虑" and
  $actual["exact-over-fuzzy-long-sentence"][0] == "我想从这里考虑" and
  $actual["fuzzy-feature-control-zege"][0] == "这个" and
  $actual["abbreviation-control-zenm"][0] == "怎么"
' "$REPORT" >/dev/null

jq -c '.observations[] | {id, query, candidates: .actual}' "$REPORT"
echo "fuzzy_exact_priority_replay=PASS cases=5 sessions=25 top1=5/5 stable=5/5 fuzzy_mask=231 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA"
