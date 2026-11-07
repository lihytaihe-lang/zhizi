#!/bin/bash
# Public calibration of the C2/C3 thresholds for one ONNX model: eval first-choice on a public case file,
# one run per setting.   scripts/eval/calib_sweep.sh <onnx> <cases.jsonl> <outdir> "<label>|ENV=VAL ENV=VAL" ...
# Empty user directory (nothing private), Zhixu on, the shipped public lexicon.
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
onnx="$1"; cases="$2"; out="$3"; shift 3
export ZHIZI_SHARED="${ZHIZI_SHARED:-$HOME/Library/Input Methods/Agent Input Method.app/Contents/Resources/Rime}"
export ZHIZI_ZHIXU_WHOLE=1 ZHIZI_EVAL_REQUIRE_DECODER=zhixu ZHIZI_EVAL_OUT="$out"
mkdir -p "$out"
for spec in "$@"; do
  label="${spec%%|*}"; envs="${spec#*|}"
  ( env $envs ZHIZI_MODEL_OVERRIDE="$onnx" "$ROOT/scripts/eval/run_eval.sh" "$label" "$cases" > "$out/$label.log" 2>&1
    echo "$label $envs $(tail -1 "$out/$label.log" | tr '\t' ' ')" >> "$out/summary.txt" ) &
  while [ "$(jobs -r | wc -l)" -ge 3 ]; do sleep 5; done
done
wait
