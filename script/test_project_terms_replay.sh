#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/project-terms-regression-v1.json"
ABBREVIATION_CORPUS="$ROOT_DIR/quality/corpus/project-abbreviation-regression-v1.json"
FROZEN_CORPUS_SHA="702307b4a85ab14b63bf6e3f451ae9f572a2a55444e86dcf8b58cc31c2b68780"
FROZEN_ABBREVIATION_CORPUS_SHA="a59c63874cc97f7f5d491741ec875855d5f351c7b96efd6b8a0fdbe0440981ec"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "project-terms corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}
ACTUAL_ABBREVIATION_CORPUS_SHA="$(shasum -a 256 "$ABBREVIATION_CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_ABBREVIATION_CORPUS_SHA" == "$FROZEN_ABBREVIATION_CORPUS_SHA" ]] || {
  echo "project-abbreviation corpus checksum mismatch: expected $FROZEN_ABBREVIATION_CORPUS_SHA, got $ACTUAL_ABBREVIATION_CORPUS_SHA" >&2
  exit 6
}

touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-project-terms.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ABBREVIATION_REPORT="$TEMP_ROOT/abbreviation-report.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 7 and .repetitions == 5 and
  .attemptedSessions == 35 and .createdSessions == 35 and .executedSessions == 35 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 7 and
  .mustPassTop1Hits == 7 and .mustPassTop1Cases == 7 and .stableCases == 7 and
  (.failures | length) == 0 and
  (.observations | map({key: .query, value: .actual[0]}) | from_entries) as $first |
  $first.productinputs == "ProductInputs" and $first.librime == "librime" and
  $first.entitlement == "entitlement" and $first.agentinputmethod == "AgentInputMethod" and
  $first.productcommit == "ProductCommit" and $first.nihao == "你好" and
  $first.zhongguo == "中国"
' "$REPORT" >/dev/null

DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/abbreviation-user" \
  "$ABBREVIATION_CORPUS" "$ABBREVIATION_REPORT" 5 "$ARTIFACT_SHA" >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 5 and .repetitions == 5 and
  .attemptedSessions == 25 and .createdSessions == 25 and .executedSessions == 25 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 5 and .top5Hits == 5 and
  .mustPassTop1Hits == 5 and .mustPassTop1Cases == 5 and .stableCases == 5 and
  (.failures | length) == 0 and
  (.observations | map({key: .id, value: .actual}) | from_entries) as $actual |
  $actual["pi-inside"] == ["ProductInputs", "皮", "批", "屁", "披"] and
  $actual["pi-outside"] == ["皮", "批", "屁", "披", "脾"] and
  $actual["zhem-control"] == ["这么", "折磨", "这名", "这门", "这面"] and
  $actual["zenm-control"] == ["怎么", "怎", "则能", "责难", "之恩"] and
  $actual["dansh-control"] == ["但是", "单身", "诞生", "单手", "淡水"]
' "$ABBREVIATION_REPORT" >/dev/null

jq -c '.observations[] | {query, candidates: .actual}' "$REPORT"
echo "project_terms_replay=PASS cases=7 sessions=35 top1=7/7 stable=7/7 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA"
jq -c '.observations[] | {id, query, candidates: .actual}' "$ABBREVIATION_REPORT"
echo "project_abbreviation_replay=PASS cases=5 sessions=25 top1=5/5 stable=5/5 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_ABBREVIATION_CORPUS_SHA"
