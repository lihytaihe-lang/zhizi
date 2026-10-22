#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/short-input-factory-top1-v1.json"
FROZEN_CORPUS_SHA="2d27435e62f42331b12e636a747ff5c120f657b6d759042ca30b2e64c9f7fc03"

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || {
  echo "missing release input bundle: $BUNDLE" >&2
  exit 2
}
ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "factory-top1 corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}
jq -e '
  .kind == "short-input-factory-top1-regression" and
  (.cases | length) == 20 and
  ([.cases[] | select(.id | startswith("single-"))] | length) == 16 and
  ([.cases[] | select(.id | startswith("screenshot-"))] | length) == 4 and
  all(.cases[]; (.allowed | length) == 1 and .mustPassTop1 == true)
' "$CORPUS" >/dev/null

touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-factory-top1.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" \
  "$HARNESS" "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/clean-user" \
  "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 20 and .repetitions == 5 and
  .attemptedSessions == 100 and .createdSessions == 100 and .executedSessions == 100 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 20 and .top5Hits == 20 and
  .mustPassTop1Hits == 20 and .mustPassTop1Cases == 20 and .stableCases == 20 and
  (.failures | length) == 0
' "$REPORT" >/dev/null

echo "short_input_factory_top1=PASS source=clean-user cases=20 single=16/16 strings=4/4 sessions=100 top1=20/20 stable=20/20 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA"
