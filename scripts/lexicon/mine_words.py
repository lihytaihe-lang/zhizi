"""Word discovery from running text: strings of 2-4 Han characters that stick together and are not in any shipped dictionary.

  ime_context_model/.venv/bin/python scripts/lexicon/mine_words.py --out <dir> [--min-count 12] [text files ...]

(The gap this closes: 柴犬 and 白兔 were in no dictionary, which no list of "common words" had caught - owner 2026-10-17.)
For every 2-4 character string with at least `--min-count` occurrences it computes, over the whole text,
  - cohesion: the smallest, over all splits, of log( P(w) / (P(left) P(right)) )  (pointwise mutual information),
  - freedom: the smaller of the left and right neighbour entropies (bits): a word has many different characters on both sides.
It writes candidates.tsv (text, count, cohesion, freedom) for the strings that are in no shipped dictionary and pass both bars.
This is only the statistical half: gen_new_words-style review (judge_mined.py) decides which of them are words people type.
"""
import argparse
import math
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_general_lexicon import existing_entries  # noqa: E402

WORK = str(WORK_DIR)
BASE = 21000  # a character id is cp - 0x4E00 + 1 (< 21000); id 0 separates runs of Han characters
DEFAULT_TEXTS = [os.path.join(WORK, "corpus/v1/wikipedia.train.txt"), os.path.join(WORK, "corpus/v2/synth.train.txt")]


def han_ids(path):
    text = open(path, "rb").read().decode("utf-8", errors="ignore")
    codes = np.frombuffer(text.encode("utf-32-le"), dtype=np.uint32).astype(np.int64)
    ids = np.where((codes >= 0x4E00) & (codes <= 0x9FFF), codes - 0x4E00 + 1, 0).astype(np.int64)
    return ids


def grams(ids, n):
    """Codes of all n-grams made only of Han characters (position i = ids[i:i+n])."""
    size = len(ids) - n + 1
    code = np.zeros(size, dtype=np.int64)
    valid = np.ones(size, dtype=bool)
    for k in range(n):
        part = ids[k:k + size]
        code = code * BASE + part
        valid &= part > 0
    return code, valid


def entropy(groups, counts):
    """Entropy (bits) per candidate from (candidate index, neighbour) counts."""
    result = np.zeros(groups.max() + 1 if len(groups) else 0)
    totals = np.bincount(groups, weights=counts)
    p = counts / totals[groups]
    np.add.at(result, groups, -p * np.log2(p))
    return result


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("texts", nargs="*", default=DEFAULT_TEXTS)
    ap.add_argument("--out", required=True)
    ap.add_argument("--min-count", type=int, default=12)
    ap.add_argument("--min-cohesion", type=float, default=4.0)
    ap.add_argument("--min-freedom", type=float, default=1.2)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    ids = np.concatenate([np.concatenate([han_ids(p), np.zeros(8, dtype=np.int64)]) for p in args.texts])
    total = int((ids > 0).sum())
    print(f"{len(ids)} positions, {total} Han characters", flush=True)
    shipped = set(existing_entries())

    unigram = np.bincount(ids, minlength=BASE + 1).astype(np.float64)
    tables = {1: (np.arange(BASE + 1, dtype=np.int64), unigram)}
    seen = {}
    for n in (2, 3, 4):
        code, valid = grams(ids, n)
        unique, counts = np.unique(code[valid], return_counts=True)
        keep = counts >= 2  # sub-gram lookups need the small counts too, but not the long tail of singletons
        tables[n] = (unique[keep], counts[keep].astype(np.float64))
        print(f"n={n}: {len(unique)} distinct, {int(keep.sum())} with >=2", flush=True)

    def count_of(n, codes):
        unique, counts = tables[n]
        at = np.searchsorted(unique, codes)
        at[at >= len(unique)] = len(unique) - 1
        found = unique[at] == codes
        return np.where(found, counts[at], 1.0)

    rows = []
    for n in (2, 3, 4):
        unique, counts = tables[n]
        sel = counts >= args.min_count
        cand, c = unique[sel], counts[sel]
        # cohesion
        cohesion = np.full(len(cand), np.inf)
        for k in range(1, n):
            left, right = cand // BASE ** (n - k), cand % BASE ** (n - k)
            pmi = np.log2(c * total / (count_of(k, left) * count_of(n - k, right)))
            cohesion = np.minimum(cohesion, pmi)
        # neighbour entropies: scan the positions where a candidate occurs
        code, valid = grams(ids, n)
        position = np.nonzero(valid)[0]
        at = np.searchsorted(cand, code[position])
        at[at >= len(cand)] = len(cand) - 1
        hit = cand[at] == code[position]
        position, index = position[hit], at[hit]
        freedoms = []
        for side in (-1, n):
            neighbour = ids[np.clip(position + side, 0, len(ids) - 1)]
            pair = index.astype(np.int64) * BASE + neighbour
            u, cnt = np.unique(pair, return_counts=True)
            freedoms.append(entropy(u // BASE, cnt.astype(np.float64)))
        size = len(cand)
        freedom = np.minimum(np.pad(freedoms[0], (0, size - len(freedoms[0]))), np.pad(freedoms[1], (0, size - len(freedoms[1]))))
        for i in np.nonzero((cohesion >= args.min_cohesion) & (freedom >= args.min_freedom))[0]:
            digits, value = [], int(cand[i])
            for _ in range(n):
                digits.append(chr(value % BASE - 1 + 0x4E00))
                value //= BASE
            word = "".join(reversed(digits))
            if word not in shipped:
                rows.append((word, int(c[i]), round(float(cohesion[i]), 2), round(float(freedom[i]), 2)))
        print(f"n={n}: {len(cand)} frequent, {sum(1 for r in rows if len(r[0]) == n)} candidates not shipped", flush=True)
    rows.sort(key=lambda r: -r[1])
    with open(os.path.join(args.out, "candidates.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\tcount\tcohesion\tfreedom\n")
        for r in rows:
            fh.write("\t".join(str(v) for v in r) + "\n")
    probe = {r[0]: r for r in rows}
    print("柴犬", probe.get("柴犬"), "白兔", probe.get("白兔"))
    print(f"{len(rows)} candidates")


if __name__ == "__main__":
    main()
