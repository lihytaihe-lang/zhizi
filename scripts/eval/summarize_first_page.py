#!/usr/bin/env python3
"""Summarize first-page hit rates from a ZhijuDeliveryReport (0.3.39).

CLI:
    python3 scripts/eval/summarize_first_page.py CASES REPORT OUTPUT

CASES is a JSON array of public objects: unique string id, lowercase input,
string expected, optional accepted list of strings (default [expected]) and
optional context, which is never echoed into the output. REPORT is a
ZhijuDeliveryReport JSON whose rows match the cases one-to-one in order,
identity of input and expected, and count. The summary reports the SHA256 of
both raw input files, the cases count, immediate/final top1 and top5 hit
counts against accepted, candidate-pool and scoring-union observation counts,
counts of both integrity checks, per-row hit booleans with a conservative
diagnostic category, and the standing limits.

Python standard library only.
"""

import argparse
import hashlib
import json
import sys

CANDIDATE_FIELDS = (
    "recalledCandidates",
    "immediatePage",
    "displayedPage",
    "groupScoredCandidates",
    "compositionScoredCandidates",
    "shortWordScoredCandidates",
)
INTEGRITY_FIELDS = ("pageMatchesCore", "digitSelectionMatchesDisplay")
OUTCOME_FIELDS = ("contextOutcome", "zhijuOutcome")
CASE_KEYS = frozenset(("id", "input", "expected", "accepted", "context",
                       "fuzzyMask", "typoCorrectionEnabled", "appIdentifier",
                       "selectionDigit", "shortAbbreviations", "fifthSlotSelection"))

CATEGORY_ALREADY_CORRECT = "already_correct"
CATEGORY_NOT_OBSERVED = "target_not_observed_in_pool"
CATEGORY_NOT_SCORED = "target_observed_but_not_scored"
CATEGORY_NOT_FIRST = "scored_target_not_first"
CATEGORIES = (
    CATEGORY_ALREADY_CORRECT,
    CATEGORY_NOT_OBSERVED,
    CATEGORY_NOT_SCORED,
    CATEGORY_NOT_FIRST,
)

LIMITS = (
    "scored_target_not_first does not distinguish scores, guards or timing",
    "context capture is not independently established by report rows",
    "cases/reports must be bound by runner for provenance",
)

SCHEMA = "first-page-summary/1"


class ValidationError(ValueError):
    pass


def _require_non_empty_str(value, label):
    if not isinstance(value, str) or value == "":
        raise ValidationError(f"{label} must be a non-empty string")
    return value


def validate_cases(cases):
    if not isinstance(cases, list):
        raise ValidationError("cases must be a JSON array of objects")
    if not cases:
        raise ValidationError("cases must not be empty")
    seen_ids = set()
    normalized = []
    for index, case in enumerate(cases):
        label = f"case #{index}"
        if not isinstance(case, dict):
            raise ValidationError(f"{label} must be an object")
        unknown = sorted(set(case) - CASE_KEYS)
        if unknown:
            raise ValidationError(f"{label} has unknown keys: {', '.join(unknown)}")
        case_id = _require_non_empty_str(case.get("id"), f"{label} id")
        if case_id in seen_ids:
            raise ValidationError(f"duplicate case id: {case_id}")
        seen_ids.add(case_id)
        label = f"case {case_id}"
        raw_input = _require_non_empty_str(case.get("input"), f"{label} input")
        if raw_input != raw_input.lower():
            raise ValidationError(f"{label} input must be lowercase")
        expected = _require_non_empty_str(case.get("expected"), f"{label} expected")
        if "accepted" in case:
            accepted = case["accepted"]
            if not isinstance(accepted, list) or not accepted:
                raise ValidationError(
                    f"{label} accepted must be a non-empty list of strings"
                )
            for entry in accepted:
                _require_non_empty_str(entry, f"{label} accepted entry")
            accepted = list(accepted)
        else:
            accepted = [expected]
        if expected not in accepted:
            raise ValidationError(f"{label} expected is missing from accepted")
        normalized.append(
            {"id": case_id, "input": raw_input, "expected": expected, "accepted": accepted}
        )
    return normalized


