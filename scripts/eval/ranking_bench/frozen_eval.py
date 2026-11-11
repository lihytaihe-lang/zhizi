"""Evaluate external cases with S1's frozen choices; never select a threshold."""
import argparse
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
import json
import math
from pathlib import Path

import margin_gate as gate

ROOT = Path("tmp/delegation/rank-s3-independent-protection-sets")
MODES = (("current", False, True), ("fixed_model", True, False),
         ("gate_model", True, False), ("gate_current", False, False))
FIELDS = ("id", "bucket", "raw", "context", "target", "source")


def load_selection(path):
    selection = json.loads(Path(path).read_text(encoding="utf-8"))
    choices = selection.get("choices")
    if not isinstance(choices, dict):
        raise ValueError("missing frozen choices")
    for bucket in gate.BUCKETS:
        for n in gate.LIMITS:
            key = f"{bucket}/{n}"
            if key not in choices:
                raise ValueError(f"missing frozen threshold: {key}")
            value = choices[key]
            if value is not None and (type(value) not in (int, float) or value not in gate.THRESHOLDS):
                raise ValueError(f"invalid frozen threshold: {key}")
    return selection


def normalize_cases(rows):
    if not rows:
        raise ValueError("empty cases file")
    result, ids = [], set()
    for original in rows:
        row = dict(original)
        if any(not isinstance(row.get(k), str) for k in FIELDS):
            raise ValueError("missing or non-string case fields")
        bucket = row["bucket"]
        row.setdefault("sub", row.get("origin_bucket", bucket))
        row.setdefault("src", row.get("corpus_source", row["source"]))
        if bucket in ("name", "english"):
            row["bucket"], row["sub"] = "full", bucket
        if row["bucket"] not in gate.BUCKETS or (row["sub"] in ("name", "english") and row["bucket"] != "full"):
            raise ValueError("unsupported case bucket")
        if (not row["id"] or not row["target"] or not row["raw"]
                or any(c not in "abcdefghijklmnopqrstuvwxyz" for c in row["raw"])
                or len(row["context"]) > 32 or not row["source"]
                or any(not isinstance(row[k], str) or not row[k] or row[k] == "*" for k in ("sub", "src"))):
            raise ValueError("invalid case (no silent filtering or clipping)")
        if row["id"] in ids:
            raise ValueError("duplicate case ID")
        ids.add(row["id"])
        result.append(row)
    return result


def required_correct(broken, baseline_correct):
    if not baseline_correct:
        return dict(observed_rate=None, required_total=None, additional=None, reason="no_originally_correct_cases")
    rate = broken / baseline_correct
    if rate >= .01:
        return dict(observed_rate=rate, required_total=None, additional=None, reason="observed_rate_at_or_above_1_percent")
    # Planning approximation: keep the observed rate fixed, allowing fractional expected errors.
    low, high = 1, 1
    while gate.wilson(rate * high, high)[1] > .01:
        high *= 2
    while low < high:
        middle = (low + high) // 2
        if gate.wilson(rate * middle, middle)[1] <= .01:
            high = middle
        else:
            low = middle + 1
    return dict(observed_rate=rate, required_total=low,
                additional=max(0, low - baseline_correct), reason="fixed_observed_rate_projection")


def new_output(path):
    Path(path).mkdir(parents=True)


def validate_pairs(current, scored):
    if not current or len(current) != len(scored) or len({r["id"] for r in current}) != len(current):
        raise ValueError("missing or duplicate paired rows")
    for old, pool in zip(current, scored):
        if old["id"] != pool["id"]:
            raise ValueError("current/pool alignment mismatch")
        texts, scores = pool["texts"], pool["scores"]
        if (len(texts) != len(scores) or len(texts) != len(set(texts)) or len(texts) > 40
                or any(not isinstance(t, str) or not 1 <= len(t) <= 39 for t in texts)
                or any(type(s) not in (int, float) or not math.isfinite(s) for s in scores)):
            raise ValueError("missing, duplicate or nonfinite pool scores")


