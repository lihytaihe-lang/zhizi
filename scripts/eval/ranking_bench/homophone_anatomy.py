#!/usr/bin/env python3
"""同音排错解剖（rank-h1，工作线 B 度量工具）。

    scripts/eval/ranking_bench/homophone_anatomy.py --rows FILE [FILE ...] --lexicon-dir DIR --out FILE [--json FILE] [--show-text]

读回放产物 rows.jsonl（格式见同目录 schema.md），对 bucket=full、rank_all>=1（目标未排
首位）、目标在 candidates 内的用例取首位候选 top 与目标 target，基于词库 *.dict.yaml
（复用 lexicon_tiers.parse_dict_yaml，跳过 agent_owner_words）判定：是否同音（任一读音
相同即同音，任一方不在词库记 unknown）、双方出现的来源与最大数值权重（缺失记 none）、
错序分类（target_missing_in_lexicon / tie_same_weight / weight_inverted /
top_weight_higher / other，互斥按序判）、长度关系与前缀关系。输出 Markdown 计数表与
每类至多 5 个示例 id。只报事实，不给修改建议。候选/目标文本与相同读音默认不写入报告
和逐例 JSON（--show-text 才写入）；上文任何情况都不写入；文本可按 id 与 rows.jsonl 回连。
"""
import argparse
import importlib.util
import json
import sys
from pathlib import Path

_SCRIPT_DIR = Path(__file__).resolve().parent
_LEXICON_TIERS_SPEC = importlib.util.spec_from_file_location("lexicon_tiers", _SCRIPT_DIR / "lexicon_tiers.py")
lexicon_tiers = importlib.util.module_from_spec(_LEXICON_TIERS_SPEC)
_LEXICON_TIERS_SPEC.loader.exec_module(lexicon_tiers)

CLASSES = ("target_missing_in_lexicon", "tie_same_weight", "weight_inverted", "top_weight_higher", "other")
MAX_EXAMPLES = 5
NO_SOURCE = "（无来源）"


def load_lexicon(lexicon_dir):
    """词库索引：term -> {sources: {来源: 最大数值权重或 None}, readings: 读音集合}。

    读音做空白归一并小写；权重转 int，非数值记 None 但仍记录来源归属。
    """
    index = {}
    for path in sorted(Path(lexicon_dir).glob("*.dict.yaml")):
        source_id, _imports, rows, _skipped = lexicon_tiers.parse_dict_yaml(path)
        if source_id in lexicon_tiers.EXCLUDED_SOURCES:
            continue
        for term, pinyin, weight in rows:
            entry = index.setdefault(term, {"sources": {}, "readings": set()})
            try:
                value = int(weight)
            except ValueError:
                value = None
            current = entry["sources"].get(source_id)
            if current is None or (value is not None and value > current):
                entry["sources"][source_id] = value
            entry["readings"].add(" ".join(pinyin.split()).lower())
    return index


def max_weight(entry):
    if entry is None:
        return None
    values = [weight for weight in entry["sources"].values() if weight is not None]
    return max(values) if values else None


def validate_row(path, number, row):
    """只校验本工具消费的字段；schema.md 其余字段不要求。"""
    def invalid(field):
        raise ValueError(f"{path} line {number}: invalid {field}")

    if not isinstance(row, dict):
        invalid("object")
    if not isinstance(row.get("id"), str) or not row["id"]:
        invalid("id")
    if not isinstance(row.get("bucket"), str) or not row["bucket"]:
        invalid("bucket")
    if not isinstance(row.get("target"), str) or not row["target"]:
        invalid("target")
    candidates = row.get("candidates")
    if not isinstance(candidates, list) or any(not isinstance(text, str) or not text for text in candidates):
        invalid("candidates")
    rank = row.get("rank_all")
    if type(rank) is not int or rank < -1:
        invalid("rank_all")


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
    """任务口径：bucket=full、rank_all>=1、目标在 candidates 内。"""
    selected = []
    skipped = {"bucket": 0, "rank": 0, "absent": 0}
    for row in rows:
        if row["bucket"] != "full":
            skipped["bucket"] += 1
        elif row["rank_all"] < 1:
            skipped["rank"] += 1
        elif row["target"] not in row["candidates"]:
            skipped["absent"] += 1
        else:
            selected.append(row)
    return selected, skipped


def classify(top_entry, target_entry):
    if target_entry is None:
        return "target_missing_in_lexicon"
    target_w = max_weight(target_entry)
    top_w = max_weight(top_entry)
    if target_w is None or top_w is None:
        return "other"
    if target_w == top_w:
        return "tie_same_weight"
    return "weight_inverted" if target_w > top_w else "top_weight_higher"


def analyze_case(row, lexicon, show_text=False):
    candidates = row["candidates"]
    top = candidates[0]
    target = row["target"]
    top_entry = lexicon.get(top)
    target_entry = lexicon.get(target)
    if top_entry is None or target_entry is None:
        homophone = "unknown"
        shared = None
    else:
        common = top_entry["readings"] & target_entry["readings"]
        homophone = "yes" if common else "no"
        shared = sorted(common) if common else None
    case = {
        "id": row["id"],
        "rank_all": row["rank_all"],
        "homophone": homophone,
        "top_in_lexicon": top_entry is not None,
        "target_in_lexicon": target_entry is not None,
        "top_sources": sorted(top_entry["sources"]) if top_entry else [],
        "target_sources": sorted(target_entry["sources"]) if target_entry else [],
        "top_max_weight": max_weight(top_entry),
        "target_max_weight": max_weight(target_entry),
        "class": classify(top_entry, target_entry),
        "length": "same" if len(top) == len(target) else ("top_longer" if len(top) > len(target) else "top_shorter"),
        "prefix": top.startswith(target) or target.startswith(top),
    }
    if show_text:
        case["top"] = top
        case["target"] = target
        if shared:
            case["shared_readings"] = shared
    return case


