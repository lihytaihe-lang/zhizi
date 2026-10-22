#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/q3b2-heavy-garble-v1.json"
FROZEN_CORPUS_SHA="d926bfb55975ce3dc2d43b9b0db6492fe13f5ee33f50e1565769dba3d58ecbdc"
REGISTERED_ASCII_CORPUS="$ROOT_DIR/quality/corpus/q3b2-registered-ascii-v1.json"
FROZEN_REGISTERED_ASCII_SHA="09907801149901e967043159f0b5c027c49cd7468669e2cbe5fbaa347fc1ebe2"
MINIMUM_TOP1=8
MINIMUM_TOP5=11
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "Q3-B2 corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}
jq -e '
  .kind == "q3b2-heavy-garble" and .formatVersion == 1 and
  (.cases | length) == 13 and
  ([.cases[] | select(.category == "control-english-must-win" and .mustPassTop1)] | length) == 2 and
  ([.cases[] | select(.category == "heavy-garble" or .category == "typo-insertion")] | length) == 11
' "$CORPUS" >/dev/null
ACTUAL_REGISTERED_ASCII_SHA="$(shasum -a 256 "$REGISTERED_ASCII_CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_REGISTERED_ASCII_SHA" == "$FROZEN_REGISTERED_ASCII_SHA" ]] || {
  echo "Q3-B2 registered ASCII checksum mismatch: expected $FROZEN_REGISTERED_ASCII_SHA, got $ACTUAL_REGISTERED_ASCII_SHA" >&2
  exit 6
}
jq -e '
  .kind == "q3b2-registered-ascii" and .formatVersion == 1 and
  (.cases | length) == 10 and all(.cases[]; .mustPassTop1 == true)
' "$REGISTERED_ASCII_CORPUS" >/dev/null

touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-q3b2.XXXXXX")"
trap 'status=$?; chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"; exit $status' EXIT
REPORT="$TEMP_ROOT/report.json"
REGISTERED_ASCII_REPORT="$TEMP_ROOT/registered-ascii-report.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"

set +e
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null
HEAVY_STATUS=$?
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/registered-ascii-user" \
  "$REGISTERED_ASCII_CORPUS" "$REGISTERED_ASCII_REPORT" 5 "$ARTIFACT_SHA" >/dev/null
REGISTERED_ASCII_STATUS=$?
set -e

jq -e --arg checksum "$ARTIFACT_SHA" \
  --argjson minimumTop1 "$MINIMUM_TOP1" --argjson minimumTop5 "$MINIMUM_TOP5" '
  .artifactChecksum == $checksum and .cases == 13 and .repetitions == 5 and
  .attemptedSessions == 65 and .createdSessions == 65 and .executedSessions == 65 and
  .candidatesMissingReadingKey == 0 and .top1Hits >= $minimumTop1 and
  .top5Hits >= $minimumTop5 and .mustPassTop1Hits == 2 and
  .mustPassTop1Cases == 2 and .stableCases == 13 and (.failures | length) <= 2
' "$REPORT" >/dev/null || {
  jq -c '.failures[]' "$REPORT" >&2
  exit 7
}
[[ "$HEAVY_STATUS" -eq 0 ]] || { jq -c '.failures[]' "$REPORT" >&2; exit "$HEAVY_STATUS"; }
jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 10 and .repetitions == 5 and
  .attemptedSessions == 50 and .createdSessions == 50 and .executedSessions == 50 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 10 and .top5Hits == 10 and
  .mustPassTop1Hits == 10 and .mustPassTop1Cases == 10 and .stableCases == 10 and
  (.failures | length) == 0
' "$REGISTERED_ASCII_REPORT" >/dev/null || {
  jq -c '.failures[]' "$REGISTERED_ASCII_REPORT" >&2
  exit 7
}
[[ "$REGISTERED_ASCII_STATUS" -eq 0 ]] || {
  jq -c '.failures[]' "$REGISTERED_ASCII_REPORT" >&2
  exit "$REGISTERED_ASCII_STATUS"
}

TOP1="$(jq -r '.top1Hits' "$REPORT")"
TOP5="$(jq -r '.top5Hits' "$REPORT")"
echo "q3b2_heavy_garble_replay=PASS top1=$TOP1/13 minimum_top1=$MINIMUM_TOP1/13 top5=$TOP5/13 minimum_top5=$MINIMUM_TOP5/13 stable=13/13 english_top1=2/2 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA"
echo "q3b2_registered_ascii_replay=PASS cases=10 sessions=50 top1=10/10 stable=10/10 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_REGISTERED_ASCII_SHA"

if [[ -n "${AIM_Q3B2_REPORT_OUT:-}" ]]; then
  mkdir -p "$(dirname "$AIM_Q3B2_REPORT_OUT")"
  cp "$REPORT" "$AIM_Q3B2_REPORT_OUT"
fi