def evaluate_rows(current, scored, selection, directory):
    validate_pairs(current, scored)
    results, errors, coverage = [], [], []
    for bucket in gate.BUCKETS:
        indexes = [i for i, row in enumerate(current) if row["bucket"] == bucket]
        if not indexes:
            continue
        old, pool = [current[i] for i in indexes], [scored[i] for i in indexes]
        groups = defaultdict(list)
        for i, row in enumerate(old):
            groups["bucket", "*", "*"].append(i)
            groups["sub", row["sub"], "*"].append(i)
            groups["stratum", row["sub"], row["src"]].append(i)
        for n in gate.LIMITS:
            threshold = selection["choices"][f"{bucket}/{n}"]
            for (level, sub, src), indices in sorted(groups.items()):
                actual = [min(n, len(pool[i]["texts"])) for i in indices]
                coverage.append(dict(bucket=bucket, sub=sub, src=src, level=level, n=n,
                                     total=len(indices), empty=sum(k == 0 for k in actual),
                                     actual_mean=sum(actual) / len(actual), full_n=sum(k == n for k in actual),
                                     head_scored=sum(bool(old[i]["candidates"]) and old[i]["candidates"][0] in pool[i]["texts"][:n] for i in indices),
                                     target_in_current=sum(old[i]["target"] in old[i]["candidates"] for i in indices),
                                     protected_head=sum(bool(old[i]["candidates"]) and not gate.han(old[i]["candidates"][0]) for i in indices),
                                     empty_context=sum(not old[i]["context"] for i in indices)))
            for mode, tail, baseline in MODES:
                choice = threshold if mode.startswith("gate_") else None
                aggregate, outputs = gate.evaluate(old, pool, n, choice, tail, baseline)
                summaries = {tuple(range(len(old))): aggregate}
                for (level, sub, src), indices in sorted(groups.items()):
                    key = tuple(indices)
                    if key not in summaries:
                        summaries[key] = gate.summarize([old[i] for i in indices], [outputs[i] for i in indices])
                    summary = summaries[key]
                    results.append(dict(bucket=bucket, sub=sub, src=src, level=level, n=n, mode=mode,
                                        threshold=choice, new_errors=summary["broken"] if sub in ("name", "english") else None,
                                        sample_estimate=required_correct(summary["broken"], summary["baseline_correct"]), **summary))
                details = []
                for row, output in zip(old, outputs):
                    details.append(dict(id=row["id"], candidates=output,
                                        rank_all=output.index(row["target"]) if row["target"] in output else -1))
                    if (row["sub"] in ("name", "english") and row["candidates"]
                            and row["candidates"][0] == row["target"] and (not output or output[0] != row["target"])):
                        errors.append(dict(row, n=n, mode=mode, threshold=choice,
                                           original_candidates=row["candidates"], candidates=output))
                gate.write_jsonl(directory / f"n{n}-{mode}-{bucket}.jsonl", details)
    return results, errors, coverage


def render_metrics(summary, path):
    lines = ["# 冻结评估结果", "", "所有率、MRR 以 % 表示；括号为逐项 95% 区间。`*` 是合并项。", "",
             "| bucket/sub/src | 层 | N/排法/T | 样本/原本正确 | 首位 | 前二 | 前五 | MRR | 修好/弄坏 | 弄坏/原本正确 | 平均变格/首页变动率 | 新增保护错误 |",
             "|---|---|---|---:|---|---|---|---|---:|---|---|---:|"]
    for r in summary["results"]:
        cells = [gate.metric(r[key]) for key in ("top1", "top2", "top5", "mrr")]
        lines.append(f"| {r['bucket']}/{r['sub']}/{r['src']} | {r['level']} | {r['n']}/{r['mode']}/{r['threshold']} | {r['total']}/{r['baseline_correct']} | "
                     + " | ".join(cells) + f" | {r['fixed']}/{r['broken']} | {gate.metric(r['broken_original'])} | {gate.metric(r['changed_slots'], 1)} / {gate.metric(r['changed_rate'])} | {r['new_errors'] if r['new_errors'] is not None else '—'} |")
    lines += ["", "## 1% 上界所需原本正确样本估算", "",
              "按观察弄坏率不变，求 Wilson 95% 上界 ≤1% 的最小整数总样本量；允许预期错误数为小数，仅作规划。观察率 ≥1% 时，扩大同率样本无法达标。零弄坏需至少 381 个原本正确样本，不能保证未来继续零弄坏。", "",
              "| bucket/sub/src | 层/N | 弄坏/原本正确 | 所需总数 | 还需 | 原因 |", "|---|---|---:|---:|---:|---|"]
    for r in summary["results"]:
        if r["mode"] != "gate_model":
            continue
        estimate = r["sample_estimate"]
        lines.append(f"| {r['bucket']}/{r['sub']}/{r['src']} | {r['level']}/{r['n']} | {r['broken']}/{r['baseline_correct']} | {estimate['required_total']} | {estimate['additional']} | {estimate['reason']} |")
    Path(path).write_text("\n".join(lines) + "\n", encoding="utf-8")


