"""Read normalized ranking JSONL and report metrics without starting the engine."""
import argparse
import json
import math
from pathlib import Path
import sys

BUCKETS = ("full", "abbrev", "mixed", "typo", "english", "name")
DEFAULT_WEIGHTS = dict(zip(BUCKETS, (0.5, 0.25, 0.1, 0.1, 0.025, 0.025)))
METRICS = ("top1", "top5", "mrr", "pool_rate")
CASE_FIELDS = ("bucket", "raw", "context", "target", "source")


def validate_records(records):
    seen = set()
    for number, row in enumerate(records, 1):
        def invalid(field):
            raise ValueError(f"record {number}: invalid {field}")

        if not isinstance(row, dict):
            invalid("object")
        if not isinstance(row.get("id"), str) or not row["id"] or row["id"] in seen:
            invalid("id (must be nonempty and unique)")
        seen.add(row["id"])
        if row.get("bucket") not in BUCKETS:
            invalid("bucket")
        if row.get("source") not in ("ext", "owner_pre", "friend"):
            invalid("source")
        for field in ("raw", "context", "target"):
            if not isinstance(row.get(field), str) or (field != "context" and not row[field]):
                invalid(field)
        candidates = row.get("candidates")
        if not isinstance(candidates, list) or len(candidates) > 60 or any(not isinstance(text, str) or not text for text in candidates):
            invalid("candidates")
        rank = row.get("rank_all")
        if type(rank) is not int or rank < -1:
            invalid("rank_all")
        if rank >= 0 and rank < len(candidates):
            if candidates[rank] != row["target"] or row["target"] in candidates[:rank]:
                invalid("rank_all/candidates")
        elif row["target"] in candidates or (rank >= 0 and len(candidates) != 60):
            invalid("rank_all/candidates")
        if type(row.get("model_ready")) is not bool:
            invalid("model_ready")
        if "model_ms" not in row:
            invalid("model_ms (required)")
        ms = row.get("model_ms")
        if ms is not None and (type(ms) not in (int, float) or not math.isfinite(ms) or ms < 0):
            invalid("model_ms")
    return records


def load_records(path):
    records = []
    with Path(path).open(encoding="utf-8") as stream:
        for number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            try:
                records.append(json.loads(line))
            except (json.JSONDecodeError, ValueError) as exc:
                raise ValueError(f"line {number}: invalid JSON") from exc
    return validate_records(records)


def normalize_weights(weights):
    if not isinstance(weights, dict) or set(weights) != set(BUCKETS):
        raise ValueError("weights must specify exactly the six buckets")
    if any(type(value) not in (int, float) or not math.isfinite(value) or value < 0 for value in weights.values()):
        raise ValueError("weights must be finite nonnegative numbers")
    total = sum(weights.values())
    if not math.isfinite(total) or total <= 0:
        raise ValueError("weights must have a positive finite sum")
    return {bucket: weights[bucket] / total for bucket in BUCKETS}


def load_weights(path):
    with Path(path).open(encoding="utf-8") as stream:
        return normalize_weights(json.load(stream))


def model_readiness(records):
    records = list(records)
    scored = [row for row in records if row["model_ready"] is True or row.get("outcome") != "no_group"]
    total = len(scored)
    ready = sum(row["model_ready"] is True for row in scored)
    no_task = len(records) - total
    return {"total": total, "ready": ready, "rate": ready / total if total else 0,
            "no_task": no_task, "no_task_ratio": no_task / len(records) if records else 0,
            "passed": total > 0 and ready * 100 >= total * 98}


def bucket_metrics(rows):
    count = len(rows)
    if not count:
        return {"count": 0, **dict.fromkeys(METRICS)}
    ranks = [row["rank_all"] for row in rows]
    return {"count": count,
            "top1": sum(rank == 0 for rank in ranks) / count,
            "top5": sum(0 <= rank < 5 for rank in ranks) / count,
            "mrr": sum(1 / (rank + 1) if rank >= 0 else 0 for rank in ranks) / count,
            "pool_rate": sum(rank >= 0 for rank in ranks) / count}


def summarize(records, weights=None):
    records = validate_records(records)
    weights = normalize_weights(DEFAULT_WEIGHTS if weights is None else weights)
    buckets = {bucket: bucket_metrics([row for row in records if row["bucket"] == bucket]) for bucket in BUCKETS}
    coverage = sum(weights[bucket] for bucket in BUCKETS if buckets[bucket]["count"])
    if records and not coverage:
        raise ValueError("no populated bucket has positive weight")
    effective = {bucket: weights[bucket] / coverage if coverage and buckets[bucket]["count"] else 0 for bucket in BUCKETS}
    weighted = {metric: sum(effective[bucket] * buckets[bucket][metric] for bucket in BUCKETS if effective[bucket]) if coverage else None for metric in METRICS}
    return {"total": len(records), "buckets": buckets, "weights": weights,
            "effective_weights": effective, "weight_coverage": coverage,
            "missing_buckets": [bucket for bucket in BUCKETS if not buckets[bucket]["count"]],
            "weighted": weighted, "model_readiness": model_readiness(records)}


