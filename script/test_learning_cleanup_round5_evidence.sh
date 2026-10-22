#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROBE="$ROOT_DIR/quality/corpus/learning-cleanup-round5-probe-v1.json"
EVIDENCE="$ROOT_DIR/quality/corpus/learning-cleanup-evidence-v3.json"
EXPECTED_PROBE_SHA="99abfe33e160fa79cca597e3448bc59ea1cbfd49304222ce5c98a2b3debc94ec"
EXPECTED_EVIDENCE_SHA="86cef3c620154762deaa0dcf899b2d88b9983b2976c68ec0ced911ba482d18cf"

actual_probe_sha="$(shasum -a 256 "$PROBE" | awk '{print $1}')"
actual_evidence_sha="$(shasum -a 256 "$EVIDENCE" | awk '{print $1}')"
[[ "$actual_probe_sha" == "$EXPECTED_PROBE_SHA" ]] || {
  echo "ROUND-5 learning cleanup probe checksum mismatch: $actual_probe_sha" >&2
  exit 1
}
[[ "$actual_evidence_sha" == "$EXPECTED_EVIDENCE_SHA" ]] || {
  echo "ROUND-5 learning cleanup evidence checksum mismatch: $actual_evidence_sha" >&2
  exit 1
}
if rg -n '"(recordedAtMs|precedingText|appHash|activeProject|activePacks)"' \
  "$PROBE" "$EVIDENCE"; then
  echo "ROUND-5 learning cleanup evidence contains a privacy-forbidden field" >&2
  exit 1
fi
jq -e '
  .round5Cohort.sourceReadings == 197
  and .round5Cohort.sourceIncidents == 210
  and .round5Cohort.eligibleReadings == 190
  and .round5Cohort.eligibleIncidents == 203
  and .round5Cohort.ownerEntriesUnchanged
' "$EVIDENCE" >/dev/null
cd "$ROOT_DIR"
node scripts/generate_learning_cleanup_round5_evidence.mjs --verify-frozen
echo "learning_cleanup_round5_evidence_contract=PASS cohort=210 eligible=203 privacy=PASS probe_sha256=$actual_probe_sha evidence_sha256=$actual_evidence_sha"
