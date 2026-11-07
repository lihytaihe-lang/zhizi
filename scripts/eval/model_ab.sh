#!/bin/bash
# Same lexicon, same frozen owner data copy, two ONNX models: owner sample (live copy) and external set.
#   scripts/eval/model_ab.sh <labelA> <onnxA> <labelB> <onnxB>
# Needs eval-runs/live-snap (a frozen ditto of the owner Rime dir) and eval-runs/live-snap-events.jsonl.
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export ZHIZI_SHARED="${ZHIZI_SHARED:-$HOME/Library/Input Methods/Agent Input Method.app/Contents/Resources/Rime}"
export ZHIZI_ZHIXU_WHOLE=1 ZHIZI_LEARNING=zhixu ZHIZI_EVAL_REQUIRE_DECODER=zhixu ZHIZI_EVAL_OUT="$ROOT/eval-runs/ab"
export ZHIZI_LIVE_DIR="$ROOT/eval-runs/live-snap"
mkdir -p "$ZHIZI_EVAL_OUT"
while [ $# -ge 2 ]; do
  label="$1"; onnx="$2"; shift 2
  ZHIZI_MODEL_OVERRIDE="$onnx" ZHIZI_OWNER_RECORDS="$ROOT/eval-runs/live-snap-events.jsonl" \
    "$ROOT/scripts/eval/run_eval.sh" "own-$label" "$ROOT/eval-runs/owner-replay-v1/cases.jsonl" live > "$ZHIZI_EVAL_OUT/own-$label.log" 2>&1
  ZHIZI_MODEL_OVERRIDE="$onnx" "$ROOT/scripts/eval/run_eval.sh" "ext-$label" "$ROOT/eval-runs/external.jsonl" > "$ZHIZI_EVAL_OUT/ext-$label.log" 2>&1
done
