#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

PROBE="quality/corpus/owner-error-ranking-probe-v1.json"
RANKING="quality/corpus/owner-error-ranking-v1.json"
CLEANUP_EVIDENCE="quality/corpus/learning-cleanup-evidence-v2.json"
EXPECTED_PROBE_SHA="4b826a9c9218a1e5db0dca0e18696e6cec0885b51a269402852d157267524b83"
EXPECTED_RANKING_SHA="2b263f8b15a2f182ae2aa862115a052081516b7cfef3e96a9a4b29b6b8a27788"
EXPECTED_CLEANUP_SHA="cf41c7bccf01a5bebc198a55eedd4ae27afff9c1283b17b9f9be4faeb8ce2bbf"

actual_probe_sha="$(shasum -a 256 "$PROBE" | awk '{print $1}')"
actual_ranking_sha="$(shasum -a 256 "$RANKING" | awk '{print $1}')"
actual_cleanup_sha="$(shasum -a 256 "$CLEANUP_EVIDENCE" | awk '{print $1}')"
[[ "$actual_probe_sha" == "$EXPECTED_PROBE_SHA" ]] || {
  echo "owner error probe checksum mismatch: $actual_probe_sha" >&2
  exit 1
}
[[ "$actual_ranking_sha" == "$EXPECTED_RANKING_SHA" ]] || {
  echo "owner error ranking checksum mismatch: $actual_ranking_sha" >&2
  exit 1
}
[[ "$actual_cleanup_sha" == "$EXPECTED_CLEANUP_SHA" ]] || {
  echo "learning cleanup evidence checksum mismatch: $actual_cleanup_sha" >&2
  exit 1
}

if rg -n '"(recordedAtMs|precedingText|appHash|activeProject|activePacks)"' \
  "$RANKING" "$CLEANUP_EVIDENCE"; then
  echo "owner error ranking contains a privacy-forbidden raw field" >&2
  exit 1
fi

node scripts/generate_owner_error_ranking.mjs --verify-frozen
node scripts/generate_learning_cleanup_evidence.mjs --check
echo "owner_error_ranking_contract=PASS probe_sha256=$actual_probe_sha ranking_sha256=$actual_ranking_sha cleanup_sha256=$actual_cleanup_sha"
