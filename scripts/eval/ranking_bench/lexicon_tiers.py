#!/usr/bin/env python3
"""词库权重层次审计（rank-b0，工作线 B 前置）。

    scripts/eval/ranking_bench/lexicon_tiers.py [--lexicon-dir DIR] [--catalog FILE] [--json FILE]

读 Rime 词库 *.dict.yaml（默认 native/resources/Rime），按来源统计：条数、不同权重值
个数、最大同值占比、熵（按权重值分布，单位 bit）、词长分布与 2–4 字占比；标出最大
同值占比 > 80% 的平档来源，并与 native/resources/Lexicon/lexicon_catalog.json 的目录
条数对照。只读词库；跳过 agent_owner_words（owner 数据）。只报事实，不给重排建议。
"""
import argparse
import json
import math
import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
FLAT_SHARE = 0.8
EXCLUDED_SOURCES = {"agent_owner_words": "owner 数据，审计不读"}
NAME_RE = re.compile(r"^name:\s*(\S+)")
IMPORT_RE = re.compile(r"^\s*-\s*(\S+)")


def parse_dict_yaml(path):
    """返回 (source_id, imports, rows, skipped_rows)。

    rows 为 (term, pinyin, weight) 三元组；数据段里字段数 != 3 的行计入 skipped_rows。
    """
    source_id = path.stem.removesuffix(".dict")
    imports = []
    rows = []
    skipped = 0
    in_meta = False
    in_data = False
    current_import = False
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.strip() == "---" and not in_meta and not in_data:
            in_meta = True
            continue
        if in_meta and line.strip() == "...":
            in_meta = False
            in_data = True
            current_import = False
            continue
        if in_meta:
            m = NAME_RE.match(line)
            if m:
                source_id = m.group(1)
            if re.match(r"^import_tables:", line):
                current_import = True
            elif current_import:
                m = IMPORT_RE.match(line)
                if m:
                    imports.append(m.group(1))
                else:
                    current_import = False
            continue
        if not in_data or not line.strip() or line.lstrip().startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) == 3:
            rows.append((fields[0], fields[1], fields[2].strip()))
        else:
            skipped += 1
    return source_id, imports, rows, skipped


def length_bucket(term):
    n = len(term)
    return str(n) if n < 5 else "5+"


def weight_stats(rows):
    n = len(rows)
    if n == 0:
        return {"entries": 0, "distinct_weights": 0, "max_share": 0.0, "entropy_bits": 0.0,
                "top_weight": "", "top_count": 0}
    counts = {}
    for _, _, weight in rows:
        counts[weight] = counts.get(weight, 0) + 1
    top_weight = max(counts, key=counts.get)
    top = counts[top_weight]
    entropy = sum((c / n) * math.log2(n / c) for c in counts.values())
    return {"entries": n, "distinct_weights": len(counts), "max_share": top / n,
            "entropy_bits": entropy, "top_weight": top_weight, "top_count": top}


def source_stats(source_id, imports, rows, skipped, catalog_entry):
    stats = weight_stats(rows)
    lengths = {"1": 0, "2": 0, "3": 0, "4": 0, "5+": 0}
    for term, _, _ in rows:
        lengths[length_bucket(term)] += 1
    in_2_4 = lengths["2"] + lengths["3"] + lengths["4"]
    out = {
        "id": source_id,
        "imports": imports,
        "group": catalog_entry.get("group", "") if catalog_entry else "",
        "catalog_rows": catalog_entry.get("rows") if catalog_entry else None,
        "skipped_rows": skipped,
        "lengths": lengths,
        "share_2_4": in_2_4 / stats["entries"] if stats["entries"] else 0.0,
        "flat": stats["entries"] > 0 and stats["max_share"] > FLAT_SHARE,
    }
    out.update(stats)
    return out


