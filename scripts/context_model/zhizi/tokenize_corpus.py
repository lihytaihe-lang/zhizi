"""Encode corpus/<source>.{train,valid}.txt into uint16 token streams.
Paragraphs are separated by <bos>, so a training window that crosses a
paragraph boundary learns that context resets there."""

import argparse
import glob
import os

import numpy as np

from model import Vocab


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True)
    args = ap.parse_args()
    vocab = Vocab(os.path.join(args.corpus, "vocab.json"))
    assert len(vocab) < 65535
    for path in sorted(glob.glob(os.path.join(args.corpus, "*.txt"))):
        ids = []
        with open(path, encoding="utf-8") as f:
            for line in f:
                ids.append(vocab.bos)
                ids.extend(vocab.encode(line.rstrip("\n")))
        arr = np.array(ids, dtype=np.uint16)
        arr.tofile(path[:-4] + ".bin")
        unk = int((arr == vocab.unk).sum())
        print(f"{os.path.basename(path)} tokens={len(arr):,} unk={unk:,}", flush=True)


if __name__ == "__main__":
    main()
