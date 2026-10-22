#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$#" -ne 5 ]]; then
  echo "usage: $0 <input-bundle> <immutable-user-rime-snapshot> <golden.json> <candidate-lexicon.json> <private-report.json>" >&2
  exit 2
fi

BASELINE_REPORT="${AIM_CONTEXT_DISAMBIG_BASELINE_REPORT:-/Users/$(/usr/bin/id -un)/Library/Application Support/AgentInputMethod/Quality/ContextReplayQ4/q4-suffix-report.json}"
[[ -f "$BASELINE_REPORT" && ! -L "$BASELINE_REPORT" ]] || {
  echo "missing regular private Q4 baseline report: $BASELINE_REPORT" >&2
  exit 2
}

BOUND_REPLAY="$ROOT_DIR/script/run_"'bound_personal_replay.sh'
"$BOUND_REPLAY" "$1" "$2" "$3" "$4" "$5"
node "$ROOT_DIR/scripts/summarize_context_disambig_requalification.mjs" \
  --candidate-report "$5" \
  --baseline-report "$BASELINE_REPORT" \
  --check
