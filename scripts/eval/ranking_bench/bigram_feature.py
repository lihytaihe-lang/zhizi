#!/usr/bin/env python3
"""上文搭配特征的离线验证（rank-b1，工作线 B 度量工具）。

    scripts/eval/ranking_bench/bigram_feature.py build --corpus FILE [FILE ...] --out TABLE.json [--min-count 2] [--max-entries 200000]
    scripts/eval/ranking_bench/bigram_feature.py score --rows FILE [FILE ...] --table TABLE.json --out REPORT.md [--lambdas 0,0.25,0.5,1,2] [--alpha 1.0] [--show-text]

build 读已分词语料（每行一个句子、空格分词；语料留在仓库外，build_general_lexicon.py
用的是 WORK 下的原始文本而非分词语料，见其 corpus_files/_segment），统计行内相邻
词对 prev→next 的频次，只保留 count≥--min-count 的对；总对数超过 --max-entries
时按频次降序（并列按词对字典序）截断；同时记录一元频次与词表大小 V，写成 JSON。

score 读回放产物 rows.jsonl（格式见同目录 schema.md），取目标在 candidates 内的
用例：对 context 末尾按搭配表词表做最长匹配切出上文末词（无命中取末 1–2 字），
每个候选得分 base_rank_score+λ·bigram，其中 base_rank_score=-log(1+原名次)（候选页
内名次的负对数代理）、bigram=log((c(prev,cand)+α)/(c(prev)+α·V))；按 λ 网格输出
Markdown：首位率、Top3、MRR、被挤掉数（原首位是目标、重排后不是）、拉回数
（原非首位、重排后首位）、净变化，按目标长度 1/2/3+ 字分桶，并给出空上文、
末词无搭配记录的覆盖率。λ=0 时得分只随原名次单调，排序与原序完全一致；
末词无搭配记录或空上文时 bigram 项对所有候选同值，不改变排序。
报告只出计数与 id；候选/目标/末词文本仅 --show-text 才写入，上文任何情况不写入，
文本可按 id 与 rows.jsonl 回连。只报事实，不给上线建议。
"""
import argparse
import json
import math
import sys
from collections import Counter, namedtuple
from pathlib import Path

MIN_COUNT_DEFAULT = 2
MAX_ENTRIES_DEFAULT = 200000
ALPHA_DEFAULT = 1.0
LAMBDAS_DEFAULT = "0,0.25,0.5,1,2"
FALLBACK_CHARS = 2
MAX_EXAMPLES = 5
BUCKETS = ((1, "1字"), (2, "2字"), (3, "3字+"))

Table = namedtuple("Table", "data vocab max_len")


def build_table(paths, min_count=MIN_COUNT_DEFAULT, max_entries=MAX_ENTRIES_DEFAULT):
    """行内相邻词对计数 → {"V", "unigram", "bigram"}；返回 (表, 计数统计)。"""
    pair_counts = Counter()
    unigram = Counter()
    tokens = 0
    for path in paths:
        with Path(path).open(encoding="utf-8") as stream:
            for line in stream:
                words = line.split()
                if not words:
                    continue
                tokens += len(words)
                unigram.update(words)
                pair_counts.update(zip(words, words[1:]))
    pairs = [(prev, word, count) for (prev, word), count in pair_counts.items() if count >= min_count]
    stats = {"tokens": tokens, "pairs_total": len(pairs)}
    pairs.sort(key=lambda item: (-item[2], item[0], item[1]))
    if max_entries is not None and len(pairs) > max_entries:
        pairs = pairs[:max_entries]
    bigram = {}
    for prev, word, count in pairs:
        bigram.setdefault(prev, {})[word] = count
    stats["pairs_kept"] = len(pairs)
    return {"V": len(unigram), "unigram": dict(unigram), "bigram": bigram}, stats


