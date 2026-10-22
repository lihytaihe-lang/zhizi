#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
TARGET="native/core/agent-ime-core/src/rime.rs"
MUTATION="quality/mutations/required-segment-regular-selection.patch"
OUTPUT_DIR="$ROOT_DIR/build/test-results"
LOG="$OUTPUT_DIR/learning-integrity-mutation.log"

git diff --quiet -- "$TARGET" || {
  echo "mutation verification requires an unmodified $TARGET" >&2
  exit 2
}
mkdir -p "$OUTPUT_DIR"
restored=false
restore_source() {
  if [[ "$restored" == false ]]; then
    git apply --reverse "$MUTATION"
    restored=true
  fi
}
trap restore_source EXIT

git apply --check "$MUTATION"
git apply "$MUTATION"
set +e
./script/test_learning_degradation.sh >"$LOG" 2>&1
status=$?
set -e
restore_source

if [[ "$status" -eq 0 ]]; then
  echo "learning_integrity_mutation=FAIL gate accepted old regular-selection behavior" >&2
  exit 1
fi
grep -F "learning_degradation_gate=FAIL unauthorized_persistent_learning" "$LOG" >/dev/null || {
  echo "learning_integrity_mutation=FAIL gate failed for an unexpected reason" >&2
  tail -40 "$LOG" >&2
  exit 1
}
failure_line="$(grep -F "learning_degradation_gate=FAIL unauthorized_persistent_learning" "$LOG" | tail -1)"
echo "learning_integrity_mutation=PASS mutation=required-segment-regular-selection gate_exit=$status error=\"$failure_line\""
