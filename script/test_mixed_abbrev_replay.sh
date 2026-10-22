#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
CORPUS="$ROOT_DIR/quality/corpus/mixed-abbrev-regression.json"
FROZEN_CORPUS_SHA="2c866db6edf442fc722368d2714bd80906a77d83ac120810efc44cb50db7d419"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }

ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "mixed-abbreviation corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}

BIN_DIR="$(AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEReplayHarness"
[[ -x "$HARNESS" ]] || { echo "missing replay harness: $HARNESS" >&2; exit 2; }

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-mixed-abbrev.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
REPORT="$TEMP_ROOT/report.json"
ARTIFACT_SHA="$($ROOT_DIR/script/artifact_checksum.sh "$BUNDLE")"
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user" \
  "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" \
  >/dev/null

jq -e --arg checksum "$ARTIFACT_SHA" '
  .artifactChecksum == $checksum and .cases == 4 and .repetitions == 5 and
  .attemptedSessions == 20 and .createdSessions == 20 and .executedSessions == 20 and
  .candidatesMissingReadingKey == 0 and .top5Hits == 4 and
  .mustPassTop1Hits == 1 and .mustPassTop1Cases == 1 and .stableCases == 4 and
  (.failures | length) == 0 and
  (.observations | map({key: .query, value: .actual}) | from_entries) as $actual |
  $actual.dansh[0] == "但是" and
  ($actual.gengjai | index("更加")) != null and
  ($actual.zhem | index("这么")) != null and
  ($actual.zenm | index("怎么")) != null
' "$REPORT" >/dev/null

jq -c '.observations[] | {query, candidates: .actual}' "$REPORT"
echo "mixed_abbrev_replay=PASS cases=4 sessions=20 top5=4/4 must_top1=1/1 stable=4/4 artifact_checksum=$ARTIFACT_SHA corpus_checksum=$ACTUAL_CORPUS_SHA"
if [[ -n "${AIM_MIXED_ABBREV_REPORT_OUT:-}" ]]; then
  mkdir -p "$(dirname "$AIM_MIXED_ABBREV_REPORT_OUT")"
  cp "$REPORT" "$AIM_MIXED_ABBREV_REPORT_OUT"
fi
