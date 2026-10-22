#!/bin/bash
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
# Run synthesis in 1M-char batches (owner 2026-10-08: no single 20M run).
# Each batch: own dir, stats.json, check.json, DONE marker. Re-running skips DONE batches.
# Stops at the first batch whose quality check fails, so a bad batch never cascades.
set -u
ML="${ZHIZI_WORK:-$ROOT_DIR/ime_context_model}"
FIRST=${1:-1}; LAST=${2:-20}; WORKERS=${WORKERS:-qwen:8,glm:4}
cd $ML/src
for i in $(seq -f "%03g" $FIRST $LAST); do
  B=$ML/synth/v1/batch-$i
  [ -f $B/DONE ] && { echo "batch-$i already done"; continue; }
  rm -rf $B
  echo "=== batch-$i start $(date '+%F %T')"
  ../.venv/bin/python synth_generate.py --run v1/batch-$i --calls 900 --target-chars 1000000 \
      --workers $WORKERS --seed $((10#$i * 7919)) > $ML/synth/v1/batch-$i.log 2>&1
  if ../.venv/bin/python synth_check.py $B; then
    touch $B/DONE; echo "=== batch-$i done $(date '+%F %T')"
  else
    echo "=== batch-$i FAILED quality check - stopping"; exit 1
  fi
done
echo "=== all batches $FIRST-$LAST done"
