#!/usr/bin/env bash
# Rebuild the privacy-safe owner error ranking from the frozen local quality
# history. Raw records and app/context fields never leave the private source.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

EVENTS="${1:-${HOME}/Library/Application Support/AgentInputMethod/Quality/Frozen/events-2026-07-24.jsonl}"
OUTPUT_DIR="${2:-/private/tmp/agent-ime-owner-error-ranking}"
PROBE="$ROOT_DIR/quality/corpus/owner-error-ranking-probe-v1.json"

[[ -f "$EVENTS" ]] || { echo "missing private quality history: $EVENTS" >&2; exit 2; }
mkdir -p "$OUTPUT_DIR"

"$ROOT_DIR/script/probe_short_input_learning.sh" "$OUTPUT_DIR" "$PROBE"
node "$ROOT_DIR/scripts/generate_owner_error_ranking.mjs" \
  --events "$EVENTS" \
  --comparison "$OUTPUT_DIR/comparison.json" \
  --write
node "$ROOT_DIR/scripts/generate_owner_error_ranking.mjs" --verify-frozen
