#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/cn-en-mixed-narrow-v1.json"
CORPUS_SHA="4c3033c36b3124259185f7bde8f05e8b0198157b8e964ddbc216f43d48c0b1ca"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || {
  echo "missing release input bundle: $BUNDLE" >&2
  exit 2
}

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$CORPUS_SHA" ]] || {
  echo "CN-EN mixed narrow corpus checksum mismatch: expected $CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}

AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
[[ -x "$HARNESS" ]] || { echo "missing replay harness: $HARNESS" >&2; exit 2; }

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-cn-en-mixed-narrow.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"

DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 7 and .repetitions == 5 and
  .attemptedSessions == 35 and .createdSessions == 35 and .executedSessions == 35 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 7 and .top5Hits == 7 and
  .mustPassTop1Hits == 7 and .mustPassTop1Cases == 7 and .stableCases == 7 and
  (.failures | length) == 0 and
  (.observations | map({key: .id, value: .actual[0]}) | from_entries) as $first |
  $first["embedded-macbookpro"] == "当前这台MacBookPro啊" and
  $first["embedded-iphone"] == "当前这台iPhone啊" and
  $first["embedded-github"] == "当前这台GitHub啊" and
  $first["ascii-ansible"] == "Ansible" and
  $first["ascii-safari"] == "Safari" and
  $first["ascii-bun"] == "Bun" and
  $first["ascii-deno"] == "Deno"
' "$REPORT" >/dev/null

echo "cn_en_mixed_narrow_replay=PASS cases=7 sessions=35 top1=7/7 stable=7/7 corpus_checksum=$ACTUAL_CORPUS_SHA"
