#!/bin/bash
# Per-key latency on the owner's real path, offline (Q12 / latency-gate tier "owner path").
#   scripts/eval/owner_path_latency.sh [cases.jsonl] [max-cases] [label]
# Runs the host eval example on a copy of the owner's Rime user directory (live), with the
# Zhixu switch and Zhixu learning on, the personal model files and the owner's settled
# choices, and prints p50/p95/p99 of the wall time of every typed key (ZHIZI_EVAL_KEY_MS).
# The empty-directory, no-personal-model gate (script/test_performance_controller_path.sh)
# does not measure this path.  The eval host scores with ONNX, the app with Core ML, so the
# model share here is only indicative; scripts/eval/key_stages.py reads the real app's
# stages.  Local only: owner data stays under eval-runs/.
# Build first: (cd native/host && cargo build --release --example eval)
set -eu
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CASES="${1:-$ROOT/eval-runs/owner-replay-v1/cases.jsonl}"
MAX="${2:-1500}"
LABEL="${3:-owner-path-$(git -C "$ROOT" rev-parse --short HEAD)}"
OUT="${ZHIZI_EVAL_OUT:-$ROOT/eval-runs/latency}"; mkdir -p "$OUT"
SUBSET="$OUT/cases-$LABEL.jsonl"
head -n "$MAX" "$CASES" > "$SUBSET"
KEYS="$OUT/keys-$LABEL.tsv"
export ZHIZI_EVAL_OUT="$OUT"
export ZHIZI_ZHIXU_WHOLE=1 ZHIZI_LEARNING=zhixu ZHIZI_EVAL_REQUIRE_DECODER=zhixu
export ZHIZI_EVAL_KEY_MS="$KEYS"
RECORDS="${ZHIZI_OWNER_RECORDS:-$HOME/Library/Application Support/AgentInputMethod/Quality/events.jsonl}"
[ -f "$RECORDS" ] && export ZHIZI_OWNER_RECORDS="$RECORDS"
"$ROOT/scripts/eval/run_eval.sh" "$LABEL" "$SUBSET" live > "$OUT/run-$LABEL.log" 2>&1 || { tail -20 "$OUT/run-$LABEL.log" >&2; exit 1; }
python3 -I - "$KEYS" <<'EOF'
import sys
values = sorted(float(line.rsplit("\t", 1)[1]) for line in open(sys.argv[1]))
def pct(p): return values[min(len(values) - 1, int(len(values) * p))]
print(f"owner_path_keys n={len(values)} p50_ms={pct(.5):.2f} p95_ms={pct(.95):.2f} p99_ms={pct(.99):.2f} max_ms={values[-1]:.2f}")
EOF
