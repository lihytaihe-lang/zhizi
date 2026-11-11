"""Offline S1 slot rules: prepare a fresh split, tune, then open test once."""
import argparse
from collections import Counter
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

BUCKETS = ("full", "abbrev", "mixed", "typo")
THRESHOLDS = (1, 2, 3, 5, 6, 8)
LIMITS = (20, 40)
ROOT = Path("tmp/delegation/rank-s1-slot-rules-validation")


def han(text):
    return bool(text) and all("\u3400" <= c <= "\u9fff" for c in text)


def case_key(row):
    return row["raw"], row["context"], row["target"]


def arrange(current, texts, scores, context, threshold, model_tail):
    if len(texts) != len(scores) or any(not math.isfinite(s) for s in scores):
        raise ValueError("missing or nonfinite model scores")
    if len(texts) != len(set(texts)):
        raise ValueError("duplicate scored text")
    if not current or not texts:
        return list(current)
    order = sorted(range(len(texts)), key=lambda i: (-scores[i], i))
    first = current[0]
    head = first
    if threshold is not None and context and han(first) and first in texts:
        if scores[order[0]] - scores[texts.index(first)] > threshold:
            head = texts[order[0]]
    if not model_tail and head == first:
        return list(current)
    page = [head]
    rest = [texts[i] for i in order] if model_tail else current[1:]
    for text in [*rest, *current]:
        if len(page) == 5:
            break
        if text not in page:
            page.append(text)
    return page + [text for text in current if text not in page]


def partition(rows):
    tune, test = [], []
    for bucket in BUCKETS:
        group = sorted((r for r in rows if r["bucket"] == bucket),
                       key=lambda r: (hashlib.sha256(r["id"].encode()).digest(), r["id"]))
        middle = len(group) // 2
        tune.extend(group[:middle])
        test.extend(group[middle:])
    return tune, test


def wilson(successes, total):
    if not total:
        return None, None
    z = 1.959963984540054
    p = successes / total
    scale = 1 + z * z / total
    center = (p + z * z / (2 * total)) / scale
    radius = z * math.sqrt(p * (1 - p) / total + z * z / (4 * total * total)) / scale
    return (0 if successes == 0 else max(0, center - radius),
            1 if successes == total else min(1, center + radius))


def select_threshold(trials):
    eligible = [(t, v) for t, v in trials.items()
                if v["baseline_correct"] and v["broken"] / v["baseline_correct"] <= .01]
    return max(eligible, key=lambda item: (item[1]["top1"], -item[1]["broken"], item[0]))[0] if eligible else None


def paired_counts(before, after, targets):
    if len(before) != len(after) or len(before) != len(targets):
        raise ValueError("unpaired rows")
    return dict(fixed=sum(not b and a for b, a in zip(before, after)),
                broken=sum(b and not a for b, a in zip(before, after)),
                baseline_correct=sum(before), top1=sum(after), total=len(before))


def read_jsonl(path):
    with Path(path).open(encoding="utf-8") as stream:
        return [json.loads(line) for line in stream if line.strip()]


def write_json(path, value):
    with Path(path).open("x", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=False, allow_nan=False, indent=2)
        stream.write("\n")


