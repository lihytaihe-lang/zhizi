#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/q3b1-garble-v1.json"
FROZEN_CORPUS_SHA="6aae6e9c0973c91c3be4b02feeeb22254a2fd04e82af83375507cfcceece393e"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "Q3-B1 corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}
jq -e '
  (.cases | length) == 25 and all(.cases[]; .mustPassTop1 == true) and
  ([.cases[] | select(.category == "must-flip-to-chinese")] | length) == 8 and
  ([.cases[] | select(.category == "must-stay-chinese")] | length) == 8 and
  ([.cases[] | select(.category == "must-stay-english")] | length) == 9
' "$CORPUS" >/dev/null

touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-q3b1.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"

DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 25 and .repetitions == 5 and
  .attemptedSessions == 125 and .createdSessions == 125 and .executedSessions == 125 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 25 and .top5Hits == 25 and
  .mustPassTop1Hits == 25 and .mustPassTop1Cases == 25 and .stableCases == 25 and
  (.failures | length) == 0
' "$REPORT" >/dev/null

echo "q3b1_garble_replay=PASS cases=25 sessions=125 top1=25/25 top5=25/25 stable=25/25 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA groups=flip:8,stay_chinese:8,stay_english:9"

if [[ -n "${AIM_Q3B1_REPORT_OUT:-}" ]]; then
  mkdir -p "$(dirname "$AIM_Q3B1_REPORT_OUT")"
  cp "$REPORT" "$AIM_Q3B1_REPORT_OUT"
fi
