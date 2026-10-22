#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_EVENTS="/Users/$(/usr/bin/id -un)/Library/Application Support/AgentInputMethod/Quality/Frozen/events-2026-07-24.jsonl"
EVENTS="${AIM_OWNER_QUALITY_EVENTS:-$DEFAULT_EVENTS}"

[[ -f "$EVENTS" && ! -L "$EVENTS" ]] || {
  echo "missing regular owner-private quality history required by the active context gate: $EVENTS" >&2
  exit 2
}

cd "$ROOT_DIR"
node scripts/evaluate_owner_context_disambig.mjs --events "$EVENTS" --check