def load_table(path):
    """读 build 产出的 JSON；匹配词表 = 一元键 ∪ 搭配前词 ∪ 搭配后词。"""
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"{path}: invalid JSON") from exc
    if not isinstance(data, dict) or not isinstance(data.get("unigram"), dict) \
            or not isinstance(data.get("bigram"), dict):
        raise ValueError(f"{path}: missing unigram/bigram")
    if type(data.get("V")) is not int or data["V"] < 1:
        raise ValueError(f"{path}: invalid V")
    vocab = set(data["unigram"])
    for prev, nexts in data["bigram"].items():
        if not isinstance(nexts, dict):
            raise ValueError(f"{path}: invalid bigram entry for {prev}")
        vocab.add(prev)
        vocab.update(nexts)
    max_len = max((len(word) for word in vocab), default=0)
    return Table(data, vocab, max_len)


def extract_prev_word(context, vocab, max_len):
    """context 末尾按词表最长匹配切末词；无命中回退末 1–2 字。空上文返回 (None, False)。"""
    if not context:
        return None, False
    for size in range(min(len(context), max_len), 0, -1):
        tail = context[-size:]
        if tail in vocab:
            return tail, True
    size = min(FALLBACK_CHARS, len(context))
    return context[-size:], False


def bigram_logprob(prev, cand, data, alpha):
    """log((c(prev,cand)+α)/(c(prev)+α·V))；prev 为空或无记录时对所有候选同值。"""
    pair = data["bigram"].get(prev, {}).get(cand, 0) if prev else 0
    count_prev = data["unigram"].get(prev, 0) if prev else 0
    return math.log((pair + alpha) / (count_prev + alpha * data["V"]))


def rerank(candidates, prev, data, alpha, lam):
    """base_rank_score+λ·bigram 降序（并列保持原序），返回原下标的新顺序。"""
    scores = []
    for rank, cand in enumerate(candidates):
        base = -math.log(1.0 + rank)
        scores.append(base + lam * bigram_logprob(prev, cand, data, alpha))
    return sorted(range(len(candidates)), key=lambda index: (-scores[index], index))


def target_rank(candidates, order, target):
    """重排后目标的页内名次（首次出现，从 0 起）。"""
    for position, index in enumerate(order):
        if candidates[index] == target:
            return position
    return -1


def bucket_label(length):
    for size, label in BUCKETS:
        if length <= size:
            return label
    return BUCKETS[-1][1]


def validate_row(path, number, row):
    """只校验本工具消费的字段；schema.md 其余字段不要求。"""
    def invalid(field):
        raise ValueError(f"{path} line {number}: invalid {field}")

    if not isinstance(row, dict):
        invalid("object")
    if not isinstance(row.get("id"), str) or not row["id"]:
        invalid("id")
    if not isinstance(row.get("context"), str):
        invalid("context")
    if not isinstance(row.get("target"), str) or not row["target"]:
        invalid("target")
    candidates = row.get("candidates")
    if not isinstance(candidates, list) or any(not isinstance(text, str) or not text for text in candidates):
        invalid("candidates")


def load_rows(paths):
    rows = []
    for path in paths:
        with Path(path).open(encoding="utf-8") as stream:
            for number, line in enumerate(stream, 1):
                if not line.strip():
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError as exc:
                    raise ValueError(f"{path} line {number}: invalid JSON") from exc
                validate_row(path, number, row)
                rows.append(row)
    return rows


def select_cases(rows):
    """口径：目标在 candidates 内；其余计 absent 跳过。"""
    selected, absent = [], 0
    for row in rows:
        if row["target"] in row["candidates"]:
            selected.append(row)
        else:
            absent += 1
    return selected, absent


def analyze_case(row, table, lambdas, alpha, show_text=False):
    """单例重排：原名次、各 λ 新名次；文本字段仅 show_text 才写入。"""
    candidates = row["candidates"]
    target = row["target"]
    prev, matched = extract_prev_word(row["context"], table.vocab, table.max_len)
    orders = {lam: rerank(candidates, prev, table.data, alpha, lam) for lam in lambdas}
    case = {
        "id": row["id"],
        "base_rank": candidates.index(target),
        "matched": matched,
        "empty_context": not row["context"],
        "has_record": bool(prev) and prev in table.data["bigram"],
        "length_bucket": bucket_label(len(target)),
        "ranks": {lam: target_rank(candidates, orders[lam], target) for lam in lambdas},
    }
    if show_text:
        case["prev"] = prev
        case["target"] = target
        case["candidates"] = candidates
        case["new_top"] = {lam: candidates[orders[lam][0]] for lam in lambdas}
    return case


