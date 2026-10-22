#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_EVENTS="/Users/$(/usr/bin/id -un)/Library/Application Support/AgentInputMethod/Quality/Frozen/events-2026-07-24.jsonl"
EVENTS="${AIM_OWNER_QUALITY_EVENTS:-$DEFAULT_EVENTS}"
PROBE="$ROOT_DIR/quality/corpus/owner-error-ranking-full-probe-v1.json"
RANKING="$ROOT_DIR/quality/corpus/owner-error-ranking-full-v1.json"
EXPECTED_PROBE_SHA="43d6da0c15d050f17c9db81dc658465ea1abc638e27385658e516e6bbe2caf72"
EXPECTED_RANKING_SHA="8b1725e79adedeafaad942c147e98d96b71998ab4c103b0a299633753ecdcd7c"

[[ -f "$EVENTS" && ! -L "$EVENTS" ]] || {
  echo "missing regular owner-private quality history required by the full ranking gate: $EVENTS" >&2
  exit 2
}

actual_probe_sha="$(shasum -a 256 "$PROBE" | awk '{print $1}')"
actual_ranking_sha="$(shasum -a 256 "$RANKING" | awk '{print $1}')"
[[ "$actual_probe_sha" == "$EXPECTED_PROBE_SHA" ]] || {
  echo "owner full-error probe checksum mismatch: $actual_probe_sha" >&2
  exit 1
}
[[ "$actual_ranking_sha" == "$EXPECTED_RANKING_SHA" ]] || {
  echo "owner full-error ranking checksum mismatch: $actual_ranking_sha" >&2
  exit 1
}

if rg -n '"(recordedAtMs|precedingText|appHash|activeProject|activePacks)"' \
  "$PROBE" "$RANKING"; then
  echo "owner full-error ranking contains a privacy-forbidden field" >&2
  exit 1
fi

cd "$ROOT_DIR"
node scripts/generate_owner_error_ranking_full.mjs --verify-frozen
node scripts/generate_owner_error_ranking_full.mjs \
  --events "$EVENTS" --verify-source
echo "owner_error_ranking_full_contract=PASS incidents=875 readings=632 privacy=PASS probe_sha256=$actual_probe_sha ranking_sha256=$actual_ranking_sha"
