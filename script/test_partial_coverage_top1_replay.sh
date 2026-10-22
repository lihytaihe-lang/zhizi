#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/partial-coverage-top1-v1.json"
FROZEN_CORPUS_SHA="f13db46c21d8b945b0cd69c507d14163eb8d85e821d4f06dac46c39712fc4c2d"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "partial-coverage corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}
jq -e '
  (.cases | length) == 9 and all(.cases[]; .mustPassTop1 == true) and
  ([.cases[] | select(.expectedSet != null)] | length) == 3 and
  ([.cases[] | select(.category == "strict-prefix-abbreviation-regression")] | length) == 1 and
  ([.cases[] | select(.category == "registered-ascii-control")] | length) == 3
' "$CORPUS" >/dev/null

touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-partial-coverage.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"
OWNER_RIME="${AGENT_IME_OWNER_RIME_DIR:-$HOME/Library/Application Support/AgentInputMethod/Rime}"
[[ -d "$OWNER_RIME" && ! -L "$OWNER_RIME" ]] || {
  echo "missing owner Rime directory required by frozen owner-real cases: $OWNER_RIME" >&2
  exit 2
}
ditto "$OWNER_RIME" "$TEMP_ROOT/owner-rime"

DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/owner-rime" "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 9 and .repetitions == 5 and
  .attemptedSessions == 45 and .createdSessions == 45 and .executedSessions == 45 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 9 and .top5Hits == 9 and
  .mustPassTop1Hits == 9 and .mustPassTop1Cases == 9 and .stableCases == 9 and
  (.failures | length) == 0
' "$REPORT" >/dev/null

echo "partial_coverage_top1_replay=PASS source=owner-rime-copy cases=9 sessions=45 top1=9/9 sets=3/3 prefixes=1/1 ascii=3/3 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA"
