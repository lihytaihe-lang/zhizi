"""Build homophone-choice training examples from the IME's own dictionaries.

Homophone groups come from the shipped Rime tables (same reading -> words by
weight), so the distractors are exactly what the IME would put next to the
right word. For each corpus line we read it with pypinyin, find a word whose
reading group has >= 2 members, and emit (preceding context, candidates, gold).

Output: <out>/<split>.jsonl  {"ctx": str, "cands": [str], "gold": int, "src": str}
"""

import argparse
import glob
import json
import os
import random
import re
from collections import defaultdict
from multiprocessing import Pool

from pypinyin import Style, lazy_pinyin

RIME = os.path.expanduser("~/Library/Input Methods/Agent Input Method.app/Contents/Resources/Rime")
EXCLUDE = ()  # dictionary file name prefixes left out of the homophone groups (--exclude); the release build leaves out generated layers
CJK = re.compile(r"^[一-鿿]+$")
GROUPS = None
VOCAB = None


def load_groups(max_cands):
    entries = defaultdict(dict)
    for path in glob.glob(os.path.join(RIME, "*.dict.yaml")):
        if os.path.basename(path).startswith(EXCLUDE):
            continue
        body = False
        for line in open(path, encoding="utf-8"):
            if line.startswith("..."):
                body = True
                continue
            if not body or line.startswith("#"):
                continue
            parts = line.rstrip("\n").split("\t")
            if len(parts) < 2 or not CJK.match(parts[0]) or len(parts[0]) > 3:
                continue
            word, reading = parts[0], parts[1]
            if len(reading.split()) != len(word):
                continue
            weight = float(parts[2]) if len(parts) > 2 and parts[2].replace(".", "").isdigit() else 0.0
            entries[reading][word] = max(weight, entries[reading].get(word, 0.0))
    groups = {}
    for reading, words in entries.items():
        ranked = [w for w, _ in sorted(words.items(), key=lambda kv: -kv[1])]
        ranked = [w for w in ranked if all(c in VOCAB for c in w)][:max_cands]
        if len(ranked) >= 2:
            groups[reading] = ranked
    return groups


def init(vocab_path, max_cands, rime=None, exclude=()):
    global GROUPS, VOCAB, RIME, EXCLUDE
    RIME, EXCLUDE = rime or RIME, tuple(exclude)
    VOCAB = set(json.load(open(vocab_path, encoding="utf-8")))
    GROUPS = load_groups(max_cands)


def examples_from(args):
    path, src, per_line, max_ctx, seed, limit = args
    rng = random.Random(seed)
    out = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f):
            if limit and n >= limit:
                break
            line = line.rstrip("\n")
            if len(line) < 6:
                continue
            py = lazy_pinyin(line, style=Style.NORMAL, errors=lambda x: ["?"] * len(x))
            if len(py) != len(line):
                continue
            spots = []
            for p in range(1, len(line)):
                for L in (2, 1, 3):
                    word = line[p:p + L]
                    if len(word) < L or not CJK.match(word):
                        continue
                    group = GROUPS.get(" ".join(py[p:p + L]))
                    if group and word in group:
                        spots.append((p, word, group))
                        break
            for p, word, group in rng.sample(spots, min(per_line, len(spots))):
                ctx = line[max(0, p - rng.randint(4, max_ctx)):p]
                out.append({"ctx": ctx, "cands": group, "gold": group.index(word), "src": src})
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True, help="dir with <src>.{train,valid}.txt and vocab.json")
    ap.add_argument("--sources", default="wikipedia:200000,un:50000,asap:0,tatoeba:0,synth:0",
                    help="src:max_lines (0 = all)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--per-line", type=int, default=2)
    ap.add_argument("--max-ctx", type=int, default=32)
    ap.add_argument("--max-cands", type=int, default=8)
    ap.add_argument("--rime", default=None, help="Rime resources dir holding the *.dict.yaml (default: the installed IME)")
    ap.add_argument("--exclude", default="", help="comma list of dictionary file name prefixes to leave out, e.g. agent_spoken,agent_pack")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    srcs = [(s.split(":")[0], int(s.split(":")[1])) for s in args.sources.split(",")]
    for split in ("train", "valid"):
        jobs = [(os.path.join(args.corpus, f"{s}.{split}.txt"), s, args.per_line, args.max_ctx, i, lim)
                for i, (s, lim) in enumerate(srcs) if os.path.exists(os.path.join(args.corpus, f"{s}.{split}.txt"))]
        with Pool(min(10, len(jobs)), initializer=init,
                  initargs=(os.path.join(args.corpus, "vocab.json"), args.max_cands, args.rime,
                                [x for x in args.exclude.split(",") if x])) as pool:
            with open(os.path.join(args.out, f"{split}.jsonl"), "w", encoding="utf-8") as f:
                n = 0
                for rows in pool.imap_unordered(examples_from, jobs):
                    for r in rows:
                        f.write(json.dumps(r, ensure_ascii=False) + "\n")
                    n += len(rows)
        print(split, n, flush=True)


if __name__ == "__main__":
    main()