def audit_dir(lexicon_dir, catalog_path=None):
    catalog = {}
    if catalog_path and Path(catalog_path).exists():
        data = json.loads(Path(catalog_path).read_text(encoding="utf-8"))
        catalog = {layer["id"]: layer for layer in data.get("layers", [])}
    sources = []
    excluded = []
    for path in sorted(Path(lexicon_dir).glob("*.dict.yaml")):
        source_id, imports, rows, skipped = parse_dict_yaml(path)
        if source_id in EXCLUDED_SOURCES:
            excluded.append(source_id)
            continue
        sources.append(source_stats(source_id, imports, rows, skipped, catalog.get(source_id)))
    sources.sort(key=lambda s: -s["entries"])
    catalog_missing = sorted(set(catalog) - {s["id"] for s in sources} - set(EXCLUDED_SOURCES))
    row_mismatches = [s["id"] for s in sources if s["catalog_rows"] not in (None, s["entries"])]
    flat_sources = [s for s in sources if s["flat"]]
    return {"sources": sources, "flat_sources": flat_sources, "excluded": excluded,
            "catalog_missing": catalog_missing, "row_mismatches": row_mismatches}


def pct(x):
    return f"{100 * x:.1f}%"


def format_markdown(audit):
    lines = []
    lines.append("## 来源 × 权重层次（按条数降序）")
    lines.append("")
    lines.append("| 来源 | 组 | 条数 | 目录条数 | 权重值数 | 最大同值占比 | 熵(bit) | 2–4字占比 | 平档 |")
    lines.append("|---|---|---:|---:|---:|---:|---:|---:|---|")
    for s in audit["sources"]:
        cat = s["catalog_rows"] if s["catalog_rows"] is not None else "—"
        lines.append(f"| {s['id']} | {s['group'] or '—'} | {s['entries']} | {cat} | "
                     f"{s['distinct_weights']} | {pct(s['max_share'])} | {s['entropy_bits']:.3f} | "
                     f"{pct(s['share_2_4'])} | {'是' if s['flat'] else ''} |")
    lines.append("")
    lines.append("## 词长分布（按字符数，占该来源条数 %）")
    lines.append("")
    lines.append("| 来源 | 1字 | 2字 | 3字 | 4字 | 5字+ |")
    lines.append("|---|---:|---:|---:|---:|---:|")
    for s in audit["sources"]:
        total = max(s["entries"], 1)
        cells = " | ".join(pct(s["lengths"][b] / total) for b in ("1", "2", "3", "4", "5+"))
        lines.append(f"| {s['id']} | {cells} |")
    lines.append("")
    lines.append(f"## 平档来源（最大同值占比 > {FLAT_SHARE:.0%}，按条数降序）")
    lines.append("")
    if audit["flat_sources"]:
        for s in audit["flat_sources"]:
            others = s["entries"] - s["top_count"]
            detail = f"权重 {s['top_weight']}×{s['top_count']}"
            if others:
                detail += f"，其余 {others} 条分布在另外 {s['distinct_weights'] - 1} 个值"
            lines.append(f"- {s['id']}：{s['entries']} 条，{detail}，"
                         f"最大同值占比 {pct(s['max_share'])}，熵 {s['entropy_bits']:.3f} bit")
    else:
        lines.append("（无）")
    lines.append("")
    lines.append("## 对照与例外")
    lines.append("")
    if audit["excluded"]:
        for sid in audit["excluded"]:
            lines.append(f"- 排除来源：`{sid}`（{EXCLUDED_SOURCES[sid]}）")
    if audit["catalog_missing"]:
        lines.append(f"- 目录有、词库目录无文件：{', '.join(audit['catalog_missing'])}")
    if audit["row_mismatches"]:
        lines.append(f"- 目录条数与实际条数不一致：{', '.join(audit['row_mismatches'])}")
    aggregates = [s["id"] for s in audit["sources"] if s["imports"]]
    if aggregates:
        for s in audit["sources"]:
            if s["imports"]:
                lines.append(f"- 聚合档：`{s['id']}` 自身 0 条，import_tables = {', '.join(s['imports'])}")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--lexicon-dir", default=str(REPO_ROOT / "native" / "resources" / "Rime"))
    parser.add_argument("--catalog", default=str(REPO_ROOT / "native" / "resources" / "Lexicon" / "lexicon_catalog.json"))
    parser.add_argument("--json", dest="json_out", default="", help="同时把统计写成 JSON 文件")
    args = parser.parse_args()
    audit = audit_dir(args.lexicon_dir, args.catalog)
    print(format_markdown(audit))
    if args.json_out:
        Path(args.json_out).write_text(json.dumps(audit, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