def validate_report(report, cases):
    if not isinstance(report, dict):
        raise ValidationError("report must be a JSON object")
    rows = report.get("rows")
    if not isinstance(rows, list):
        raise ValidationError("report must contain a rows array")
    if len(rows) != len(cases):
        raise ValidationError(
            f"report has {len(rows)} rows but cases count is {len(cases)}"
        )
    normalized = []
    for index, (row, case) in enumerate(zip(rows, cases)):
        label = f"row #{index} (case {case['id']})"
        if not isinstance(row, dict):
            raise ValidationError(f"{label} must be an object")
        if row.get("input") != case["input"]:
            raise ValidationError(f"{label} input does not match the case input")
        if row.get("expected") != case["expected"]:
            raise ValidationError(f"{label} expected does not match the case expected")
        entry = {
            "id": case["id"],
            "input": case["input"],
            "expected": case["expected"],
            "accepted": list(case["accepted"]),
        }
        for field in CANDIDATE_FIELDS:
            if field not in row:
                raise ValidationError(f"{label} is missing {field}")
            value = row[field]
            if not isinstance(value, list) or not all(
                isinstance(item, str) for item in value
            ):
                raise ValidationError(
                    f"{label} field {field} must be a list of strings"
                )
            entry[field] = list(value)
        for field in INTEGRITY_FIELDS:
            if field not in row:
                raise ValidationError(f"{label} is missing {field}")
            if not isinstance(row[field], bool):
                raise ValidationError(f"{label} field {field} must be a boolean")
            entry[field] = row[field]
        for field in OUTCOME_FIELDS:
            if field not in row:
                raise ValidationError(f"{label} is missing {field}")
            entry[field] = row[field]
        normalized.append(entry)
    return normalized


def summarize(cases, report, cases_sha256="", report_sha256=""):
    """Pure function: validated cases/report in, bounded summary dict out."""
    normalized_cases = validate_cases(cases)
    rows = validate_report(report, normalized_cases)
    hits = {
        "immediate_top1": 0,
        "immediate_top5": 0,
        "final_top1": 0,
        "final_top5": 0,
    }
    integrity = {field: {"true": 0, "false": 0} for field in INTEGRITY_FIELDS}
    diagnostics = {category: 0 for category in CATEGORIES}
    pool_observed = 0
    scored_observed = 0
    row_summaries = []
    for row in rows:
        accepted = set(row["accepted"])
        immediate = row["immediatePage"]
        display = row["displayedPage"]
        immediate_top1 = bool(immediate) and immediate[0] in accepted
        immediate_top5 = any(item in accepted for item in immediate[:5])
        final_top1 = bool(display) and display[0] in accepted
        final_top5 = any(item in accepted for item in display[:5])
        pool = set(row["recalledCandidates"]) | set(immediate) | set(display)
        scored = (
            set(row["groupScoredCandidates"])
            | set(row["compositionScoredCandidates"])
            | set(row["shortWordScoredCandidates"])
        )
        target = row["expected"]
        if final_top1:
            category = CATEGORY_ALREADY_CORRECT
        elif target not in pool and target not in scored:
            category = CATEGORY_NOT_OBSERVED
        elif target not in scored:
            category = CATEGORY_NOT_SCORED
        else:
            category = CATEGORY_NOT_FIRST
        hits["immediate_top1"] += immediate_top1
        hits["immediate_top5"] += immediate_top5
        hits["final_top1"] += final_top1
        hits["final_top5"] += final_top5
        for field in INTEGRITY_FIELDS:
            integrity[field]["true" if row[field] else "false"] += 1
        diagnostics[category] += 1
        pool_observed += target in pool
        scored_observed += target in scored
        row_summaries.append(
            {
                "id": row["id"],
                "immediate_top1": immediate_top1,
                "immediate_top5": immediate_top5,
                "final_top1": final_top1,
                "final_top5": final_top5,
                "category": category,
            }
        )
    return {
        "schema": SCHEMA,
        "inputs": {"cases_sha256": cases_sha256, "report_sha256": report_sha256},
        "cases_count": len(rows),
        "hits": hits,
        "observation": {
            "candidate_pool_observed": pool_observed,
            "scoring_union_observed": scored_observed,
        },
        "integrity": {
            "page_matches_core": integrity["pageMatchesCore"],
            "digit_selection_matches_display": integrity["digitSelectionMatchesDisplay"],
        },
        "diagnostics": diagnostics,
        "rows": row_summaries,
        "limits": list(LIMITS),
    }


def _load_json_with_sha256(path):
    try:
        with open(path, "rb") as handle:
            raw = handle.read()
    except OSError as exc:
        raise ValidationError(f"cannot read {path}: {exc}") from exc
    digest = hashlib.sha256(raw).hexdigest()
    try:
        data = json.loads(raw.decode("utf-8"))
    except ValueError as exc:
        raise ValidationError(f"cannot parse {path} as JSON: {exc}") from exc
    return data, digest


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Summarize first-page hits from a ZhijuDeliveryReport."
    )
    parser.add_argument("cases", help="path to the public CASES JSON array")
    parser.add_argument("report", help="path to the ZhijuDeliveryReport JSON")
    parser.add_argument("output", help="path to write the summary JSON")
    args = parser.parse_args(argv)
    try:
        cases, cases_sha256 = _load_json_with_sha256(args.cases)
        report, report_sha256 = _load_json_with_sha256(args.report)
        summary = summarize(
            cases,
            report,
            cases_sha256=cases_sha256,
            report_sha256=report_sha256,
        )
    except ValidationError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    try:
        with open(args.output, "w", encoding="utf-8") as handle:
            json.dump(summary, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
    except OSError as exc:
        print(f"error: cannot write {args.output}: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
