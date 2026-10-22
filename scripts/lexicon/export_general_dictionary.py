"""Export the reviewed general-vocabulary list as a Rime dictionary plus provenance.

Policy (v1): a new word never outranks the existing first choice for its full reading;
its weight is capped one below that entry. Homophone order is left to the context model.

Inputs:  <work>/lexicon/v2/reviewed.tsv (from judge_lexicon.py)
Outputs: <out>/agent_general.dict.yaml, <out>/agent_general.provenance.tsv, <out>/export.json
"""

import argparse
import hashlib
import json
import os
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, os.path.dirname(__file__))
from build_general_lexicon import existing_entries  # noqa: E402

WORK = str(WORK_DIR / 'lexicon')
LICENSES = {
    "jieba": "MIT (jieba 0.42.1 dict.txt, PyPI sdist sha256 055ca12f62674fafed09427f176506079bc135638a14e23e25be909131928db2)",
    "pypinyin-phrases": "MIT (pypinyin 0.55.0 phrases_dict)",
    "phrase-layer-mined": "project-authored (n-grams mined from licensed corpora, DeepSeek-reviewed)",
    "phrase-layer-generated": "project-authored (DeepSeek-generated phrases, corpus-validated)",
    "owner-missing-curated": "project-authored (words the owner's records show missing, curated by hand)",
}
REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OWNER_MISSING_DEFAULT_WEIGHT = 500


# LEXICON-L1: preserve frozen factory candidates, including non-exact readings.
# Full-reading weight ceilings cannot protect abbreviation/prefix competition.
FROZEN_GATE_EXCLUSIONS = {
    "征募": "fuzzy-exact-priority-v1: zenm must keep 怎么 as Top-1",
    "镇民": "fuzzy-exact-priority-v1: zenm must keep 怎么 as Top-1",
    "正忙": "fuzzy-exact-priority-v1: zenm must keep 怎么 as Top-1",
    "枕木": "fuzzy-exact-priority-v1: zenm must keep 怎么 as Top-1",
    "知恩": "fuzzy-exact-priority-v1: zenm must keep 怎么 as Top-1 with fuzzyMask=231",
    "灾厄": "fuzzy-exact-priority-v1: zenm must keep 怎么 as Top-1 with fuzzyMask=231",
    "汁儿": "fuzzy-exact-priority-v1: zenm must keep 怎么 as Top-1 with fuzzyMask=231",
    "九块": "partial-coverage-top1-v1: jiukyi must preserve 就开 in its frozen Top-5 set",
    "就可以": "partial-coverage-top1-v1: jiukyi must preserve its frozen Top-5 set",
    "单式": "project-abbreviation-regression-v1: dansh must keep its frozen Top-5 (淡水 fifth)",
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reviewed", default=os.path.join(WORK, "v2/reviewed.tsv"))
    ap.add_argument("--out", default=os.path.join(WORK, "export-v1"))
    ap.add_argument("--owner-missing",
                    default=os.path.join(REPO, "quality/corpus/lexicon-owner-missing-v1.tsv"))
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    if len(FROZEN_GATE_EXCLUSIONS) > 200:
        raise RuntimeError("LEXICON-L1 blocked: more than 200 frozen-gate exclusions")
    # Re-exporting after integration must use the pre-L1 baseline, not dedup
    # against the very outputs being regenerated.
    existing = existing_entries(exclude_dictionaries=("agent_general", "agent_newwords"))
    top = {}
    for word, rows in existing.items():
        for reading, weight in rows:
            if weight > top.get(reading, -1):
                top[reading] = weight

    rows, stats = [], Counter()
    for line in list(open(args.reviewed, encoding="utf-8"))[1:]:
        text, reading, weight, colloq, written, source, flags = line.rstrip("\n").split("\t")
        if text in FROZEN_GATE_EXCLUSIONS:
            stats["frozen_gate_exclusions"] += 1
            continue
        if text in existing:
            stats["already_shipped"] += 1
            continue
        weight = int(weight)
        if reading in top and weight >= top[reading]:
            weight = max(1, top[reading] - 1)
            stats["capped_below_first_choice"] += 1
        rows.append((text, reading, weight, int(colloq), int(written), source))
    # Owner-observed gaps (daishuru -> 代输入): same cap as every new word, so a
    # homophone with an existing first choice lands second and the model decides.
    seen = {r[0] for r in rows}
    for line in list(open(args.owner_missing, encoding="utf-8"))[1:]:
        text, reading, _evidence = line.rstrip("\n").split("\t")
        if text in existing or text in seen:
            stats["owner_missing_already_present"] += 1
            continue
        weight = OWNER_MISSING_DEFAULT_WEIGHT
        if reading in top and weight >= top[reading]:
            weight = max(1, top[reading] - 1)
            stats["capped_below_first_choice"] += 1
        stats["owner_missing_added"] += 1
        rows.append((text, reading, weight, 0, 0, "owner-missing-curated"))
    rows.sort(key=lambda r: (r[1], -r[2], r[0]))

    header = (
        "# Agent Input Method general vocabulary layer (generated, do not edit by hand)\n"
        "# Built by scripts/lexicon/{build_general_lexicon,judge_lexicon,export_general_dictionary}.py\n"
        "# Weights from segmented corpus frequency; never above the existing first choice per reading.\n"
        "---\nname: agent_general\nversion: \"0.1.0\"\nsort: by_weight\n...\n\n"
    )
    body = "".join(f"{t}\t{r}\t{w}\n" for t, r, w, *_ in rows)
    dict_path = os.path.join(args.out, "agent_general.dict.yaml")
    with open(dict_path, "w", encoding="utf-8") as fh:
        fh.write(header + body)
    with open(os.path.join(args.out, "agent_general.provenance.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\treading\tweight\tcolloquial_tokens\twritten_tokens\tsource\tlicense\n")
        for t, r, w, c, wr, s in rows:
            fh.write(f"{t}\t{r}\t{w}\t{c}\t{wr}\t{s}\t{LICENSES.get(s, 'unknown')}\n")
    sha = hashlib.sha256(open(dict_path, "rb").read()).hexdigest()
    summary = {"rows": len(rows), "sha256": sha, "stats": stats,
               "by_source": Counter(r[5] for r in rows), "by_length": Counter(len(r[0]) for r in rows),
               "exclusions": FROZEN_GATE_EXCLUSIONS,
               "weight_p50": sorted(r[2] for r in rows)[len(rows) // 2]}
    json.dump(summary, open(os.path.join(args.out, "export.json"), "w"), ensure_ascii=False, indent=1)
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