def write_jsonl(path, rows):
    with Path(path).open("x", encoding="utf-8") as stream:
        for row in rows:
            stream.write(json.dumps(row, ensure_ascii=False, allow_nan=False) + "\n")


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def prepare(external, old, root):
    old_rows = read_jsonl(old)
    old_keys = {case_key(row) for row in old_rows}
    external_rows = read_jsonl(external)
    rows, seen = [], set()
    removed_old, removed_internal = Counter(), Counter()
    for row in external_rows:
        origin = row["bucket"]
        bucket = {"noisy": "typo", "short": "full", "erhua": "full"}.get(origin, origin)
        if bucket not in BUCKETS:
            raise ValueError("unexpected input bucket")
        item = dict(id=f"s1-20261109-{row['id']}", bucket=bucket, raw=row["keys"],
                    context=row["context"], target=row["target"], source="ext",
                    origin_bucket=origin, corpus_source=row["source"])
        if case_key(item) in old_keys:
            removed_old[origin] += 1
        elif case_key(item) in seen:
            removed_internal[origin] += 1
        else:
            seen.add(case_key(item))
            rows.append(item)
    if len({row["id"] for row in rows}) != len(rows):
        raise ValueError("duplicate case IDs")
    tune, test = partition(rows)
    if {case_key(r) for r in tune} & {case_key(r) for r in test}:
        raise ValueError("split overlap")
    root.mkdir(parents=True, exist_ok=True)
    files = {}
    for name, part in (("tune", tune), ("test", test)):
        for suffix, group in [(name, part), *[(f"{name}-{b}", [r for r in part if r["bucket"] == b]) for b in BUCKETS]]:
            path = root / f"{suffix}.jsonl"
            write_jsonl(path, group)
            files[path.name] = digest(path)
    manifest = dict(seed=20261109, requested_per_bucket=1000,
                    generated=dict(Counter(r["bucket"] for r in external_rows)),
                    old_count=len(old_rows), removed_old=dict(removed_old),
                    removed_internal=dict(removed_internal),
                    retained=dict(Counter(r["bucket"] for r in rows)),
                    retained_origin=dict(Counter(r["origin_bucket"] for r in rows)),
                    split={name: dict(bucket=dict(Counter(r["bucket"] for r in part)),
                                      origin=dict(Counter(r["origin_bucket"] for r in part)),
                                      empty_context=dict(Counter(r["bucket"] for r in part if not r["context"])))
                           for name, part in (("tune", tune), ("test", test))},
                    external_sha256=digest(external), old_sha256=digest(old), files=files)
    write_json(root / "dataset.json", manifest)
    return manifest


def summarize(rows, outputs):
    import numpy as np
    if not rows or len(rows) != len(outputs):
        raise ValueError("empty or unpaired summary")
    before = [bool(r["candidates"]) and r["candidates"][0] == r["target"] for r in rows]
    after = [bool(out) and out[0] == r["target"] for r, out in zip(rows, outputs)]
    counts = paired_counts(before, after, [r["target"] for r in rows])
    ranks = [out.index(r["target"]) if r["target"] in out else -1 for r, out in zip(rows, outputs)]
    slots = [sum((old[i] if i < len(old) else None) != (out[i] if i < len(out) else None)
                 for i in range(5)) for old, out in ((r["candidates"], o) for r, o in zip(rows, outputs))]
    n = len(rows)
    result = dict(counts)
    for key, successes, total in (("top1", sum(after), n),
                                  ("top2", sum(0 <= r < 2 for r in ranks), n),
                                  ("top5", sum(0 <= r < 5 for r in ranks), n),
                                  ("fixed_rate", counts["fixed"], n),
                                  ("broken_all", counts["broken"], n),
                                  ("broken_original", counts["broken"], counts["baseline_correct"]),
                                  ("changed_rate", sum(s > 0 for s in slots), n)):
        result[key] = dict(count=successes, denominator=total, value=successes / total if total else None,
                           ci95=wilson(successes, total))
    # Resample the paired case, retaining its old/new result and all five slots together.
    samples = np.random.default_rng(20261109).integers(0, n, size=(4000, n))
    for key, values in (("mrr", [1 / (r + 1) if r >= 0 else 0 for r in ranks]),
                        ("changed_slots", slots),
                        ("delta_top1", [int(a) - int(b) for a, b in zip(after, before)])):
        values = np.asarray(values, dtype=float)
        bounds = np.quantile(values[samples].mean(axis=1), [.025, .975])
        result[key] = dict(value=float(values.mean()), ci95=list(map(float, bounds)))
    return result


def identity():
    model = {}
    for variable in ("ZHIZI_ORT", "ZHIZI_MODEL", "ZHIZI_ITOS"):
        value = os.environ.get(variable)
        if not value:
            raise ValueError(f"missing {variable}")
        model[variable] = digest(value)
    return dict(model=model, code={p: digest(p) for p in
                ("native/host/examples/slot_rules.rs", "scripts/eval/ranking_bench/margin_gate.py")})


def verify_split(root):
    manifest = json.loads((root / "dataset.json").read_text())
    for name, checksum in manifest["files"].items():
        if digest(root / name) != checksum:
            raise ValueError(f"split hash changed: {name}")
    return manifest


