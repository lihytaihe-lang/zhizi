#!/bin/bash
# Replay a frozen owner case set through two commits and compare them (G1).
#   scripts/eval/owner_replay.sh <old-commit> <new-commit> <cases-dir> [live|nomodel]
# For each commit: a worktree under tmp/replay-wt/<short> (created once, reused), built
# with `cargo build --release --offline --example eval`, then that worktree's run_eval.sh
# runs the cases into eval-runs/replay/<short>.  ZHIZI_WORK / ZHIZI_SHARED / ZHIZI_RIME_LIB /
# ZHIZI_LIVE_DIR / ZHIZI_MODEL_OVERRIDE pass through from the caller's environment.
# Local only: the case set and outputs are owner data and stay under eval-runs/.
set -eu

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OLD_COMMIT="${1:?usage: owner_replay.sh <old-commit> <new-commit> <cases-dir> [live|nomodel]}"
NEW_COMMIT="${2:?old commit missing}"
CASES_DIR="${3:?cases dir missing}"
MODE="${4:-}"
case "$MODE" in
  live|nomodel|"") ;;
  *) echo "unknown mode: $MODE (want live or nomodel)" >&2; exit 2 ;;
esac

OLD_SHORT="$(git -C "$ROOT" rev-parse --short "$OLD_COMMIT")"
NEW_SHORT="$(git -C "$ROOT" rev-parse --short "$NEW_COMMIT")"

run_one() {
  local short="$1" commit="$2" wt
  wt="$ROOT/tmp/replay-wt/$short"
  if [ ! -d "$wt" ]; then
    git -C "$ROOT" worktree add --detach "$wt" "$commit"
  fi
  if [ ! -e "$wt/.build-deps" ] && [ -e "$ROOT/.build-deps" ]; then
    ln -s "$ROOT/.build-deps" "$wt/.build-deps"
  fi
  local out="$ROOT/eval-runs/replay/$short${MODE:+-$MODE}"
  # The replay is deterministic (two runs of one build matched case for case, 2026-10-28), so a
  # finished run of this commit, mode and case count is reused instead of being overwritten.
  if [ -f "$out/out-$short.jsonl" ] && [ "$(wc -l < "$out/out-$short.jsonl")" -eq "$(wc -l < "$CASES_DIR/cases.jsonl")" ]; then
    echo "reusing $out/out-$short.jsonl"
    return
  fi
  (cd "$wt/native/host" && cargo build --release --offline --example eval)
  ZHIZI_EVAL_OUT="$out" "$wt/scripts/eval/run_eval.sh" "$short" "$CASES_DIR/cases.jsonl" "$MODE"
}

run_one "$OLD_SHORT" "$OLD_COMMIT"
run_one "$NEW_SHORT" "$NEW_COMMIT"

REPORT="$ROOT/eval-runs/replay/compare-$OLD_SHORT-$NEW_SHORT${MODE:+-$MODE}.md"
python3 "$ROOT/scripts/eval/owner_replay.py" compare --cases "$CASES_DIR" \
  --old "$ROOT/eval-runs/replay/$OLD_SHORT${MODE:+-$MODE}/out-$OLD_SHORT.jsonl" \
  --new "$ROOT/eval-runs/replay/$NEW_SHORT${MODE:+-$MODE}/out-$NEW_SHORT.jsonl" \
  --report "$REPORT"
