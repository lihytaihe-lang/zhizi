#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Importing zhizi modules (eval_owner -> model) must not drop __pycache__ .pyc
# files under scripts/context_model: rc verify-inputs forbids untracked inputs.
export PYTHONDONTWRITEBYTECODE=1
python3 "$ROOT_DIR/scripts/context_model/test_copy_bundle_model.py"
python3 "$ROOT_DIR/scripts/context_model/test_quality_metadata_compatibility.py"
TEMP_ROOT="$(mktemp -d /tmp/agent-ime-context-model-gate.XXXXXX)"
trap 'rm -rf "$TEMP_ROOT"' EXIT
python3 "$ROOT_DIR/scripts/context_model/copy_bundle_model.py" "$TEMP_ROOT/ContextModel"
if ! python3 -c 'import json,sys; sys.exit(not json.load(open(sys.argv[1]))["available"])' "$TEMP_ROOT/ContextModel/availability.json"; then
  # AIM_CONTEXT_MODEL_REQUIRED=1 (set by script/test_all.sh) turns a missing
  # model into a gate failure: inside the recorded suite a SKIP-with-exit-0
  # would be reported as PASS, silently dropping the alignment evidence.
  # Running this script directly keeps the original optional behavior.
  if [[ "${AIM_CONTEXT_MODEL_REQUIRED:-0}" == "1" ]]; then
    echo 'context_model_alignment=FAIL reason=model-required-but-unavailable'
    echo 'context_model_rerank_performance=FAIL reason=model-required-but-unavailable'
    exit 3
  fi
  echo 'context_model_alignment=SKIP reason=model-files-missing-or-sha256-mismatch'
  echo 'context_model_rerank_performance=SKIP reason=model-files-missing-or-sha256-mismatch'
  exit 0
fi
ML="${ZHIZI_WORK:-$ROOT_DIR/ime_context_model}"
PYTHON="${AIM_CONTEXT_MODEL_PYTHON:-$ML/.venv/bin/python}"
[[ -x "$PYTHON" ]] || { echo 'context_model_alignment=FAIL reason=missing-Python-reference-runtime' >&2; exit 2; }
if [[ -n "${AIM_CONTEXT_MODEL_HARNESS:-}" ]]; then
  HARNESS="$AIM_CONTEXT_MODEL_HARNESS"
else
  swift build --package-path "$ROOT_DIR/native/macos" -c release --disable-automatic-resolution --product AgentIMEContextModelHarness
  BIN_DIR="$(swift build --package-path "$ROOT_DIR/native/macos" -c release --disable-automatic-resolution --show-bin-path)"
  HARNESS="$BIN_DIR/AgentIMEContextModelHarness"
fi
DYLD_LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib" "$PYTHON" \
  "$ROOT_DIR/scripts/context_model/zhizi/check_host_alignment.py" \
  --harness "$HARNESS" --directory "$TEMP_ROOT/ContextModel" \
  --out "$ROOT_DIR/build/model-integration-c1-handoff/alignment-public.json"
# Typing has pauses. After about a second idle the Neural Engine clocks down and the next
# prediction took 18-25 ms in C1, so back-to-back latency alone overstated how often the
# model ran. Replay keys with real gaps through the production loader and keep-warm path.
DYLD_LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib" "$HARNESS" --paced "$TEMP_ROOT/ContextModel" "$TEMP_ROOT/paced.json"
python3 - "$TEMP_ROOT/paced.json" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))
ms = sorted(r["elapsedNanoseconds"] / 1e6 for r in rows)
n = len(ms)
effective = sum(1 for x in ms if x <= 50) / n   # applied: on budget or late but under the hard cap
on_budget = sum(1 for x in ms if x <= 8) / n
cold = sum(1 for r in rows if r["cold"])
p95 = ms[min(n - 1, int(n * 0.95))]
ok = effective >= 0.95
print(f"context_model_paced_rerank={'PASS' if ok else 'FAIL'} keys={n} effective={effective:.3f} "
      f"on_budget={on_budget:.3f} cold_starts={cold} p50_ms={ms[n // 2]:.2f} p95_ms={p95:.2f} max_ms={ms[-1]:.2f}")
sys.exit(0 if ok else 1)
PY