def open_phase(root, phase, fingerprint):
    verify_split(root)
    if phase == "test":
        selection = json.loads((root / "selection.json").read_text())
        if selection["identity"] != fingerprint:
            raise ValueError("model or implementation changed after tuning lock")
        # Exclusive creation is intentionally before the first test process, even if it later fails.
        write_json(root / "test-opened.json", dict(selection_sha256=digest(root / "selection.json"),
                                                  identity=fingerprint))
    else:
        if (root / "selection.json").exists():
            raise ValueError("tuning is already frozen")
        selection = None
    directory = root / f"{phase}-run"
    directory.mkdir()
    return directory, selection


def run_engine(binary, shared, directory, cases_path, bucket, mode, current=None):
    output = directory / f"{mode}-{bucket}"
    command = [str(binary.resolve()), str(shared.resolve()), str(output.resolve()),
               str(cases_path.resolve()), mode, bucket]
    if current is not None:
        command.append(str(current.resolve()))
    with (directory / f"{mode}-{bucket}.log").open("xb") as log:
        done = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT)
    if done.returncode:
        raise ValueError(f"{mode}/{bucket} exited {done.returncode}; inspect process log")
    result = read_jsonl(output / "rows.jsonl")
    cases = read_jsonl(cases_path)
    for expected, actual in zip(cases, result):
        if any(expected[k] != actual[k] for k in ("id", "bucket", "raw", "context", "target", "source")):
            raise ValueError("engine case alignment mismatch")
    if len(cases) != len(result) or len({r["id"] for r in result}) != len(result):
        raise ValueError("missing or duplicate engine result")
    print(f"{mode}/{bucket}: {len(result)} rows", flush=True)
    return result


def evaluate(current, scored, n, threshold, model_tail, baseline=False):
    if [r["id"] for r in current] != [r["id"] for r in scored]:
        raise ValueError("unpaired current/pool IDs")
    outputs = []
    for old, pool in zip(current, scored):
        texts, scores = pool["texts"][:n], pool["scores"][:n]
        outputs.append(list(old["candidates"]) if baseline else arrange(
            old["candidates"], texts, scores, old["context"], threshold, model_tail))
    return summarize(current, outputs), outputs


def run_phase(args):
    root = args.root
    fingerprint = identity()
    fingerprint["binary_sha256"] = digest(args.binary)
    directory, selection = open_phase(root, args.phase, fingerprint)
    results, choices, tuning, coverage = [], {}, {}, []
    for bucket in BUCKETS:
        cases_path = root / f"{args.phase}-{bucket}.jsonl"
        current = run_engine(args.binary, args.shared, directory, cases_path, bucket, "current")
        current_path = directory / f"current-{bucket}/rows.jsonl"
        scored = run_engine(args.binary, args.shared, directory, cases_path, bucket, "pool", current_path)
        for n in LIMITS:
            key = f"{bucket}/{n}"
            if args.phase == "tune":
                trials = {}
                for threshold in THRESHOLDS:
                    summary, _ = evaluate(current, scored, n, threshold, True)
                    trials[threshold] = dict(top1=summary["top1"]["count"], broken=summary["broken"],
                                             baseline_correct=summary["baseline_correct"])
                threshold = select_threshold(trials)
                tuning[key] = trials
            else:
                threshold = selection["choices"][key]
            choices[key] = threshold
            modes = (("current", None, False, True), ("fixed_model", None, True, False),
                     ("gate_model", threshold, True, False), ("gate_current", threshold, False, False))
            for mode, gate, tail, baseline in modes:
                summary, outputs = evaluate(current, scored, n, gate, tail, baseline)
                results.append(dict(bucket=bucket, n=n, mode=mode, threshold=gate, **summary))
                write_jsonl(directory / f"n{n}-{mode}-{bucket}.jsonl",
                            [dict(id=r["id"], candidates=out, rank_all=out.index(r["target"]) if r["target"] in out else -1)
                             for r, out in zip(current, outputs)])
            actual = [min(n, len(r["texts"])) for r in scored]
            coverage.append(dict(bucket=bucket, n=n, total=len(actual), actual_min=min(actual),
                                 actual_mean=sum(actual) / len(actual), actual_max=max(actual),
                                 full_n=sum(k == n for k in actual), empty=sum(k == 0 for k in actual),
                                 head_scored=sum(bool(c["candidates"]) and c["candidates"][0] in r["texts"][:n]
                                                 for c, r in zip(current, scored)),
                                 empty_context=sum(not r["context"] for r in current),
                                 protected_head=sum(bool(r["candidates"]) and not han(r["candidates"][0]) for r in current),
                                 current_ready=sum(r["model_ready"] for r in current),
                                 current_outcomes=dict(Counter(r["outcome"] for r in current)),
                                 pool_candidates=sum(r["pool_size"] for r in scored),
                                 complete=sum(r["complete_count"] for r in scored),
                                 comparable=sum(r["comparable_count"] for r in scored)))
    summary = dict(phase=args.phase, choices=choices, tuning=tuning, results=results, coverage=coverage,
                   identity=fingerprint)
    write_json(directory / "summary.json", summary)
    if args.phase == "tune":
        write_json(root / "selection.json", dict(choices=choices, identity=fingerprint,
                   tuning_summary_sha256=digest(directory / "summary.json"),
                   dataset_sha256=digest(root / "dataset.json"),
                   rule="broken / baseline_correct <= 0.01; highest top1, fewer breaks, largest T; none => fixed head"))
    render_report(root, args.report)
    print(json.dumps(dict(phase=args.phase, choices=choices), ensure_ascii=False))


