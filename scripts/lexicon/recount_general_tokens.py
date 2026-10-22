# -*- coding: utf-8 -*-
"""Recount the corpus frequencies behind the weights of agent_general, over the old or the clean corpus.

  ime_context_model/.venv/bin/python scripts/lexicon/recount_general_tokens.py --corpus old   --out <dir>
  ime_context_model/.venv/bin/python scripts/lexicon/recount_general_tokens.py --corpus clean --out <dir>

Why (docs/current/licenses/GLM-QWEN-CLEANUP-2026-10-21.md): the weights of agent_general were fitted on word counts over a
colloquial corpus that included GLM / Qwen synthetic text, whose terms do not allow that use.  This script runs the *same two counting
passes* as build_general_lexicon.py (raw substring counts -> unigram costs -> best segmentation, counted per token) and writes, for
every word of quality/corpus/agent-general-provenance.tsv, the recounted colloquial and written token counts next to the stored ones.

  --corpus old    the corpora build_general_lexicon.py used: corpus/v2 (asap, tatoeba, wikipedia, un) + synth/v1/batch-*
                  (all batches, mixed providers).  Reproduction check: the recount must match the stored columns.
  --corpus clean  clean-v2/corpus/v2 only (asap, tatoeba, DeepSeek-only synthetic text, wikipedia, un).

The vocabulary is rebuilt as it was on 2026-10-09: the dictionary layers that existed then, so the later layers are excluded.
Nothing here edits a dictionary; the output is a table to review.
"""
import argparse
import csv
import math
import os
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_general_lexicon as bgl  # noqa: E402

LATER_LAYERS = ("agent_ai_names", "agent_ai_terms", "agent_general", "agent_newwords", "agent_spoken",
                "agent_pack_cad", "agent_pack_industry", "agent_pack_poetry", "agent_pack_terms")
PROVENANCE = os.path.join(bgl.REPO, "quality/corpus/agent-general-provenance.tsv")


def clean_files():
    v2 = os.path.join(bgl.WORK, "clean-v2/corpus/v2")
    colloq = [(os.path.join(v2, f), None) for f in ("asap.train.txt", "tatoeba.train.txt", "synth.train.txt")]
    written = [(os.path.join(v2, "wikipedia.train.txt"), 80_000_000), (os.path.join(v2, "un.train.txt"), 20_000_000)]
    return colloq, written


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", choices=("old", "clean"), required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--procs", type=int, default=6)
    ap.add_argument("--max-batch", type=int, default=None,
                    help="with --corpus old: keep only synth batches numbered up to this (the build of 2026-10-09 saw 001-014)")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    jieba = os.path.join(bgl.WORK, "lexicon/src/jieba-0.42.1/jieba-0.42.1/jieba/dict.txt")
    existing = bgl.existing_entries(exclude_dictionaries=LATER_LAYERS)
    words = bgl.candidate_words(jieba)
    new = {w: m for w, m in words.items() if w not in existing}
    vocab = set(new) | {w for w in existing if bgl.HAN.match(w)} | {w for w in existing if len(w) == 1}
    colloq_files, written_files = bgl.corpus_files() if args.corpus == "old" else clean_files()
    if args.max_batch is not None:
        import re
        colloq_files = [(p, m) for p, m in colloq_files
                        if not (re.search(r"/batch-(\d+)", p) and int(re.search(r"/batch-(\d+)", p).group(1)) > args.max_batch)]
    print(f"corpus={args.corpus} existing={len(existing)} candidates={len(words)} new={len(new)} vocab={len(vocab)}", flush=True)
    for path, _ in colloq_files + written_files:
        print("  file", os.path.relpath(path, bgl.WORK), flush=True)

    raw, raw_chars = bgl.count_corpus(vocab, colloq_files + written_files, args.procs)
    print(f"pass1 substring counts over {raw_chars} chars", flush=True)
    cost = {w: -math.log((c + 0.5) / raw_chars) for w, c in raw.items() if c > 0}
    colloq, colloq_chars = bgl.count_corpus(None, colloq_files, args.procs, cost)
    written, written_chars = bgl.count_corpus(None, written_files, args.procs, cost)
    print(f"colloquial chars={colloq_chars} written chars={written_chars}", flush=True)

    # The same weight rule as build_general_lexicon.py + export_general_dictionary.py: fit log(top weight) ~ a + b log(per10m) on the
    # shipped words, weight = exp(fit) for a word, then never above the existing first choice of its reading (top - 1).
    def per10m(w):
        return 1e7 * (2 * colloq[w] / max(colloq_chars, 1) + written[w] / max(written_chars, 1)) / 3

    xs, ys = [], []
    for w, wrows in existing.items():
        top = max(wt for _, wt in wrows)
        f = per10m(w) if bgl.HAN.match(w) else 0
        if top > 0 and f > 0:
            xs.append(math.log(f))
            ys.append(math.log(top))
    mx, my = sum(xs) / len(xs), sum(ys) / len(ys)
    b = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sum((x - mx) ** 2 for x in xs)
    a = my - b * mx
    print(f"weight fit on {len(xs)} shipped words: log w = {a:.3f} + {b:.3f} log f", flush=True)
    top_by_reading = {}
    for w, wrows in existing.items():
        for reading, wt in wrows:
            if wt > top_by_reading.get(reading, -1):
                top_by_reading[reading] = wt

    def formula_weight(w, reading):
        f = per10m(w)
        if f <= 0:
            return None, f
        weight = max(1, int(math.exp(a + b * math.log(f))))
        top = top_by_reading.get(reading)
        if top is not None and weight >= top:
            weight = max(1, top - 1)
        return weight, f

    rows = list(csv.DictReader(open(PROVENANCE, encoding="utf-8"), delimiter="\t"))
    suffix = f"{args.corpus}{'-max' + str(args.max_batch) if args.max_batch else ''}"
    with open(os.path.join(args.out, f"recount-{suffix}.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\treading\tsource\tstored_weight\tformula_weight\tper10m\tstored_colloquial\tstored_written\tnew_colloquial\tnew_written\n")
        for r in rows:
            w = r["text"]
            weight, f = formula_weight(w, r["reading"].strip())
            fh.write(f"{w}\t{r['reading'].strip()}\t{r['source']}\t{r['weight']}\t{'' if weight is None else weight}\t{f:.4f}\t"
                     f"{r['colloquial_tokens']}\t{r['written_tokens']}\t{colloq.get(w, 0)}\t{written.get(w, 0)}\n")
    json_out = {"corpus": args.corpus, "colloquial_chars": colloq_chars, "written_chars": written_chars, "rows": len(rows),
                "fit_a": a, "fit_b": b, "fit_n": len(xs)}
    import json
    json.dump(json_out, open(os.path.join(args.out, f"recount-{suffix}.json"), "w"), indent=1)
    print("done", json_out, flush=True)


if __name__ == "__main__":
    main()
