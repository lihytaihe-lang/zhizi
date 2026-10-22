#!/usr/bin/env bash
# Reproduce the ROUND-5 cohort, clean/owner qualification, grouped cleanup, and
# full before/after impact on private copies. The live UserDB is never passed
# to a harness or delete operation.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVENTS="${1:-${HOME}/Library/Application Support/AgentInputMethod/Quality/Frozen/events-2026-07-24.jsonl}"
OUTPUT_DIR="${2:-${TMPDIR:-/tmp}/agent-ime-learning-cleanup-round5}"
PROBE="$ROOT_DIR/quality/corpus/learning-cleanup-round5-probe-v1.json"
REPORT="$ROOT_DIR/quality/corpus/learning-cleanup-round5-impact-v1.json"

[[ -f "$EVENTS" && ! -L "$EVENTS" ]] || {
  echo "missing regular owner-private quality history: $EVENTS" >&2
  exit 2
}
mkdir -p "$OUTPUT_DIR"

cd "$ROOT_DIR"
node scripts/generate_learning_cleanup_round5_evidence.mjs --write-probe
./script/probe_short_input_learning.sh "$OUTPUT_DIR/arms" "$PROBE"
node scripts/generate_learning_cleanup_round5_evidence.mjs \
  --comparison "$OUTPUT_DIR/arms/comparison.json" --write
AIM_LEARNING_CLEANUP_IMPACT_MODE=write \
  AIM_LEARNING_CLEANUP_IMPACT_REPORT="$REPORT" \
  ./script/probe_learning_cleanup_impact.sh "$EVENTS"
node scripts/generate_learning_cleanup_round5_evidence.mjs --verify-frozen
