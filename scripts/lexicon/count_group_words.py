#!/usr/bin/env python3
"""Corpus counts for the words of near-tie homophone groups (evidence for judge_homophone_order.py).

    scripts/lexicon/count_group_words.py --rime <Resources/Rime> --corpus-dir <clean-v2/corpus/v2> --out <tsv>

Colloquial = asap + tatoeba + synth (DeepSeek-only) train text; written = the first --wiki-mb
of the wikipedia train text.  The corpora are the clean-v2 set the published model was
trained on (docs/current/licenses/TRAINING-DATA-TERMS-2026-10-15.md); LCCC is not used.
Counts are raw substring occurrences of each word (2-4 Han characters).
"""
import argparse
import collections
import os
import sys
from multiprocessing import Pool

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from judge_homophone_order import shipped_weights  # noqa: E402

WORDS = set()


def init(words):
    global WORDS
    WORDS = words


def count_chunk(lines):
    counts = collections.Counter()
    for line in lines:
        n = len(line)
        for i in range(n - 1):
            for k in (2, 3, 4):
                if i + k <= n:
                    piece = line[i:i + k]
                    if piece in WORDS:
                        counts[piece] += 1
    return counts


def count_file(path, limit_bytes, pool):
    total = collections.Counter()
    read = 0

    def chunks():
        nonlocal read
        batch = []
        with open(path, encoding="utf-8", errors="ignore") as handle:
            for line in handle:
                read += len(line.encode("utf-8"))
                batch.append(line.rstrip("\n"))
                if len(batch) >= 4000:
                    yield batch
                    batch = []
                if limit_bytes and read >= limit_bytes:
                    break
        if batch:
            yield batch

    for counts in pool.imap_unordered(count_chunk, chunks()):
        total.update(counts)
    return total


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rime", required=True)
    ap.add_argument("--corpus-dir", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--wiki-mb", type=int, default=120)
    ap.add_argument("--ratio", type=float, default=2.0)
    ap.add_argument("--min-weight", type=int, default=100)
    ap.add_argument("--max-words", type=int, default=6)
    args = ap.parse_args()
    words = set()
    for _reading, ws in shipped_weights(args.rime).items():
        if len(ws) < 2:
            continue
        ranked = sorted(ws.items(), key=lambda item: (-item[1], item[0]))[:args.max_words]
        if ranked[0][1] >= args.min_weight and ranked[0][1] <= ranked[1][1] * args.ratio:
            words.update(w for w, _ in ranked)
    print(f"words {len(words)}", flush=True)
    colloquial, written = collections.Counter(), collections.Counter()
    with Pool(os.cpu_count() or 4, initializer=init, initargs=(words,)) as pool:
        for name in ("asap.train.txt", "tatoeba.train.txt", "synth.train.txt"):
            colloquial.update(count_file(os.path.join(args.corpus_dir, name), 0, pool))
            print("done", name, flush=True)
        written.update(count_file(os.path.join(args.corpus_dir, "wikipedia.train.txt"), args.wiki_mb * 1_000_000, pool))
        print("done wikipedia", flush=True)
    with open(args.out, "w", encoding="utf-8") as handle:
        handle.write("word\tcolloquial\twritten\n")
        for word in sorted(words):
            handle.write(f"{word}\t{colloquial[word]}\t{written[word]}\n")


if __name__ == "__main__":
    main()
