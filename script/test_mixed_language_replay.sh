#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CONTEXT_CORPUS="$ROOT_DIR/quality/corpus/mixed-language-context-v2.json"
CONTEXT_CORPUS_SHA="588ddecfae4188e5485b31dc7e69a43bbe5856c0c1d51a9adc02d74ff524c004"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }
[[ "$(shasum -a 256 "$CONTEXT_CORPUS" | awk '{print $1}')" == "$CONTEXT_CORPUS_SHA" ]] || {
  echo "mixed-language context corpus checksum mismatch" >&2
  exit 6
}

AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
[[ -x "$HARNESS" ]] || { echo "missing replay harness: $HARNESS" >&2; exit 2; }

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-mixed-language.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
CONTEXT_REPORT="$TEMP_ROOT/context-report.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" \
  "$ROOT_DIR/quality/corpus/mixed-language-regression-v2.json" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 8 and .repetitions == 5 and
  .attemptedSessions == 40 and .createdSessions == 40 and .executedSessions == 40 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 8 and .top5Hits == 8 and
  .mustPassTop1Hits == 8 and .mustPassTop1Cases == 8 and .stableCases == 8 and
  (.failures | length) == 0 and
  (.observations | map({key: .query, value: .actual[0]}) | from_entries) as $first |
  $first.codex == "Codex" and $first.claude == "Claude" and
  $first.github == "GitHub" and $first.api == "API" and
  $first.token == "token" and $first.python == "Python" and
  $first.nihao == "你好" and $first.zhongguo == "中国"
' "$REPORT" >/dev/null

DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/context-user" \
  "$CONTEXT_CORPUS" "$CONTEXT_REPORT" 5 "$ARTIFACT_SHA" >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 8 and .repetitions == 5 and
  .attemptedSessions == 40 and .createdSessions == 40 and .executedSessions == 40 and
  .candidatesMissingReadingKey == 0 and .top1Hits == 8 and .top5Hits == 8 and
  .mustPassTop1Hits == 8 and .mustPassTop1Cases == 8 and .stableCases == 8 and
  (.failures | length) == 0 and
  (.observations | map({key: .id, value: .actual[0]}) | from_entries) as $first |
  $first["code-term-001"] == "ProductInputs" and $first["code-term-002"] == "Swift" and
  $first["code-term-003"] == "Claude" and $first["code-pinyin-001"] == "你好" and
  $first["code-pinyin-002"] == "中国" and $first["prose-pinyin-001"] == "你好" and
  $first["unknown-pinyin-001"] == "中国" and $first["override-term-001"] == "ProductInputs"
' "$CONTEXT_REPORT" >/dev/null

echo "mixed_language_replay=PASS cases=8 sessions=40 top1=8/8 stable=8/8"
jq -c '.observations[] | {id, query, candidates: .actual}' "$CONTEXT_REPORT"
echo "mixed_language_context_replay=PASS cases=8 sessions=40 top1=8/8 stable=8/8 corpus_checksum=$CONTEXT_CORPUS_SHA"
if [[ -n "${AIM_MIXED_LANGUAGE_REPORT_OUT:-}" ]]; then
  mkdir -p "$(dirname "$AIM_MIXED_LANGUAGE_REPORT_OUT")"
  cp "$REPORT" "$AIM_MIXED_LANGUAGE_REPORT_OUT"
fi
