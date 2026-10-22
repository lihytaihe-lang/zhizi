# -*- coding: utf-8 -*-
"""Recount the synthetic-chat hits behind the 250 / 400 tiers of agent_newwords, over all providers and over DeepSeek only.

  ime_context_model/.venv/bin/python scripts/lexicon/recount_newwords_synth.py --out <dir>

Why (docs/current/licenses/GLM-QWEN-CLEANUP-2026-10-21.md): export_new_words.py gives a word weight 400 when wiki_hits + synth_hits > 0
and 250 otherwise.  synth_hits are substring counts over synth/v1/batch-*/*.jsonl, which hold GLM and Qwen text next to DeepSeek text;
GLM and Qwen terms do not allow that use.  For every row with synth_hits > 0 this writes the stored count, the recount over every
provider (reproduction check: it should equal the stored count) and the recount over DeepSeek files only (the clean count).
Rows with synth_hits = 0 stay 0.  Nothing here edits a dictionary.
"""
import argparse
import csv
import glob
import os
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WORK = os.environ.get("ZHIZI_WORK", os.path.join(ROOT, "ime_context_model"))
PROVENANCE = os.path.join(ROOT, "quality/corpus/agent-newwords-provenance.tsv")


def provider_texts(provider):
    paths = sorted(glob.glob(os.path.join(WORK, "synth/v1/batch-*", "*.jsonl")))
    if provider != "all":
        paths = [p for p in paths if os.path.basename(p).split(".")[0] == provider]
    return paths, "\n".join(open(p, encoding="utf-8", errors="ignore").read() for p in paths)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    rows = list(csv.DictReader(open(PROVENANCE, encoding="utf-8"), delimiter="\t"))
    targets = [r for r in rows if int(r["synth_hits"]) > 0]
    print(f"rows={len(rows)} with synth_hits>0: {len(targets)}", flush=True)
    counts = {}
    for provider in ("all", "deepseek"):
        paths, text = provider_texts(provider)
        print(f"{provider}: {len(paths)} files, {len(text):,} chars", flush=True)
        counts[provider] = {r["text"]: text.count(r["text"]) for r in targets}
        del text
    with open(os.path.join(args.out, "newwords-synth-recount.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\tweight\twiki_hits\tstored_synth\trecount_all\trecount_deepseek\n")
        for r in targets:
            w = r["text"]
            fh.write(f"{w}\t{r['weight']}\t{r['wiki_hits']}\t{r['synth_hits']}\t{counts['all'][w]}\t{counts['deepseek'][w]}\n")
    print("done", flush=True)


if __name__ == "__main__":
    sys.exit(main())
