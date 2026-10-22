# -*- coding: utf-8 -*-
"""Finish replacing GLM / Qwen-derived evidence in the weights of agent_general and agent_newwords.

  python3 scripts/lexicon/apply_clean_remainder.py --general-open <open-114.tsv> --fit <recount-clean.json> \
      --newwords-recount <newwords-synth-recount.tsv> [--dry-run]

1. agent_general: the 114 rows whose weight apply_general_recount.py had to keep.  Their clean segmentation count is zero, so the
   formula gives no weight; the old weight rests mostly on GLM / Qwen text.  They get the weight the formula gives a word exactly at
   the inclusion threshold (2.0 occurrences per 10M characters, build_general_lexicon.py --min-per-10m) on the clean fit, and never
   more than the old weight (a lowering only), never above the first choice of the reading's other entries (unchanged: the cap only
   ever lowers).
2. agent_newwords: a row at weight 400 whose tier rests on synthetic hits alone (wiki_hits = 0) falls to 250 when no DeepSeek file
   contains the word; the synth_hits column of the rows with synthetic hits becomes the DeepSeek-only count.  Rows with calibrated
   weights (clean texts, calibrate_weights.py) and rows with wiki evidence are not touched.
Dictionary and provenance keep the same row order (reading, weight descending, text); scripts/validate_agent_daily_dictionary.mjs
compares them row by row.
"""
import argparse
import csv
import math
import json
import os

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
RIME = os.path.join(ROOT, "native/resources/Rime")
CORPUS = os.path.join(ROOT, "quality/corpus")
THRESHOLD_PER_10M = 2.0


def split_dictionary(path):
    text = open(path, encoding="utf-8").read()
    marker = "...\n\n"
    cut = text.index(marker) + len(marker)
    return text[:cut]


def write_pair(name, header, rows, fieldnames, prov_path, dict_path, weight_of):
    rows = sorted(rows, key=lambda r: (r["reading"].strip(), -int(r["weight"]), r["text"]))
    open(dict_path, "w", encoding="utf-8").write(header + "".join(f"{r['text']}\t{r['reading'].strip()}\t{r['weight']}\n" for r in rows))
    with open(prov_path, "w", encoding="utf-8") as fh:
        fh.write("\t".join(fieldnames) + "\n")
        for r in rows:
            fh.write("\t".join(r[f] for f in fieldnames) + "\n")


def read(path):
    fh = open(path, encoding="utf-8")
    names = fh.readline().rstrip("\n").split("\t")
    fh.seek(0)
    return names, list(csv.DictReader(fh, delimiter="\t"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--general-open", required=True)
    ap.add_argument("--fit", required=True)
    ap.add_argument("--newwords-recount", required=True)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    fit = json.load(open(args.fit, encoding="utf-8"))
    floor = int(math.exp(fit["fit_a"] + fit["fit_b"] * math.log(THRESHOLD_PER_10M)))
    print(f"floor weight at {THRESHOLD_PER_10M} per 10M on the clean fit: {floor}")

    # --- agent_general
    gd, gp = os.path.join(RIME, "agent_general.dict.yaml"), os.path.join(CORPUS, "agent-general-provenance.tsv")
    gnames, grows = read(gp)
    open_words = {r["text"] for r in csv.DictReader(open(args.general_open, encoding="utf-8"), delimiter="\t")}
    lowered = unchanged = 0
    for r in grows:
        if r["text"] in open_words:
            if int(r["weight"]) > floor:
                r["weight"] = str(floor)
                lowered += 1
            else:
                unchanged += 1
    print(f"agent_general: open rows {len(open_words)}, lowered to {floor}: {lowered}, already at or below: {unchanged}")

    # --- agent_newwords
    nd, npv = os.path.join(RIME, "agent_newwords.dict.yaml"), os.path.join(CORPUS, "agent-newwords-provenance.tsv")
    nnames, nrows = read(npv)
    recount = {r["text"]: r for r in csv.DictReader(open(args.newwords_recount, encoding="utf-8"), delimiter="\t")}
    tier_down = hits_updated = 0
    for r in nrows:
        c = recount.get(r["text"])
        if c is None:
            continue
        clean = int(c["recount_deepseek"])
        if int(r["wiki_hits"]) == 0 and r["weight"] == "400" and clean == 0:
            r["weight"] = "250"
            tier_down += 1
        if int(r["synth_hits"]) != clean:
            r["synth_hits"] = str(clean)
            hits_updated += 1
    print(f"agent_newwords: rows with synthetic hits {len(recount)}, tier 400 -> 250: {tier_down}, synth_hits column replaced: {hits_updated}")
    if args.dry_run:
        return
    write_pair("agent_general", split_dictionary(gd), grows, gnames, gp, gd, None)
    write_pair("agent_newwords", split_dictionary(nd), nrows, nnames, npv, nd, None)
    print("written")


if __name__ == "__main__":
    main()