def metric_row(cases, lam):
    top1 = sum(case["ranks"][lam] == 0 for case in cases)
    top3 = sum(case["ranks"][lam] < 3 for case in cases)
    mrr = sum(1.0 / (case["ranks"][lam] + 1) for case in cases) / len(cases) if cases else 0.0
    demoted = sum(case["base_rank"] == 0 and case["ranks"][lam] != 0 for case in cases)
    promoted = sum(case["base_rank"] != 0 and case["ranks"][lam] == 0 for case in cases)
    return {"top1": top1, "top3": top3, "mrr": mrr, "demoted": demoted, "promoted": promoted}


def metric_cells(metrics, total):
    if total:
        return (f"{metrics['top1'] / total:.1%}", f"{metrics['top3'] / total:.1%}", f"{metrics['mrr']:.3f}")
    return ("—", "—", "—")


def render_report(cases, absent, rows_paths, table_path, alpha, lambdas, show_text):
    total = len(cases)
    lines = ["# 上文搭配特征重放（rank-b1）", ""]
    lines.append(f"- 产物：{', '.join(str(path) for path in rows_paths)}")
    lines.append(f"- 搭配表：{table_path}；α={alpha:g}；λ 网格：{', '.join(f'{lam:g}' for lam in lambdas)}")
    lines.append(f"- 口径：目标在 candidates 内的用例（{total} 例，名次取候选页内位置）；目标不在候选 {absent} 例跳过")
    lines.append("- 评分：base_rank_score=-log(1+原名次)；bigram=log((c(prev,cand)+α)/(c(prev)+α·V))，c(prev) 取一元频次；"
                 "得分=base+λ·bigram；末词取 context 末尾按搭配表词表最长匹配，未命中回退末 1–2 字")
    lines.append("- λ=0（若在网格中）即基线：得分只随原名次单调，排序与原序一致；末词无搭配记录或空上文的例子 "
                 "bigram 项对所有候选同值，不改变排序")
    text_note = "含候选/目标/末词文本（--show-text）" if show_text else "不含候选/目标/末词文本（--show-text 可开启）"
    lines.append(f"- 文本：{text_note}；上文任何情况不写入；文本可按 id 与 rows.jsonl 回连")
    lines.append("")

    lines.append("## λ 网格")
    lines.extend(["", "| λ | 首位率 | Top3 | MRR | 被挤掉 | 拉回 | 净变化 |",
                  "|---|---:|---:|---:|---:|---:|---:|"])
    for lam in lambdas:
        metrics = metric_row(cases, lam)
        top1, top3, mrr = metric_cells(metrics, total)
        net = metrics["promoted"] - metrics["demoted"]
        lines.append(f"| {lam:g} | {top1} | {top3} | {mrr} | {metrics['demoted']} | {metrics['promoted']} | {net:d} |")
    lines.append("")

    lines.append("## 按目标长度分桶")
    lines.extend(["", "| λ | 目标长度 | 例数 | 首位率 | Top3 | MRR | 被挤掉 | 拉回 |",
                  "|---|---|---:|---:|---:|---:|---:|---:|"])
    for lam in lambdas:
        for _size, label in BUCKETS:
            subset = [case for case in cases if case["length_bucket"] == label]
            metrics = metric_row(subset, lam)
            top1, top3, mrr = metric_cells(metrics, len(subset))
            lines.append(f"| {lam:g} | {label} | {len(subset)} | {top1} | {top3} | {mrr} | "
                         f"{metrics['demoted']} | {metrics['promoted']} |")
    lines.append("")

    lines.append("## 覆盖率")
    lines.extend(["", "| 覆盖 | 例数 | 占比 |", "|---|---:|---:|"])
    empty = sum(case["empty_context"] for case in cases)
    hit = sum(case["has_record"] for case in cases)
    miss = total - empty - hit
    for label, count in (("空上文（无末词）", empty), ("末词命中搭配记录", hit), ("末词无搭配记录（含词表外回退）", miss)):
        share = f"{count / total:.1%}" if total else "—"
        lines.append(f"| {label} | {count} | {share} |")
    matched = sum(case["matched"] for case in cases)
    lines.append("")
    lines.append(f"末词来源：最长匹配 {matched} 例、回退末 1–2 字 {total - matched} 例。")
    lines.append("")

    lines.append("## 变动示例（每 λ 每方向至多 5 个 id）")
    lines.append("")
    for lam in lambdas:
        promoted = [case for case in cases if case["base_rank"] != 0 and case["ranks"][lam] == 0][:MAX_EXAMPLES]
        demoted = [case for case in cases if case["base_rank"] == 0 and case["ranks"][lam] != 0][:MAX_EXAMPLES]
        for label, subset in (("拉回", promoted), ("被挤掉", demoted)):
            if not subset:
                continue
            if show_text:
                items = [f"{case['id']}（末词 {case['prev']}，目标 {case['target']}，"
                         f"原首位 {case['candidates'][0]}，新首位 {case['new_top'][lam]}）" for case in subset]
            else:
                items = [case["id"] for case in subset]
            lines.append(f"- λ={lam:g} {label}: {'；'.join(items)}")
    lines.append("")
    if not total:
        lines.append("（无分析用例）")
        lines.append("")
    return "\n".join(lines) + "\n"