def render_report(cases, skipped, rows_paths, lexicon_dir, show_text):
    total = len(cases)
    lines = ["# 同音排错解剖（rank-h1）", ""]
    lines.append(f"- 产物：{', '.join(str(path) for path in rows_paths)}")
    lines.append(f"- 词库目录：{lexicon_dir}（跳过 agent_owner_words；权重取词条在各来源的最大数值权重，非数值记 none）")
    lines.append(f"- 口径：bucket=full、rank_all>=1（目标未排首位）、目标在 candidates 内；"
                 f"跳过非 full {skipped['bucket']} 例、首位或池外 {skipped['rank']} 例、目标不在候选 {skipped['absent']} 例")
    lines.append(f"- 分析例数：{total}")
    text_note = "含候选/目标文本与相同读音（--show-text）" if show_text else "不含候选/目标文本与读音（--show-text 可开启）"
    lines.append(f"- 文本：{text_note}；上文任何情况不写入；文本可按 id 与 rows.jsonl 回连")
    lines.append("")

    lines.append("## 同音判定（任一读音相同即同音；任一方不在词库为 unknown）")
    lines.extend(["", "| 判定 | 例数 | 占比 |", "|---|---|---:|"])
    for key in ("yes", "no", "unknown"):
        count = sum(case["homophone"] == key for case in cases)
        share = f"{count / total:.1%}" if total else "—"
        lines.append(f"| {key} | {count} | {share} |")
    lines.append("")

    lines.append("## 分类 × 目标所在来源（例数）")
    lines.append("")
    lines.append("目标多来源时在多列各计一次，行内单元格之和可能大于该类例数；以「例数」列为准。")
    lines.append("")
    sources = sorted({source for case in cases for source in case["target_sources"]})
    if any(not case["target_sources"] for case in cases):
        sources.append(NO_SOURCE)
    if total:
        lines.append("| 分类 | " + " | ".join(sources) + " | 例数 |")
        lines.append("|---|" + "---:|" * (len(sources) + 1))
        for cls in CLASSES:
            subset = [case for case in cases if case["class"] == cls]
            cells = []
            for source in sources:
                if source == NO_SOURCE:
                    count = sum(1 for case in subset if not case["target_sources"])
                else:
                    count = sum(1 for case in subset if source in case["target_sources"])
                cells.append(str(count))
            lines.append(f"| {cls} | " + " | ".join(cells) + f" | {len(subset)} |")
        totals = [str(sum(1 for case in cases if source in case["target_sources"])) for source in sources]
        lines.append(f"| 合计 | " + " | ".join(totals) + f" | {total} |")
    else:
        lines.append("（无用例）")
    lines.append("")

    lines.append("## 分类示例（每类至多 5 个 id）")
    lines.append("")
    for cls in CLASSES:
        subset = [case for case in cases if case["class"] == cls][:MAX_EXAMPLES]
        if show_text:
            items = [f"{case['id']}（首位 {case['top']}→目标 {case['target']}"
                     + (f"，同音 {' / '.join(case['shared_readings'])}" if case.get("shared_readings") else "") + "）"
                     for case in subset]
            lines.append(f"- {cls}: {'；'.join(items) if items else '（无）'}")
        else:
            ids = [case["id"] for case in subset]
            lines.append(f"- {cls}: {', '.join(ids) if ids else '（无）'}")
    lines.append("")

    lines.append("## 长度关系（按字符数）")
    lines.extend(["", "| 关系 | 例数 |", "|---|---:|"])
    for key, label in (("same", "同字数"), ("top_longer", "首位更长"), ("top_shorter", "首位更短")):
        count = sum(case["length"] == key for case in cases)
        lines.append(f"| {label} | {count} |")
    lines.append("")
    prefix = sum(case["prefix"] for case in cases)
    lines.append(f"互为前缀（一方是另一方前缀）：{prefix} 例")
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--rows", nargs="+", required=True, type=Path, help="回放产物 rows.jsonl，可多份")
    parser.add_argument("--lexicon-dir", required=True, type=Path, help="词库目录（*.dict.yaml）")
    parser.add_argument("--out", required=True, type=Path, help="汇总 Markdown 输出文件")
    parser.add_argument("--json", dest="json_out", default=None, type=Path, help="逐例 JSON 输出文件")
    parser.add_argument("--show-text", action="store_true",
                        help="报告与逐例 JSON 写入候选/目标文本与相同读音（默认不写）")
    args = parser.parse_args(argv)
    try:
        rows = load_rows(args.rows)
        lexicon = load_lexicon(args.lexicon_dir)
        selected, skipped = select_cases(rows)
        cases = [analyze_case(row, lexicon, show_text=args.show_text) for row in selected]
        report = render_report(cases, skipped, args.rows, args.lexicon_dir, args.show_text)
        args.out.write_text(report, encoding="utf-8")
        if args.json_out:
            args.json_out.write_text(json.dumps(cases, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    except (OSError, ValueError) as exc:
        print(f"同音解剖失败：{exc}", file=sys.stderr)
        return 2
    print(report, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