def run(args):
    selection = load_selection(args.selection)
    cases = normalize_cases(gate.read_jsonl(args.cases))
    identity = gate.identity()
    if identity["model"] != selection.get("identity", {}).get("model"):
        raise ValueError("model identity differs from frozen selection")
    slot = "native/host/examples/slot_rules.rs"
    if identity["code"][slot] != selection["identity"].get("code", {}).get(slot):
        raise ValueError("slot_rules source differs from frozen selection")
    identity["binary_sha256"] = gate.digest(args.binary)
    inputs = dict(cases=gate.digest(args.cases), selection=gate.digest(args.selection))
    new_output(args.out)
    gate.write_json(args.out / "selection.json", selection)
    gate.write_jsonl(args.out / "cases.jsonl", cases)
    gate.write_json(args.out / "started.json", dict(inputs=inputs, identity=identity, jobs=args.jobs))

    def collect(bucket):
        group = [r for r in cases if r["bucket"] == bucket]
        path = args.out / f"cases-{bucket}.jsonl"
        gate.write_jsonl(path, [dict({k: r[k] for k in FIELDS}, source="ext") for r in group])
        current = gate.run_engine(args.binary, args.shared, args.out, path, bucket, "current")
        scored = gate.run_engine(args.binary, args.shared, args.out, path, bucket, "pool",
                                 args.out / f"current-{bucket}/rows.jsonl")
        for row, metadata in zip(current, group):
            row.update(sub=metadata["sub"], src=metadata["src"])
        return current, scored

    buckets = [b for b in gate.BUCKETS if any(r["bucket"] == b for r in cases)]
    current, scored = [], []
    with ThreadPoolExecutor(max_workers=args.jobs) as executor:
        for old, pool in executor.map(collect, buckets):
            current.extend(old)
            scored.extend(pool)
    results, errors, coverage = evaluate_rows(current, scored, selection, args.out)
    if inputs != dict(cases=gate.digest(args.cases), selection=gate.digest(args.selection)):
        raise ValueError("input or frozen selection changed during evaluation")
    if (gate.identity() != dict(model=identity["model"], code=identity["code"])
            or gate.digest(args.binary) != identity["binary_sha256"]):
        raise ValueError("model or implementation identity changed during evaluation")
    summary = dict(inputs=inputs, identity=identity, choices=selection["choices"], total=len(cases),
                   results=results, coverage=coverage,
                   protection_new_errors=dict(records=len(errors), unique_cases=len({r["id"] for r in errors})))
    gate.write_jsonl(args.out / "protection-new-errors.jsonl", errors)
    gate.write_json(args.out / "summary.json", summary)
    render_metrics(summary, args.out / "metrics.md")
    print(json.dumps(dict(total=len(cases), protection_new_errors=summary["protection_new_errors"])), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--selection", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--shared", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=Path("/private/tmp/rank-s1-target/release/examples/slot_rules"))
    parser.add_argument("--jobs", type=int, choices=(1, 2, 3), default=1)
    run(parser.parse_args())


if __name__ == "__main__":
    main()