def parse_lambdas(raw):
    values = []
    for part in raw.split(","):
        try:
            values.append(float(part))
        except ValueError:
            raise argparse.ArgumentTypeError(f"invalid lambda: {part!r}") from None
    if not values:
        raise argparse.ArgumentTypeError("empty lambda grid")
    return values


def positive_float(raw):
    value = float(raw)
    if value <= 0:
        raise argparse.ArgumentTypeError("alpha must be > 0")
    return value


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    subparsers = parser.add_subparsers(dest="command", required=True)

    build = subparsers.add_parser("build", help="从已分词语料构建搭配表 JSON")
    build.add_argument("--corpus", nargs="+", required=True, type=Path, help="已分词语料，每行空格分词")
    build.add_argument("--out", required=True, type=Path, help="搭配表 JSON 输出文件")
    build.add_argument("--min-count", type=int, default=MIN_COUNT_DEFAULT, help="保留的词对最小频次")
    build.add_argument("--max-entries", type=int, default=MAX_ENTRIES_DEFAULT, help="词对总条目数上限")

    score = subparsers.add_parser("score", help="按 λ 网格重排回放用例并出报告")
    score.add_argument("--rows", nargs="+", required=True, type=Path, help="回放产物 rows.jsonl，可多份")
    score.add_argument("--table", required=True, type=Path, help="build 产出的搭配表 JSON")
    score.add_argument("--out", required=True, type=Path, help="汇总 Markdown 输出文件")
    score.add_argument("--lambdas", type=parse_lambdas, default=parse_lambdas(LAMBDAS_DEFAULT),
                       help="逗号分隔的 λ 网格")
    score.add_argument("--alpha", type=positive_float, default=ALPHA_DEFAULT, help="bigram 平滑常数 α")
    score.add_argument("--show-text", action="store_true", help="报告写入候选/目标/末词文本（默认不写）")

    args = parser.parse_args(argv)
    try:
        if args.command == "build":
            data, stats = build_table(args.corpus, args.min_count, args.max_entries)
            args.out.write_text(json.dumps(data, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
            print(f"tokens={stats['tokens']} unigrams={data['V']} "
                  f"pairs={stats['pairs_kept']}/{stats['pairs_total']} "
                  f"(min_count={args.min_count}, max_entries={args.max_entries}) -> {args.out}")
            return 0
        rows = load_rows(args.rows)
        table = load_table(args.table)
        selected, absent = select_cases(rows)
        cases = [analyze_case(row, table, args.lambdas, args.alpha, show_text=args.show_text)
                 for row in selected]
        report = render_report(cases, absent, args.rows, args.table, args.alpha, args.lambdas, args.show_text)
        args.out.write_text(report, encoding="utf-8")
    except (OSError, ValueError) as exc:
        print(f"搭配特征验证失败：{exc}", file=sys.stderr)
        return 2
    print(report, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
