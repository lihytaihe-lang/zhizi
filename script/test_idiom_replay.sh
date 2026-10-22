#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/idiom-regression-v1.json"
FROZEN_CORPUS_SHA="0099f4518883250d0ee366d06786eed579c4c97857a54929520370b203343fbc"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "idiom corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}

BIN_DIR="$(AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
[[ -x "$HARNESS" ]] || { echo "missing replay harness: $HARNESS" >&2; exit 2; }

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-idiom.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" \
  "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 21 and .repetitions == 5 and
  .attemptedSessions == 105 and .createdSessions == 105 and .executedSessions == 105 and
  .candidatesMissingReadingKey == 0 and .top5Hits == 21 and
  .mustPassTop1Hits == 3 and .mustPassTop1Cases == 3 and .stableCases == 21 and
  (.failures | length) == 0 and
  (.observations | map({key: .query, value: .actual}) | from_entries) as $actual |
  $actual.baichigantougengjinyibu[0] == "百尺竿头更进一步" and
  ($actual.yisibugou | map(select(. == "一丝不苟")) | length) == 1 and
  ($actual.zixiangmaodun | map(select(. == "自相矛盾")) | length) == 1 and
  ($actual.mingfuqishi | map(select(. == "名副其实")) | length) == 1 and
  $actual.nihao[0] == "你好" and $actual.zhongguo[0] == "中国"
' "$REPORT" >/dev/null

jq -c '.observations[] | {query, candidates: .actual}' "$REPORT"
echo "idiom_replay=PASS cases=21 sessions=105 top5=21/21 must_top1=3/3 stable=21/21 dedup=0 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA"
if [[ -n "${AIM_IDIOM_REPORT_OUT:-}" ]]; then
  mkdir -p "$(dirname "$AIM_IDIOM_REPORT_OUT")"
  cp "$REPORT" "$AIM_IDIOM_REPORT_OUT"
fi
