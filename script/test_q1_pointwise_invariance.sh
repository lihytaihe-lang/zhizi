#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/input-quality-q1-invariance-v1.json"
FROZEN_CORPUS_SHA="1f976556f990685af7320caa802630fa369193c155dc90af2d550d7134ab46a2"

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || {
  echo "missing release input bundle: $BUNDLE" >&2
  exit 2
}

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "Q1 pointwise corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}

jq -e '
  .version == 1 and .kind == "input-quality-q1-invariance-regression" and
  .baselineCommit == "b36f2f3" and (.cases | length) == 3 and
  all(.cases[];
    .category == "q1-uninvolved-pointwise-control" and
    (.expectedCandidates | type == "array" and length == 5) and
    .allowed == [.expectedCandidates[0]] and .mustPassTop1 == true)
' "$CORPUS" >/dev/null

touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-q1-invariance.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"

set +e
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null
HARNESS_STATUS=$?
set -e
if [[ "$HARNESS_STATUS" -ne 0 ]]; then
  jq -c '.failures[]' "$REPORT" >&2
  exit "$HARNESS_STATUS"
fi

jq -e --arg checksum "$ARTIFACT_SHA" --slurpfile fixture "$CORPUS" '
  ($fixture[0].cases | map({key: .id, value: .expectedCandidates}) | from_entries) as $expected |
  .artifactChecksum == $checksum and .cases == 3 and .repetitions == 5 and
  .attemptedSessions == 15 and .createdSessions == 15 and .executedSessions == 15 and
  .top1Hits == 3 and .top5Hits == 3 and .mustPassTop1Hits == 3 and
  .mustPassTop1Cases == 3 and .stableCases == 3 and (.failures | length) == 0 and
  (.observations | length) == 3 and
  all(.observations[]; .actual == $expected[.id])
' "$REPORT" >/dev/null

echo "q1_pointwise_invariance=PASS cases=3 sessions=15 stable=3/3 baseline_commit=b36f2f3 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA"