def metric(cell, scale=100):
    if cell["value"] is None:
        return "未定义"
    low, high = cell["ci95"]
    return f"{cell['value'] * scale:.2f} [{low * scale:.2f}, {high * scale:.2f}]"


def render_report(root, path):
    data = verify_split(root)
    tune = json.loads((root / "tune-run/summary.json").read_text())
    selection = json.loads((root / "selection.json").read_text())
    test_path = root / "test-run/summary.json"
    test = json.loads(test_path.read_text()) if test_path.exists() else None
    names = dict(current="现状", fixed_model="首位固定＋模型后四格", gate_model="门槛＋模型后四格", gate_current="门槛＋现状后四格")
    lines = ["# rank-s1-slot-rules-validation：首页分格规则新数据验证", "",
             "日期：2026-11-09。分支：`delegate/rank-s1-slot-rules-validation`，起点 `82b51d0f`。仅离线评测，未改引擎默认行为。", "",
             "## 流程与数据", "",
             "使用现有 `build_external_set.py --seed 20261109 --per-bucket 1000`，来源为授权 v2 的 synth/tatoeba 验证文本及 jieba 词典。`noisy→typo`，`short/erhua→full`，source 统一为 ext，保留原桶来源于临时明细。", "",
             f"生成：`{json.dumps(data['generated'], ensure_ascii=False)}`。与旧集 {data['old_count']} 条按 (raw, context, target) 去重：`{json.dumps(data['removed_old'], ensure_ascii=False)}`；新集内部同三元组去重：`{json.dumps(data['removed_internal'], ensure_ascii=False)}`。", "",
             "同一 bench 桶内按 SHA-256(id) 字节序排序，前 floor(n/2) 调参、其余测试；ID 含新 seed 和原桶 ID。分组不看分数或目标正确性。两组无相同三元组，文件哈希固定。调参 full 中 3 条 short 是单字母键，按任务保留；新 example 支持它们，现有 run_bench 的至少两键限制未改。", "",
             "| 类型 | 保留总数 | 调参 | 测试 | 调参空上文 | 测试空上文 |", "|---|---:|---:|---:|---:|---:|"]
    for b in BUCKETS:
        lines.append(f"| {b} | {data['retained'][b]} | {data['split']['tune']['bucket'][b]} | {data['split']['test']['bucket'][b]} | {data['split']['tune']['empty_context'].get(b, 0)} | {data['split']['test']['empty_context'].get(b, 0)} |")
    lines += ["", f"合并前保留来源：`{json.dumps(data['retained_origin'], ensure_ascii=False)}`；测试原桶数：`{json.dumps(data['split']['test']['origin'], ensure_ascii=False)}`。去重后不足 1,000 的桶不补采、不更换 seed，以免适配结果。", "",
              "## 候选、可比性与排法", "",
              "现状使用真实 Host＋与现有 OnnxScorer 相同计分协议的 CPU 评分器（batch=8、intra/inter=1，profiling 实际 provider 仅 CPUExecutionProvider），独立空白用户目录，最终键 pending 时等待完成（最多 120 秒），评分失败即停止。现状采集保持默认旋钮；首页冻结后，后续页仅枚举、不继续模型换序，最多首页＋60 页。宽池独立空白目录，仅简拼设置 ZX_ABBREV_WORD_PAGES=20、ZX_PEEK_PAGES=60，其他类型生成参数保持默认。通过 C ABI 翻页读取 AimCandidate.reading_key，不改 mixer 或 Host。", "",
              "先筛非空纯中文、1–39 字、读音覆盖整个 raw 的候选；全拼要求完整拼写相等，简拼要求每个字对应一个音节首字母，混拼允许每音节非空前缀，手误允许完整拼写与 raw 至多一次插入/删除/替换/相邻交换。缺读音、不覆盖、拉丁混合候选不参与。进一步只在与现状中文首位相同字数内比较逐字 logprob 之和；非中文首位保留，其后四格以首个覆盖候选的字数为锚。未用 target 决定候选或长度。", "",
              "筛选后按原序取前 N∈{20,40} 个不同文本；一次评最多 40 个，N=20 使用同长、同上文窗口的前 20 分数。模型同分保留原序。现状首位不在这 N 个内时不换。上文空、现状首位非纯中文时不换。门槛严格用 best−first > T；T∈{1,2,3,5,6,8}。无合法 T 时退回首位固定。", "",
              "模型后四格：保留/决定首位，按模型降序填 2–5 格，去重，缺额按现状补齐。现状后四格：首位交换后保持原 2–5 位相对顺序；若被提升者原在其中，去掉重复并由原第 6 位起补齐。新首页之外恢复现状原序，去掉已在首页的项；被挤出的原首位留在后页。", "",
              "## 指标与区间", "",
              "首位/前二/前五按目标精确匹配。MRR 在本轮采集的完整排序上取 1/(rank+1)，池外为 0，最多 61 页，非无限全词库。修好/弄坏按同 ID 首位正确与否配对；同时报告弄坏/全体和弄坏/原本首位正确。选门槛采用后者的 ≤1%，较全体分母更严。", "",
              "所有方括号为逐项 95% 区间：比例为 Wilson；MRR、首位配对净增、平均变格数为固定 seed 20261109 的 4,000 次用例配对 bootstrap 百分位区间。平均变格数逐槽比较原首页与新首页，缺格按空槽；有变动比例为至少一格不同。区间未做多重比较修正，不把零弄坏解释为真实错误率为零。", "",
              "## 调参选择（测试开启前锁定）", "",
              "规则：弄坏/原本首位正确 ≤1% 下首位数最高；并列取弄坏更少、再取较大 T。每类每 N 单独选择；同一 T 用于两种后四格方案，因为首位决策相同。", "",
              "| 类型 | N | 选 T | 现状首位正确数 | 调参首位 | 修好/弄坏 | 弄坏/原本正确 |", "|---|---:|---:|---:|---:|---:|---:|"]
    for row in tune["results"]:
        if row["mode"] == "gate_model":
            t = row["threshold"] if row["threshold"] is not None else "无，首位固定"
            lines.append(f"| {row['bucket']} | {row['n']} | {t} | {row['baseline_correct']} | {metric(row['top1'])} | {row['fixed']}/{row['broken']} | {metric(row['broken_original'])} |")
    lines += ["", f"选择文件 SHA-256：`{digest(root / 'selection.json')}`。调参汇总 SHA-256：`{selection['tuning_summary_sha256']}`。模型 SHA-256：`{selection['identity']['model']['ZHIZI_MODEL']}`。", "",
              "### 六档门槛全部调参结果", "",
              "| 类型/N | T | 首位正确数 | 弄坏数/原本正确数 | 可选 |", "|---|---:|---:|---:|---|"]
    for key, trials in tune["tuning"].items():
        for t, row in trials.items():
            eligible = row['baseline_correct'] > 0 and row['broken'] / row['baseline_correct'] <= .01
            lines.append(f"| {key} | {t} | {row['top1']} | {row['broken']}/{row['baseline_correct']} | {'是' if eligible else '否'} |")
    lines += ["", "## 测试集", ""]
    if test is None:
        lines += ["尚未开启。先将本报告与固定选择尝试提交，再启动一次测试。", ""]
    else:
        lines += ["测试只调用一次编排；`test-opened.json` 在首个测试进程前以独占创建写入，重跑会失败。未在测试上重新选择门槛或 N，以下同时展示两个 N 与四种预定排法。所有率、MRR 以 % 表示，净增为百分点。", "",
                  "| 类型 | N | 排法 | T | 首位 | 前二 | 前五 | MRR | 首位净增 |", "|---|---:|---|---:|---:|---:|---:|---:|---:|"]
        for row in test["results"]:
            cells = " | ".join(metric(row[k]) for k in ("top1", "top2", "top5", "mrr", "delta_top1"))
            lines.append(f"| {row['bucket']} | {row['n']} | {names[row['mode']]} | {row['threshold'] if row['threshold'] is not None else '—'} | {cells} |")
        lines += ["", "### 配对损益与首页变动", "",
                  "修好/弄坏列为条数，修好率分母为全体；弄坏两种分母均列出。平均变格数范围 0–5。", "",
                  "| 类型/N | 排法 | 修好/弄坏 | 修好率 | 弄坏/全体 | 弄坏/原本正确 | 平均变格数 | 有变化条数占比 |", "|---|---|---:|---:|---:|---:|---:|---:|"]
        for row in test["results"]:
            cells = " | ".join(metric(row[k], 1 if k == "changed_slots" else 100)
                               for k in ("fixed_rate", "broken_all", "broken_original", "changed_slots", "changed_rate"))
            lines.append(f"| {row['bucket']}/{row['n']} | {names[row['mode']]} | {row['fixed']}/{row['broken']} | {cells} |")
        lines += ["", "### 预先约定的点估计门禁", "",
                  "按选定 T 的方案判定：各类弄坏/原本正确 ≤1%，简拼还须首位净增 ≥3 点，全拼还须首位不退。N=20/40 全部保留，不从测试挑赢家；门禁通过与区间能否证明阈值分开报告。", "",
                  "| 类型 | N | 门槛＋模型后四格 | 门槛＋现状后四格 | 弄坏率 95% 上界 ≤1% |", "|---|---:|---|---|---|"]
        for row in test["results"]:
            if row["mode"] != "gate_model":
                continue
            rate = row["broken_original"]["value"]
            passed = rate is not None and rate <= .01
            if row["bucket"] == "abbrev":
                passed &= row["delta_top1"]["value"] >= .03 - 1e-12
            if row["bucket"] == "full":
                passed &= row["delta_top1"]["value"] >= 0
            upper = row["broken_original"]["ci95"][1]
            lines.append(f"| {row['bucket']} | {row['n']} | {'通过' if passed else '未通过'} | {'通过' if passed else '未通过'} | {'是' if upper is not None and upper <= .01 else '否'} |")
    lines += ["", "## 候选覆盖与模型健康", "",
              "| 阶段 | 类型/N | 实际 N min/均值/max | 满 N/总数 | 空可比池 | 首位在评分表 | 现状就绪 | 非中文首位 |", "|---|---|---:|---:|---:|---:|---:|---:|"]
    for phase in [tune] + ([test] if test else []):
        for row in phase["coverage"]:
            lines.append(f"| {phase['phase']} | {row['bucket']}/{row['n']} | {row['actual_min']}/{row['actual_mean']:.2f}/{row['actual_max']} | {row['full_n']}/{row['total']} | {row['empty']} | {row['head_scored']} | {row['current_ready']}/{row['total']} | {row['protected_head']} |")
    lines += ["", "下表为去重前原池候选数、通过整串覆盖的候选数，以及覆盖后等字数且文本去重的候选数；未通过者没有送入新增排法的模型比较。现状自身的既有 C1/C2/C3 不受本实验过滤器改写。", "",
              "| 阶段 | 类型 | 原池总数 | 完整覆盖 | 等字数去重后 |", "|---|---|---:|---:|---:|"]
    for phase in [tune] + ([test] if test else []):
        for row in phase["coverage"]:
            if row["n"] == 40:
                lines.append(f"| {phase['phase']} | {row['bucket']} | {row['pool_candidates']} | {row['complete']} | {row['comparable']} |")
    lines += ["", "## 没做的和风险", "",
              "- 语料出自同一公开/合成验证池，换 seed 与三元组去重不等于独立来源或独立句子；训练重叠未核实。测试和调参可能共享原句的不同切片；一例仅一个正确标签。不能据此声称日常输入准确率。",
              "- 去重后简拼/混拼可能不足原计划每类 1,000；儿化极少。空白画像无个人确认/学习数据，英文/姓名专桶未测；仅覆盖数据中出现的非中文首位硬保护及单元测试。",
              "- 完整覆盖外再加等字数约束，避免不同长度 logprob 相加直接竞争，代价是现状首位长度不合适时无法修复。reading_key 为引擎给出的读音；部分猜测项可能回显 raw，无法独立证明其真正发音。手误的单编辑覆盖是本实验的保守定义，非线上全部纠错语义。",
              "- 本轮 ONNX 固定模型，不含个人 LoRA；线上 Core ML 单例分数有小差异，须由 Claude 复核，门槛随模型版本重标。本轮未测 15 ms 同步预算、逐键取消、异步数字键稳定性或真实 UI。",
              "- 首页变动按每例与现状比较；并非同一输入在不同上文间的长期肌肉记忆稳定性。区间为逐项区间，选择多档参数仍有调参不确定性。", "",
              "## 验证和提交状态", "",
              "待填本轮实际测试、构建及提交尝试。", "",
              "## 需要 Claude 决定的事", "",
              "1. 结合点估计、区间与样本来源，决定哪些输入类型值得进入 Core ML 复核；全拼后四格是否继续保持现状。",
              "2. 是否另建按原句分组、独立真实来源、去重后仍足量的验证集，以及补英文/姓名保护集；不要重新打开本测试集调门槛。",
              "3. 复核 reading_key 的回显项、等字数约束与修好/弄坏临时清单，决定可比性和首页变动成本是否可接受；本报告不作上线授权。", ""]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8")


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "self-test":
        unittest.main(argv=[sys.argv[0], *sys.argv[2:]])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prep = commands.add_parser("prepare")
    prep.add_argument("--external", type=Path, required=True)
    prep.add_argument("--old", type=Path, required=True)
    prep.add_argument("--root", type=Path, default=ROOT)
    run = commands.add_parser("run")
    run.add_argument("--phase", choices=("tune", "test"), required=True)
    run.add_argument("--root", type=Path, default=ROOT)
    run.add_argument("--binary", type=Path, default=Path("/private/tmp/rank-s1-target/release/examples/slot_rules"))
    run.add_argument("--shared", type=Path, required=True)
    run.add_argument("--report", type=Path, default=Path("docs/current/evidence/rank-s1-slot-rules-validation/REPORT.md"))
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            print(json.dumps(prepare(args.external, args.old, args.root), ensure_ascii=False, indent=2))
        else:
            run_phase(args)
    except (OSError, ValueError, KeyError) as exc:
        print(f"S1 评测失败：{exc}", file=sys.stderr)
        raise SystemExit(2)


