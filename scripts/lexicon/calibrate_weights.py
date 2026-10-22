"""Give new colloquial words a weight on the SAME scale as the shipped dictionaries, from how often they occur in chat-like text.

  ime_context_model/.venv/bin/python scripts/lexicon/calibrate_weights.py <kept.tsv from judge_mined.py> --out <dir> [texts ...]

Why: the new-word layers used flat weights (400 / 250), far below the base table's scale (说到 2,442, 似的 4,117, 淡水 346 ... up to six
figures), so a very common colloquial word that the base table lacks (是的, 对吧: 2026-10-17) could never outrank a rare word that it has.
Here each word's weight is looked up by QUANTILE: its count among the counts of the 2-3 character words that are shipped and occur in
the same texts gives a quantile, and the weight at that quantile of the shipped weights is used (rank mapping, so it does not depend on
the corpora's size or on a fitted curve).  Weights are clipped to [MIN_WEIGHT, MAX_WEIGHT].
Output: <out>/new_words.tsv in the format export_new_words.py reads (text, reading, year, domain, wiki, synth, deleted_votes) plus a
`weight` column that export_new_words.py honours for rows of the calibrated runs.
"""
import argparse
import csv
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_general_lexicon import existing_entries  # noqa: E402
from mine_words import BASE, grams, han_ids  # noqa: E402

WORK = str(WORK_DIR)
DEFAULT_TEXTS = [f"{WORK}/corpus/v1/asap.train.txt", f"{WORK}/clean-v2/corpus/v2/synth.train.txt", f"{WORK}/corpus/v2/tatoeba.train.txt"]
MIN_WEIGHT, MAX_WEIGHT = 120, 60000
# DeepSeek, asked for 「好不好」-style items, enumerated the same stem with every final particle (好不好咧 / 好不好咯 / 好不好哇 ...).  A word that is a
# known word plus one particle and hardly occurs in the texts is such an artefact, not a word anyone types.
PARTICLES = set("呀呢咧咯哇哈哟哦哩啊啦喽嘞嘛吧")
MECHANICAL_BELOW = 20
# Substring counts over-count words that also sit inside longer words (行的 inside 银行的), and the chat texts here are written, not spoken, so
# they under-count spoken replies (是的 occurs 287 times, 好的, which the base table weights 36,136, is far more common in real chat).  So calibrated
# weights stop at CAP, and the standalone replies are set by analogy to shipped words of the same kind: 好的 36,136, 好吧 8,222, 是吧 3,554.
CAP = 4000
from export_new_words import REPLY_WEIGHTS  # noqa: E402


def counts_of(ids, words):
    """Occurrences in `ids` of each word (2 or 3 Han characters)."""
    result = {}
    for n in (2, 3):
        wanted = [w for w in words if len(w) == n]
        if not wanted:
            continue
        code, valid = grams(ids, n)
        unique, counts = np.unique(code[valid], return_counts=True)
        for w in wanted:
            value = 0
            for ch in w:
                value = value * BASE + (ord(ch) - 0x4E00 + 1)
            at = np.searchsorted(unique, value)
            result[w] = int(counts[at]) if at < len(unique) and unique[at] == value else 0
    return result


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("kept")
    ap.add_argument("--out", required=True)
    ap.add_argument("texts", nargs="*", default=DEFAULT_TEXTS)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    ids = np.concatenate([np.concatenate([han_ids(p), np.zeros(8, dtype=np.int64)]) for p in args.texts])
    rows = list(csv.DictReader(open(args.kept, encoding="utf-8"), delimiter="\t"))
    shipped = existing_entries()
    shipped_words = [w for w, readings in shipped.items() if 2 <= len(w) <= 3 and all("一" <= c <= "鿿" for c in w)]
    shipped_count = counts_of(ids, shipped_words)
    pairs = sorted((shipped_count[w], max(weight for _, weight in shipped[w])) for w in shipped_words if shipped_count[w] >= 20)
    shipped_counts = np.array([p[0] for p in pairs], dtype=np.float64)
    shipped_weights = np.sort(np.array([p[1] for p in pairs], dtype=np.float64))
    print(f"{len(pairs)} shipped 2-3 character words occur at least 20 times; weights {shipped_weights[0]:.0f} .. {shipped_weights[-1]:.0f}, "
          f"median {np.median(shipped_weights):.0f}", flush=True)
    corr = np.corrcoef(np.log(shipped_counts), np.log(np.maximum(1, np.array([p[1] for p in pairs]))))[0, 1]
    print(f"log-log correlation of count and weight among shipped words: {corr:.2f}", flush=True)
    new_count = counts_of(ids, [r["text"] for r in rows])
    stems = {r["text"] for r in rows} | set(shipped)
    out, dropped = [], []
    for r in rows:
        count = new_count.get(r["text"], 0)
        if len(r["text"]) >= 3 and r["text"][-1] in PARTICLES and r["text"][:-1] in stems and count < MECHANICAL_BELOW:
            dropped.append(r["text"])
            continue
        quantile = np.searchsorted(shipped_counts, count) / len(shipped_counts)
        weight = int(np.clip(shipped_weights[min(len(shipped_weights) - 1, int(quantile * len(shipped_weights)))], MIN_WEIGHT, MAX_WEIGHT))
        weight = REPLY_WEIGHTS.get(r["text"], min(weight, CAP))
        out.append((r["text"], r["reading"], count, weight, quantile))
    out.sort(key=lambda x: -x[2])
    with open(os.path.join(args.out, "new_words.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\treading\tyear\tdomain\twiki\tsynth\tdeleted_votes\tweight\n")
        for text, reading, count, weight, _ in out:
            fh.write(f"{text}\t{reading}\t口语高频\t聊天语料\t0\t{count}\t0\t{weight}\n")
    for text, reading, count, weight, quantile in out[:25]:
        print(f"  {text} {reading} count={count} quantile={quantile:.2f} weight={weight}")
    print(f"{len(out)} words written; {len(dropped)} stem+particle artefacts dropped, e.g. {' '.join(dropped[:12])}")


if __name__ == "__main__":
    main()
