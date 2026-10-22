"""Pick accuracy of a 顺手·知字 checkpoint on rerank rows (MODEL-V3-PLAN), per typing bucket.

  python eval_rerank.py <checkpoint.pt> <valid.jsonl> [--k-max 64] [--limit N]

Scores every candidate the way the IME does (sum of log P(candidate | context)) and counts how often the
gold one scores highest, over the rows the core could offer the answer for. Also prints how often the
core's own first candidate was already right (the order the model has to improve on).
Local only: public / synthetic rows, no owner data.
"""
import argparse
import collections
import json
import sys
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, str(WORK_DIR / 'src'))
import torch  # noqa: E402

from model import load  # noqa: E402
from train import pick_device  # noqa: E402
from train_homophone import candidate_loss, encode_examples  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("checkpoint")
    ap.add_argument("valid")
    ap.add_argument("--k-max", type=int, default=64)
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--device", default="auto")
    ap.add_argument("--chunk", type=int, default=32)
    args = ap.parse_args()
    device = pick_device(args.device)
    model, vocab, _ = load(args.checkpoint, device)
    model.eval()
    raw = [json.loads(line) for line in open(args.valid, encoding="utf-8")]
    if args.limit:
        raw = raw[: args.limit]
    encoded = encode_examples(args.valid, vocab, limit=args.limit)
    by_bucket = collections.defaultdict(list)
    for row, enc in zip(raw, encoded):
        by_bucket[row["bucket"]].append((row, enc))
    total = [0.0, 0, 0]
    print("bucket\trows\tcore_first\tmodel_pick")
    with torch.no_grad():
        for bucket, items in sorted(by_bucket.items()):
            hits, n = 0.0, 0
            for i in range(0, len(items), args.chunk):
                chunk = items[i:i + args.chunk]
                _, acc = candidate_loss(model, [enc for _, enc in chunk], device, args.k_max)
                hits += acc * len(chunk)
                n += len(chunk)
            first = sum(row["gold"] == 0 for row, _ in items)
            print(f"{bucket}\t{n}\t{100 * first / n:.1f}%\t{100 * hits / n:.1f}%")
            total[0] += hits; total[1] += n; total[2] += first
    print(f"all\t{total[1]}\t{100 * total[2] / total[1]:.1f}%\t{100 * total[0] / total[1]:.1f}%")


if __name__ == "__main__":
    main()