class SlotRulesTests(unittest.TestCase):
    def test_prepare_maps_buckets_deduplicates_and_balances(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            external = [dict(id=f"full-{i}", bucket="full", keys=f"raw{i}", context="上文", target="目标", source="synth")
                        for i in range(4)]
            external += [dict(external[1], id="short-0", bucket="short"),
                         dict(external[2], id="erhua-0", bucket="erhua"),
                         dict(external[3], id="noisy-0", bucket="noisy", keys="other")]
            write_jsonl(root / "external", external)
            write_jsonl(root / "old", [dict(raw="raw0", context="上文", target="目标")])
            result = prepare(root / "external", root / "old", root / "split")
            self.assertEqual(result["retained"], dict(full=3, typo=1))
            self.assertEqual(result["removed_old"], dict(full=1))
            self.assertEqual(result["removed_internal"], dict(short=1, erhua=1))
            self.assertEqual(verify_split(root / "split"), result)
            with (root / "split/test.jsonl").open("a") as stream:
                stream.write("\n")
            with self.assertRaises(ValueError):
                verify_split(root / "split")

    def test_test_phase_is_exclusive_and_checks_frozen_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_json(root / "dataset.json", dict(files={}))
            fingerprint = dict(model="fixture", code="fixture")
            write_json(root / "selection.json", dict(identity=fingerprint))
            with self.assertRaises(ValueError):
                open_phase(root, "test", dict(model="changed"))
            self.assertFalse((root / "test-opened.json").exists())
            open_phase(root, "test", fingerprint)
            with self.assertRaises(FileExistsError):
                open_phase(root, "test", fingerprint)

    def test_metrics_pairing_full_mrr_and_slot_counts(self):
        rows = [dict(id="a", target="目标", candidates=["目标", "另词"]),
                dict(id="b", target="目标", candidates=["另词", "目标"])]
        result = summarize(rows, [["另词", "目标"], ["目标", "另词"]])
        self.assertEqual((result["fixed"], result["broken"]), (1, 1))
        self.assertEqual(result["broken_original"]["value"], 1)
        self.assertEqual(result["broken_all"]["value"], .5)
        self.assertEqual(result["delta_top1"]["value"], 0)
        self.assertEqual(result["mrr"]["value"], .75)
        self.assertEqual(result["changed_slots"]["value"], 2)
        self.assertEqual(result["changed_rate"]["value"], 1)

    def test_first_fixed_model_tail_stable_ties_and_fill(self):
        current = ["当前", "原二", "原三", "原四", "原五", "原六"]
        self.assertEqual(arrange(current, ["模型", "并列"], [-1, -1], "上文", None, True),
                         ["当前", "模型", "并列", "原二", "原三", "原四", "原五", "原六"])

    def test_strict_margin_and_head_outside_pool(self):
        current = ["当前", "原二", "原三", "原四", "原五"]
        self.assertEqual(arrange(current, ["当前", "模型"], [-6, -1], "上文", 5, False), current)
        self.assertEqual(arrange(current, ["模型"], [-1], "上文", 1, False), current)
        self.assertEqual(arrange(current, ["当前", "模型"], [-6.01, -1], "上文", 5, False)[:5],
                         ["模型", "原二", "原三", "原四", "原五"])

    def test_empty_context_and_non_chinese_head_protected(self):
        for first, context in (("当前", ""), ("OpenAI", "上文"), ("API接口", "上文")):
            result = arrange([first, "原二"], [first, "模型"], [-20, -1], context, 1, True)
            self.assertEqual(result[0], first)

    def test_current_tail_skips_duplicate_promoted_head(self):
        current = ["当前", "原二", "模型", "原四", "原五", "原六"]
        result = arrange(current, ["当前", "模型"], [-10, -1], "上文", 1, False)
        self.assertEqual(result[:5], ["模型", "原二", "原四", "原五", "原六"])
        self.assertEqual(result[5:], ["当前"])
        self.assertEqual(len(result), len(set(result)))

    def test_invalid_scores_rejected(self):
        for scores in ([], [math.nan], [math.inf]):
            with self.assertRaises(ValueError):
                arrange(["当前"], ["当前"], scores, "上文", 1, True)

    def test_empty_pool_and_empty_current(self):
        self.assertEqual(arrange(["当前", "原二"], [], [], "上文", 1, True), ["当前", "原二"])
        self.assertEqual(arrange([], ["模型"], [-1], "上文", 1, True), [])

    def test_split_hash_balanced_independent_of_order(self):
        rows = [dict(id=str(i), bucket=b) for b in BUCKETS for i in range(9)]
        first = partition(rows)
        self.assertEqual(first, partition(list(reversed(rows))))
        for bucket in BUCKETS:
            self.assertEqual(sum(r["bucket"] == bucket for r in first[0]), 4)
            self.assertEqual(sum(r["bucket"] == bucket for r in first[1]), 5)

    def test_wilson_zero_errors_is_not_zero_uncertainty(self):
        low, high = wilson(0, 100)
        self.assertEqual(low, 0)
        self.assertGreater(high, .03)
        self.assertLess(high, .04)
        self.assertEqual(wilson(0, 0), (None, None))

    def test_selection_uses_original_correct_denominator_and_conservative_tie(self):
        trials = {t: dict(top1=90, broken=1, baseline_correct=50) for t in THRESHOLDS}
        self.assertIsNone(select_threshold(trials))
        trials[5] = dict(top1=89, broken=0, baseline_correct=50)
        trials[8] = dict(top1=89, broken=0, baseline_correct=50)
        self.assertEqual(select_threshold(trials), 8)
        trials[2] = dict(top1=90, broken=1, baseline_correct=100)
        self.assertEqual(select_threshold(trials), 2)

    def test_pairing_counts_fix_break_not_two_unpaired_rates(self):
        self.assertEqual(paired_counts([False, True, True], [True, False, True], [1, 2, 3]),
                         dict(fixed=1, broken=1, baseline_correct=2, top1=2, total=3))


if __name__ == "__main__":
    main()
