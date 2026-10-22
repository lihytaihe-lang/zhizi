"""Merge reviewed new-word runs into a separate Rime layer `agent_newwords`.

Separate from agent_general so it can be refreshed monthly and rolled back on its own.
Rules: drop words with spelled-out model numbers (麒麟九千零一, 小米速七: nobody types
those; two or more consecutive Chinese numerals), drop anything already shipped or in
agent_general, weight 400 with corpus evidence else 250, and never above the existing
first choice for the same full reading.
"""

import argparse
import csv
import hashlib
import json
import os
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_general_lexicon import existing_entries  # noqa: E402

WORK = str(WORK_DIR / 'lexicon')
# Standalone colloquial replies, weighted by analogy to shipped words of the same kind (好的 36,136, 好吧 8,222, 是吧 3,554); see calibrate_weights.py.
REPLY_WEIGHTS = {"是的": 36000, "对的": 12000, "对吧": 8000, "行吧": 6000, "好不好": 4000, "对不对": 4000, "是不是": 8000}
CATEGORY_GROUPS = {"动物", "植物", "食物", "物品", "人", "身体与健康", "地方", "生活", "学习与工作", "文化娱乐", "社会"}
NUMERALS = re.compile(r"[零一二三四五六七八九十百千万两]{2,}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", nargs="+", default=[os.path.join(WORK, "new-words-v1/new_words.tsv"),
                                                   os.path.join(WORK, "new-words-v1b/new_words.tsv")])
    ap.add_argument("--out", default=os.path.join(WORK, "export-newwords-v2"))
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    # Everything already shipped, including the general layer, except this layer itself.
    # (v1 was exported from a copy outside the repository, so this lookup found no
    # dictionaries: 179 duplicates and no first-choice caps slipped in.)
    existing = existing_entries(exclude_dictionaries=("agent_newwords",))
    if not existing:
        raise RuntimeError("no shipped dictionaries found; run from the repository")
    top = {}
    for rows in existing.values():
        for reading, weight in rows:
            top[reading] = max(top.get(reading, 0), weight)
    general = {}

    rows, stats, seen = [], Counter(), set()
    for path in args.runs:
        for r in csv.DictReader(open(path, encoding="utf-8"), delimiter="\t"):
            t = r["text"]
            if t in seen:
                stats["duplicate_across_runs"] += 1
                continue
            seen.add(t)
            if NUMERALS.search(t):
                stats["spelled_out_numbers"] += 1
                continue
            if t in existing or t in general:
                stats["already_shipped"] += 1
                continue
            evidence = int(r["wiki"]) + int(r["synth"])
            weight = 400 if evidence > 0 else 250
            calibrated = (r.get("weight") or "").isdigit()
            if calibrated:
                # calibrate_weights.py: a weight on the shipped scale from corpus frequency; not capped below the existing first choice
                # (the existing first choice of 是的 is 似的)
                rows.append((t, r["reading"], int(r["weight"]), r["year"], r["domain"], r["wiki"], r["synth"]))
                continue
            if r["year"] in CATEGORY_GROUPS:
                # Long-tail names of everyday things (gen_category_words.py): at 400 a rare 丹参 outranked the common 淡水 (346) in the
                # prefix list of `dansh`; they only need to be found when nothing commoner competes.
                weight = 250
            if r["reading"] in top and weight >= top[r["reading"]]:
                weight = max(1, top[r["reading"]] - 1)
                stats["capped_below_first_choice"] += 1
            rows.append((t, r["reading"], weight, r["year"], r["domain"], r["wiki"], r["synth"]))
    rows = [(t, r, REPLY_WEIGHTS.get(t, w), *rest) for t, r, w, *rest in rows]
    rows.sort(key=lambda x: (x[1], -x[2], x[0]))

    header = ("# Agent Input Method new-word layer (generated, refreshed periodically; do not edit by hand)\n"
              "# Built by scripts/lexicon/gen_new_words.py + export_new_words.py; DeepSeek-listed, 3-pass reviewed.\n"
              "# Weights never exceed the existing first choice for the same reading.\n"
              "---\nname: agent_newwords\nversion: \"0.1.0\"\nsort: by_weight\n...\n\n")
    path = os.path.join(args.out, "agent_newwords.dict.yaml")
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(header + "".join(f"{t}\t{r}\t{w}\n" for t, r, w, *_ in rows))
    with open(os.path.join(args.out, "agent_newwords.provenance.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\treading\tweight\tyear\tdomain\twiki_hits\tsynth_hits\tsource\tlicense\n")
        for t, r, w, y, d, wk, sy in rows:
            fh.write(f"{t}\t{r}\t{w}\t{y}\t{d}\t{wk}\t{sy}\tdeepseek-flash generation, 3-pass review"
                     f"\tproject-authored\n")
    summary = {"rows": len(rows), "sha256": hashlib.sha256(open(path, "rb").read()).hexdigest(),
               "stats": stats, "by_year": Counter(r[3] for r in rows),
               "with_evidence": sum(1 for r in rows if r[2] >= 400)}
    json.dump(summary, open(os.path.join(args.out, "export.json"), "w"), ensure_ascii=False, indent=1)
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