def compare_records(before, after):
    before = {row["id"]: row for row in validate_records(before)}
    after = {row["id"]: row for row in validate_records(after)}
    if set(before) != set(after):
        raise ValueError("comparison requires exactly the same case ids")
    by_bucket = {bucket: {"total": 0, "fixed": 0, "broken": 0, "unchanged": 0} for bucket in BUCKETS}
    for case_id, old in before.items():
        new = after[case_id]
        if any(old[field] != new[field] for field in CASE_FIELDS):
            raise ValueError("comparison case definition changed")
        counts = by_bucket[old["bucket"]]
        counts["total"] += 1
        if old["rank_all"] != 0 and new["rank_all"] == 0:
            counts["fixed"] += 1
        elif old["rank_all"] == 0 and new["rank_all"] != 0:
            counts["broken"] += 1
        else:
            counts["unchanged"] += 1
    result = {key: sum(counts[key] for counts in by_bucket.values()) for key in ("total", "fixed", "broken", "unchanged")}
    result.update(by_bucket=by_bucket,
                  model_readiness_before=model_readiness(list(before.values())),
                  model_readiness_after=model_readiness(list(after.values())))
    return result


def report_exit_code(summary, require_model=True, comparison=None):
    if not summary["total"]:
        return 1
    if require_model and (not summary["model_readiness"]["passed"] or
                          (comparison is not None and not comparison["model_readiness_before"]["passed"])):
        return 1
    return 0


def render_report(summary, comparison=None, require_model=True):
    readiness = summary["model_readiness"]
    if not summary["total"]:
        first = "\x1b[31mFAIL 无用例，不能计算有效评测结果\x1b[0m"
    elif not require_model:
        first = "无模型评测：模型就绪门不适用。"
    else:
        first = (f"模型就绪率 {readiness['rate']:.2%} ({readiness['ready']}/{readiness['total']})，"
                 f"无评分任务 {readiness['no_task']} 条（{readiness['no_task_ratio']:.2%}），门槛 98%")
        if comparison is not None:
            baseline = comparison["model_readiness_before"]
            first += (f"；基线 {baseline['rate']:.2%} ({baseline['ready']}/{baseline['total']}，"
                      f"无评分任务 {baseline['no_task']} 条)")
        first = "\x1b[31mFAIL " + first + "\x1b[0m" if report_exit_code(summary, require_model, comparison) else "PASS " + first
    lines = [first, "", "| bucket | 样本 | 首位 | Top5 | MRR | 池内率 | 有效权重 |",
             "| --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
    def percent(value):
        return "—" if value is None else f"{value:.2%}"

    for bucket in BUCKETS:
        values = summary["buckets"][bucket]
        lines.append(f"| {bucket} | {values['count']} | " + " | ".join(percent(values[metric]) for metric in METRICS) + f" | {summary['effective_weights'][bucket]:.2%} |")
    lines.append(f"| 加权总分 | {summary['total']} | " + " | ".join(percent(summary["weighted"][metric]) for metric in METRICS) + (" | 100% |" if summary["weight_coverage"] else " | — |"))
    lines.extend(["", f"原始权重覆盖率：{summary['weight_coverage']:.2%}。"])
    if summary["missing_buckets"]:
        lines.append("缺少桶：" + ", ".join(summary["missing_buckets"]) + "；加权分仅在有样本且权重非零的桶间归一化，不能视为完整分布总分。")
    if comparison is not None:
        lines.extend(["", f"对比基线：修好 {comparison['fixed']} / 弄坏 {comparison['broken']} / 首位正确性不变 {comparison['unchanged']}（配对 {comparison['total']}）。",
                      "", "| bucket | 配对 | 修好 | 弄坏 | 首位正确性不变 |", "| --- | ---: | ---: | ---: | ---: |"])
        for bucket in BUCKETS:
            counts = comparison["by_bucket"][bucket]
            lines.append(f"| {bucket} | {counts['total']} | {counts['fixed']} | {counts['broken']} | {counts['unchanged']} |")
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("--compare", type=Path, help="baseline artifact (before)")
    parser.add_argument("--weights", type=Path, default=Path(__file__).with_name("weights.json"))
    parser.add_argument("--no-model", action="store_true", help="pool output: do not apply the model readiness gate")
    args = parser.parse_args(argv)
    try:
        rows = load_records(args.artifact)
        summary = summarize(rows, load_weights(args.weights))
        comparison = compare_records(load_records(args.compare), rows) if args.compare else None
    except (OSError, ValueError) as exc:
        print(f"评测报告失败：{exc}", file=sys.stderr)
        return 2
    print(render_report(summary, comparison, require_model=not args.no_model), end="")
    return report_exit_code(summary, require_model=not args.no_model, comparison=comparison)


if __name__ == "__main__":
    raise SystemExit(main())
