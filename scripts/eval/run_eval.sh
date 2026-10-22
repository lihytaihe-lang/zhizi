#!/bin/bash
# Bucketed evaluation through the host (native/host/examples/eval.rs).
#   scripts/eval/run_eval.sh <label> <cases.jsonl> [nomodel|live]
# Writes $ZHIZI_EVAL_OUT/out-<label>.jsonl (default <repo>/eval-runs) and prints
# the per-bucket table. A fresh user directory each run; `live` copies the owner's
# Rime user directory instead (local only: owner data never leaves this Mac).
# Optional: ZHIZI_OWNER_RECORDS=<events.jsonl> loads the owner's settled choices.
# Build first: (cd native/host && cargo build --release --example eval)
set -u
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ROOT="$ROOT_DIR"
ML="${ZHIZI_WORK:-$ROOT_DIR/ime_context_model}"
OUT="${ZHIZI_EVAL_OUT:-${EVAL_OUT:-$ROOT_DIR/eval-runs}}"; mkdir -p "$OUT"
SHARED="${ZHIZI_SHARED:-$ROOT/build/native/release/Agent Input Method.inputmethod/Contents/Resources/Rime}"
U="$OUT/user-$1"; rm -rf "$U"
if [ "${3:-}" = live ]; then
  ditto "${ZHIZI_LIVE_DIR:-$HOME/Library/Application Support/AgentInputMethod/Rime}" "$U"; chmod -R u+rwX "$U"
else
  mkdir -p "$U"
fi
M="$ML/release"
export ZHIZI_EVAL_WAIT_TICKS="${ZHIZI_EVAL_WAIT_TICKS:-400}"  # 40 let the model answer late under load: identical resources gave different firsts
export DYLD_LIBRARY_PATH="${ZHIZI_RIME_LIB:-$ROOT/.build-deps/librime/1.16.1/lib}"
if [ "${3:-}" != nomodel ]; then
  export ZHIZI_ORT="$(ls "$ML"/.venv/lib/python3.12/site-packages/onnxruntime/capi/libonnxruntime.*.dylib | head -1)"
  export ZHIZI_MODEL="${ZHIZI_MODEL_OVERRIDE:-$M/zhizi-30m-v3-onnx/zhizi30m_v2.onnx}" ZHIZI_ITOS="$M/zhizi-30m-v3/itos.json"
fi
"$ROOT/native/host/target/release/examples/eval" "$SHARED" "$U" "$2" "$OUT/out-$1.jsonl"
