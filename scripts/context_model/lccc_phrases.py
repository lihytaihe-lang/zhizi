"""Frequent spoken fragments from LCCC-base (Weibo dialogues, already word-segmented), for the phrase layer.

Counts runs of 1-4 consecutive segmenter words inside a clause (clauses end at punctuation) whose joined text is 2-6
Han characters, keeps the frequent ones and marks which are already dictionary entries. Only phrase + count are
written - no dialogue text is stored. Research data: LCCC's README restricts the dataset to research use, so what may
ship from this (counts of short fragments) is an open licensing question recorded in NEXT-ORDER, not decided here.

  python lccc_phrases.py <lccc_base_train.jsonl.gz> <Rime resources dir> <out.tsv> [--dialogues 1000000] [--min-count 40]
"""
import argparse
import collections
import glob
import gzip
import json
import os
import re

HAN = re.compile(r"^[一-鿿]+$")
CLAUSE_BREAK = re.compile(r"[，。！？、；：,.!?;:…~～\s]")


def dictionary(res):
    words = set()
    for path in glob.glob(os.path.join(res, "*.dict.yaml")):
        body = open(path, encoding="utf-8").read().split("\n...\n", 1)
        if len(body) < 2:
            continue
        for line in body[1].splitlines():
            parts = line.split("\t")
            if len(parts) >= 2 and parts[0] and not parts[0].startswith("#"):
                words.add(parts[0])
    return words


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("gz")
    ap.add_argument("res")
    ap.add_argument("out")
    ap.add_argument("--dialogues", type=int, default=1000000)
    ap.add_argument("--min-count", type=int, default=40)
    ap.add_argument("--max-words", type=int, default=4)
    args = ap.parse_args()
    counts = collections.Counter()
    done = 0
    with gzip.open(args.gz, "rt", encoding="utf-8") as fh:
        for line in fh:
            if done >= args.dialogues:
                break
            done += 1
            for turn in json.loads(line):
                # A clause is a run of segmenter words with no punctuation in between.
                clause = []
                for tok in turn.split(" ") + ["，"]:
                    if tok and HAN.match(tok):
                        clause.append(tok)
                        continue
                    for i in range(len(clause)):
                        text = ""
                        for j in range(i, min(len(clause), i + args.max_words)):
                            text += clause[j]
                            if len(text) > 6:
                                break
                            if len(text) >= 2:
                                counts[text] += 1
                    clause = []
            if done % 100000 == 0:  # keep memory bounded: singletons and doubletons never matter
                counts = collections.Counter({k: v for k, v in counts.items() if v >= 3})
                print(f"{done} dialogues, {len(counts)} kept", flush=True)
    known = dictionary(args.res)
    rows = [(p, n) for p, n in counts.items() if n >= args.min_count]
    rows.sort(key=lambda r: -r[1])
    with open(args.out, "w", encoding="utf-8") as fh:
        for phrase, n in rows:
            fh.write(f"{phrase}\t{n}\t{int(phrase in known)}\n")
    print(f"phrases>={args.min_count}: {len(rows)}; not in dictionary: {sum(1 for p, _ in rows if p not in known)}")


if __name__ == "__main__":
    main()
