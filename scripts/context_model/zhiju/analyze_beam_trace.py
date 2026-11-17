#!/usr/bin/env python3
"""Offline beam-trace comparison diagnostic (stdlib only).

Compares two synthetic relaxed-budget trace JSON files and reports per-target
ranks, early-pruning evidence and classifications. Scope: offline diagnostic
only; not 40ms acceptance.
"""

import argparse
import hashlib
import json
import math
import sys
from pathlib import Path

SCOPE = "offline diagnostic only; not 40ms acceptance"
REQUIRED_BEAM = 8
MIN_DIAGNOSTIC_BUDGET_MS = 1000


class InputError(Exception):
    """Raised when an input file violates the trace contract."""


def _fail(message):
    raise InputError(message)


def _finite_number(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    if isinstance(value, float) and not math.isfinite(value):
        return None
    return value


def _is_string_list(value):
    return isinstance(value, list) and all(isinstance(item, str) for item in value)


def _recomputed_alive(expected, prefixes):
    return any(expected.startswith(prefix) for prefix in prefixes)


def _validate_row(row, label, index):
    if not isinstance(row, dict):
        _fail(f"{label}: row {index} must be an object")

    row_id = row.get("id")
    if not isinstance(row_id, str) or not row_id:
        _fail(f"{label}: row {index} 'id' must be a nonempty string")

    expected = row.get("expected")
    if not isinstance(expected, str) or not expected:
        _fail(f"{label}: row '{row_id}' 'expected' must be a nonempty string")

    beam = row.get("beam")
    if isinstance(beam, bool) or not isinstance(beam, int) or beam != REQUIRED_BEAM:
        _fail(f"{label}: row '{row_id}' 'beam' must be {REQUIRED_BEAM}")

    budget = _finite_number(row.get("diagnosticBudgetMS"))
    if budget is None or budget < MIN_DIAGNOSTIC_BUDGET_MS:
        _fail(
            f"{label}: row '{row_id}' 'diagnosticBudgetMS' must be a number "
            f">= {MIN_DIAGNOSTIC_BUDGET_MS}"
        )

    if not _is_string_list(row.get("candidates")):
        _fail(f"{label}: row '{row_id}' 'candidates' must be a list of strings")

    callbacks = row.get("callbacks")
    if not isinstance(callbacks, list) or not callbacks:
        _fail(f"{label}: row '{row_id}' 'callbacks' must be a nonempty list")

    steps = []
    for pos, callback in enumerate(callbacks):
        if not isinstance(callback, dict):
            _fail(f"{label}: row '{row_id}' callback {pos} must be an object")
        step = callback.get("step")
        if isinstance(step, bool) or not isinstance(step, int):
            _fail(f"{label}: row '{row_id}' callback {pos} 'step' must be an integer")
        prefixes = callback.get("prefixes")
        if not _is_string_list(prefixes) or not prefixes:
            _fail(f"{label}: row '{row_id}' callback {pos} 'prefixes' must be a nonempty list of strings")
        recorded = callback.get("targetPrefixAlive")
        if not isinstance(recorded, bool):
            _fail(f"{label}: row '{row_id}' callback {pos} 'targetPrefixAlive' must be a boolean")
        recomputed = _recomputed_alive(expected, prefixes)
        if recomputed != recorded:
            _fail(
                f"{label}: row '{row_id}' callback {pos} step {step} "
                f"'targetPrefixAlive' mismatch: recorded {recorded}, recomputed {recomputed}"
            )
        steps.append(step)

    if steps != list(range(len(callbacks))):
        _fail(f"{label}: row '{row_id}' callback 'step' values must be contiguous from 0")

    for field in ("searchMs", "modelMs"):
        if _finite_number(row.get(field)) is None or row[field] < 0:
            _fail(f"{label}: row '{row_id}' '{field}' must be a finite nonnegative number")

    return row_id, expected


def _load_trace(path, label):
    try:
        raw = path.read_bytes()
    except OSError as exc:
        _fail(f"{label}: cannot read {path}: {exc}")
    try:
        data = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        _fail(f"{label}: invalid JSON in {path}: {exc}")
    if not isinstance(data, dict):
        _fail(f"{label}: top-level value must be an object")
    rows = data.get("rows")
    if not isinstance(rows, list) or not rows:
        _fail(f"{label}: 'rows' must be a nonempty list")

    order = []
    by_id = {}
    for index, row in enumerate(rows):
        row_id, expected = _validate_row(row, label, index)
        if row_id in by_id:
            _fail(f"{label}: duplicate id '{row_id}'")
        order.append(row_id)
        by_id[row_id] = {"expected": expected, "row": row}
    return raw, order, by_id


def _target_rank(expected, candidates):
    try:
        return candidates.index(expected)
    except ValueError:
        return None


def _first_pruned_step(expected, callbacks):
    length = len(expected)
    steps = [
        callback["step"]
        for callback in callbacks
        if all(len(prefix) < length for prefix in callback["prefixes"])
        and not _recomputed_alive(expected, callback["prefixes"])
    ]
    return min(steps) if steps else None


def _classify(rank_before, rank_after, first_pruned_after):
    hit_before = rank_before is not None
    hit_after = rank_after is not None
    if hit_before and hit_after:
        return "retained"
    if hit_after:
        return "fixed"
    if hit_before:
        return "beam_regression" if first_pruned_after is not None else "search_miss"
    return "both_miss"


def _build_report(before_path, after_path):
    raw_before, order_before, before = _load_trace(before_path, "before")
    raw_after, order_after, after = _load_trace(after_path, "after")

    if set(order_before) != set(order_after):
        _fail(
            "id sets differ: before-only="
            f"{sorted(set(order_before) - set(order_after))} "
            f"after-only={sorted(set(order_after) - set(order_before))}"
        )
    for row_id in order_before:
        if before[row_id]["expected"] != after[row_id]["expected"]:
            _fail(f"row '{row_id}': 'expected' differs between before and after")

    result_rows = []
    fixes = []
    regressions = []
    for row_id in order_before:
        expected = before[row_id]["expected"]
        rank_before = _target_rank(expected, before[row_id]["row"]["candidates"])
        rank_after = _target_rank(expected, after[row_id]["row"]["candidates"])
        first_pruned_before = _first_pruned_step(expected, before[row_id]["row"]["callbacks"])
        first_pruned_after = _first_pruned_step(expected, after[row_id]["row"]["callbacks"])
        classification = _classify(rank_before, rank_after, first_pruned_after)
        if classification == "fixed":
            fixes.append(row_id)
        elif classification in ("beam_regression", "search_miss"):
            regressions.append(row_id)
        result_rows.append(
            {
                "id": row_id,
                "targetRankBefore": rank_before,
                "targetRankAfter": rank_after,
                "firstPrunedStepBefore": first_pruned_before,
                "firstPrunedStepAfter": first_pruned_after,
                "classification": classification,
            }
        )

    return {
        "scope": SCOPE,
        "count": len(result_rows),
        "fixes": fixes,
        "regressions": regressions,
        "beforeSHA256": hashlib.sha256(raw_before).hexdigest(),
        "afterSHA256": hashlib.sha256(raw_after).hexdigest(),
        "rows": result_rows,
    }


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Compare two offline beam-trace JSON files (diagnostic only)."
    )
    parser.add_argument("--before", required=True, type=Path, help="before trace JSON")
    parser.add_argument("--after", required=True, type=Path, help="after trace JSON")
    parser.add_argument("--out", required=True, type=Path, help="report path (must not exist)")
    args = parser.parse_args(argv)

    if args.out.exists():
        print(f"error: output '{args.out}' already exists; refusing to overwrite", file=sys.stderr)
        return 2

    try:
        report = _build_report(args.before, args.after)
    except InputError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    try:
        with args.out.open("x", encoding="utf-8") as handle:
            handle.write(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    except OSError as exc:
        print(f"error: cannot write '{args.out}': {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
