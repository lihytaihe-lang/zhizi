#!/usr/bin/env bash
# Rebuild the privacy-safe 875-incident owner attribution from the frozen local
# quality history. The clean/owner replay only touches a private UserDB copy.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

EVENTS="${1:-${HOME}/Library/Application Support/AgentInputMethod/Quality/Frozen/events-2026-07-24.jsonl}"
OUTPUT_DIR="${2:-/private/tmp/agent-ime-owner-error-ranking-full}"
PROBE="$ROOT_DIR/quality/corpus/owner-error-ranking-full-probe-v1.json"

[[ -f "$EVENTS" && ! -L "$EVENTS" ]] || {
  echo "missing regular owner-private quality history: $EVENTS" >&2
  exit 2
}
mkdir -p "$OUTPUT_DIR"

node "$ROOT_DIR/scripts/generate_owner_error_ranking_full.mjs" \
  --events "$EVENTS" --write-probe
"$ROOT_DIR/script/probe_short_input_learning.sh" "$OUTPUT_DIR" "$PROBE"
node "$ROOT_DIR/scripts/generate_owner_error_ranking_full.mjs" \
  --events "$EVENTS" \
  --comparison "$OUTPUT_DIR/comparison.json" \
  --write
node "$ROOT_DIR/scripts/generate_owner_error_ranking_full.mjs" --verify-frozen
node "$ROOT_DIR/scripts/generate_owner_error_ranking_full.mjs" \
  --events "$EVENTS" --verify-source
