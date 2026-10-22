#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
HARNESS="${2:-}"
CORPUS="$ROOT_DIR/quality/corpus/lexicon-l1-regression-v1.json"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing input bundle: $BUNDLE" >&2; exit 2; }
[[ "$(shasum -a 256 "$CORPUS" | awk '{print $1}')" == "66412aa100a70c9b8f552bfd39677300b0cb6eb25d6d1f9b5529765b9fc7341a" ]] || { echo 'L1 regression checksum mismatch' >&2; exit 6; }
# An RC caller can supply its already-built, artifact-matched harness.
if [[ -z "$HARNESS" ]]; then
  touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
  AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --product AgentIMEReplayHarness >/dev/null
  BIN_DIR="$(swift build --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
  HARNESS="$BIN_DIR/AgentIMEReplayHarness"
fi
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-lexicon-l1.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
set +e
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" >/dev/null
STATUS=$?
set -e
if [[ -n "${AIM_LEXICON_L1_REPORT_OUT:-}" && -f "$REPORT" ]]; then
  mkdir -p "$(dirname "$AIM_LEXICON_L1_REPORT_OUT")"
  cp "$REPORT" "$AIM_LEXICON_L1_REPORT_OUT"
fi
[[ -f "$REPORT" ]] || { echo "lexicon_l1=FAIL reason=no-report status=$STATUS" >&2; exit 2; }
jq -c '.observations[] | {query, candidates: .actual}' "$REPORT"
[[ "$STATUS" == 0 ]] || { jq -c '.failures[]' "$REPORT" >&2; exit "$STATUS"; }
jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 13 and .repetitions == 5 and
  .attemptedSessions == 65 and .createdSessions == 65 and .executedSessions == 65 and
  .candidatesMissingReadingKey == 0 and .top5Hits == 13 and
  .mustPassTop1Hits == 9 and .mustPassTop1Cases == 9 and .stableCases == 13 and
  (.failures | length) == 0
' "$REPORT" >/dev/null
echo "lexicon_l1=PASS cases=13 top5=13/13 must_top1=9/9 stable=13/13 artifact_checksum=$ARTIFACT_SHA"
