#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_EVENTS="/Users/$(/usr/bin/id -un)/Library/Application Support/AgentInputMethod/Quality/Frozen/events-2026-07-24.jsonl"
EVENTS="${AIM_OWNER_QUALITY_EVENTS:-$DEFAULT_EVENTS}"
REPORT="$ROOT_DIR/quality/corpus/learning-cleanup-round5-impact-v1.json"
EXPECTED_REPORT_SHA="57d5646ab5411d6a790630727b223e099686353fff9d37a0d7a92370602912c0"

[[ -f "$EVENTS" && ! -L "$EVENTS" ]] || {
  echo "missing regular owner-private quality history required by the ROUND-5 cleanup gate: $EVENTS" >&2
  exit 2
}
actual_report_sha="$(shasum -a 256 "$REPORT" | awk '{print $1}')"
[[ "$actual_report_sha" == "$EXPECTED_REPORT_SHA" ]] || {
  echo "ROUND-5 learning cleanup impact checksum mismatch: $actual_report_sha" >&2
  exit 1
}
if rg -n '"(recordedAtMs|precedingText|appHash|activeProject|activePacks)"' "$REPORT"; then
  echo "ROUND-5 learning cleanup impact contains a privacy-forbidden field" >&2
  exit 1
fi
jq -e '
  .sourceEvents == 12252
  and .uniqueReadingsReplayed == 4054
  and .eligibleCohortReadings == 190
  and .eligibleCohortIncidents == 203
  and .cleanupEntriesDeletedOnPrivateCopy == 149
  and .undoRestoredLogicalDigest
  and .benefit.denominator == 875
  and .benefit.prevented == 7
  and .cost.denominator == 11331
  and .cost.regressed == 3
  and .netPreventedMinusRegressed == 4
  and ([.tiers[] | [.metric,.before,.after]] == [
    ["redo-after-commit",96,96],
    ["abandoned",46,46],
    ["backspace-rework",84,84],
    ["non-first-selection",875,868]
  ])
' "$REPORT" >/dev/null

cd "$ROOT_DIR"
# The replay copies the owner's live UserDB, which changes whenever the owner types, so it
# can only reproduce the July report while the IME is unused. The frozen report's checksum
# and invariants above are the gate; the live replay is opt-in (owner approved 2026-10-09).
if [[ "${AIM_OWNER_LIVE_REPLAY:-0}" == "1" ]]; then
  AIM_LEARNING_CLEANUP_IMPACT_MODE=check \
    AIM_LEARNING_CLEANUP_IMPACT_REPORT="$REPORT" \
    ./script/probe_learning_cleanup_impact.sh "$EVENTS"
else
  echo "learning_cleanup_round5_impact_live_replay=SKIP reason=owner UserDB is live (set AIM_OWNER_LIVE_REPLAY=1 to replay)"
fi
echo "learning_cleanup_round5_impact_contract=PASS benefit=7/875 regressions=3/11331 net=4 report_sha256=$actual_report_sha"
