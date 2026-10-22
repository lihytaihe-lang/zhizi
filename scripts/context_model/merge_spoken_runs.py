"""Merge the phrases.tsv of several spoken_phrases.py runs: counts add up (each count is a number of independent calls that produced the
phrase) and the backend lists are united.

  python merge_spoken_runs.py <out phrases.tsv> <run dir or phrases.tsv> [...]

A run directory is rebuilt from its <backend>.jsonl call records (a run that was stopped early has no phrases.tsv).
"""
import glob
import json
import os
import sys
from collections import defaultdict


def main():
    out, sources = sys.argv[1], sys.argv[2:]
    count, backends = defaultdict(int), defaultdict(set)
    for source in sources:
        records = sorted(glob.glob(os.path.join(source, "*.jsonl"))) if os.path.isdir(source) else []
        if records:
            for path in records:
                backend = os.path.basename(path)[:-len(".jsonl")]
                for line in open(path, encoding="utf-8"):
                    for phrase in set(json.loads(line)["lines"]):
                        count[phrase] += 1
                        backends[phrase].add(backend)
            continue
        path = os.path.join(source, "phrases.tsv") if os.path.isdir(source) else source
        for line in open(path, encoding="utf-8"):
            phrase, n, names = line.rstrip("\n").split("\t")
            count[phrase] += int(n)
            backends[phrase].update(filter(None, names.split(",")))
    with open(out, "w", encoding="utf-8") as fh:
        for phrase in sorted(count, key=lambda p: (-count[p], p)):
            fh.write(f"{phrase}\t{count[phrase]}\t{','.join(sorted(backends[phrase]))}\n")
    print(f"{len(count)} phrases from {len(sources)} runs")


if __name__ == "__main__":
    main()
