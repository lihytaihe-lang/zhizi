# -*- coding: utf-8 -*-
"""How many of the n-gram-mined rows of agent_general are attested in text that is clean for release.

  ime_context_model/.venv/bin/python scripts/lexicon/clean_attestation.py [--min 3] [--out report.tsv]

Background (docs/current/licenses/LEXICON-LAYER-AUDIT-2026-10-16.md, open item 1): the 8,296 `phrase-layer-mined` rows of agent_general were
found and weighted on synthetic text that included GLM / Qwen output, whose terms do not allow training use.  A row that also occurs at least
`--min` times in text with no such output (asap, Tatoeba, the DeepSeek-only synthetic text, Wikipedia, UN) does not depend on that text: it
can be released as is.  The rest are listed for the owner (drop from the public release, or re-mine).  Counts are plain substring counts.
"""
import argparse
import csv
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
WORK = os.environ.get("ZHIZI_WORK", os.path.join(ROOT, "ime_context_model"))
CLEAN_TEXTS = {
    "asap": f"{WORK}/clean-v2/corpus/v2/asap.train.txt",
    "tatoeba": f"{WORK}/clean-v2/corpus/v2/tatoeba.train.txt",
    "synth-deepseek": f"{WORK}/clean-v2/corpus/v2/synth.train.txt",
    "wikipedia": f"{WORK}/clean-v2/corpus/v2/wikipedia.train.txt",
    "un": f"{WORK}/clean-v2/corpus/v2/un.train.txt",
}
PROVENANCE = os.path.join(ROOT, "quality/corpus/agent-general-provenance.tsv")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--min", type=int, default=3)
    ap.add_argument("--out")
    args = ap.parse_args()
    rows = [r for r in csv.DictReader(open(PROVENANCE, encoding="utf-8"), delimiter="\t") if r["source"] == "phrase-layer-mined"]
    words = [r["text"] for r in rows]
    counts = {w: {} for w in words}
    for name, path in CLEAN_TEXTS.items():
        text = open(path, encoding="utf-8").read()
        for w in words:
            counts[w][name] = text.count(w)
        print(f"{name}: {len(text):,} characters scanned", file=sys.stderr, flush=True)
    kept = [w for w in words if sum(counts[w].values()) >= args.min]
    print(f"{len(words)} mined rows; {len(kept)} occur at least {args.min} times in clean text; {len(words) - len(kept)} do not")
    if args.out:
        with open(args.out, "w", encoding="utf-8") as fh:
            fh.write("text\ttotal\t" + "\t".join(CLEAN_TEXTS) + "\tclean\n")
            for r in rows:
                w = r["text"]
                fh.write(f"{w}\t{sum(counts[w].values())}\t" + "\t".join(str(counts[w][n]) for n in CLEAN_TEXTS) + f"\t{int(w in set(kept))}\n")


if __name__ == "__main__":
    main()
