"""Pretrain 顺手·知字. Same code on the Mac (mps, debugging) and a rented CUDA GPU.

Sources are sampled by --mix weights rather than by size, so the small
colloquial sets (reviews, everyday sentences) are not drowned by Wikipedia.
"""

import argparse
import json
import math
import os
import time

import numpy as np
import torch

from model import PRESETS, Config, Vocab, ZhiziLM, save


def pick_device(name):
    if name != "auto":
        return name
    if torch.cuda.is_available():
        return "cuda"
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--preset", default="30m", choices=PRESETS)
    ap.add_argument("--block", type=int, default=128)
    ap.add_argument("--batch", type=int, default=64)
    ap.add_argument("--steps", type=int, default=20000)
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument("--warmup", type=int, default=500)
    ap.add_argument("--eval-every", type=int, default=500)
    ap.add_argument("--mix", default="wikipedia:0.5,un:0.15,asap:0.3,tatoeba:0.05")
    ap.add_argument("--device", default="auto")
    ap.add_argument("--seed", type=int, default=1337)
    ap.add_argument("--compile", action="store_true")
    args = ap.parse_args()

    device = pick_device(args.device)
    torch.manual_seed(args.seed)
    vocab_path = os.path.join(args.corpus, "vocab.json")
    vocab = Vocab(vocab_path)
    mix = {k: float(v) for k, v in (p.split(":") for p in args.mix.split(","))}
    data = {split: {s: np.memmap(os.path.join(args.corpus, f"{s}.{split}.bin"), dtype=np.uint16, mode="r")
                    for s in mix} for split in ("train", "valid")}
    names, weights = list(mix), np.array(list(mix.values())) / sum(mix.values())
    rng = np.random.default_rng(args.seed)

    def batch(split, source=None):
        srcs = [source] * args.batch if source else rng.choice(names, size=args.batch, p=weights)
        rows = []
        for s in srcs:
            arr = data[split][s]
            o = rng.integers(0, len(arr) - args.block - 1)
            rows.append(np.asarray(arr[o:o + args.block + 1], dtype=np.int64))
        t = torch.from_numpy(np.stack(rows)).to(device)
        return t[:, :-1], t[:, 1:]

    config = Config(vocab_size=len(vocab), block_size=args.block, **PRESETS[args.preset])
    model = ZhiziLM(config).to(device)
    print(f"device={device} params={model.n_params() / 1e6:.1f}M vocab={len(vocab)}", flush=True)
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, betas=(0.9, 0.95), weight_decay=0.1)
    fwd = torch.compile(model) if args.compile else model
    amp = torch.autocast("cuda", dtype=torch.bfloat16) if device == "cuda" else torch.autocast("cpu", enabled=False)

    def lr_at(step):
        if step < args.warmup:
            return args.lr * (step + 1) / args.warmup
        p = (step - args.warmup) / max(1, args.steps - args.warmup)
        return args.lr * (0.1 + 0.9 * 0.5 * (1 + math.cos(math.pi * p)))

    @torch.no_grad()
    def evaluate():
        model.eval()
        out = {}
        for s in names:
            losses = []
            for _ in range(10):
                x, y = batch("valid", s)
                with amp:
                    losses.append(fwd(x, y)[1].item())
            out[s] = round(sum(losses) / len(losses), 4)
        model.train()
        return out

    os.makedirs(args.out, exist_ok=True)
    log = open(os.path.join(args.out, "log.jsonl"), "a")
    best = float("inf")
    started = time.time()
    for step in range(args.steps + 1):
        if step % args.eval_every == 0:
            v = evaluate()
            mean = sum(v[s] * mix[s] for s in names) / sum(mix.values())
            rec = {"step": step, "valid": v, "mean": round(mean, 4), "elapsed": round(time.time() - started)}
            print(json.dumps(rec, ensure_ascii=False), flush=True)
            log.write(json.dumps(rec, ensure_ascii=False) + "\n")
            log.flush()
            if mean < best:
                best = mean
                save(os.path.join(args.out, "best.pt"), model, vocab_path, step, {"valid": v})
        if step == args.steps:
            break
        for g in opt.param_groups:
            g["lr"] = lr_at(step)
        x, y = batch("train")
        with amp:
            _, loss = fwd(x, y)
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
    save(os.path.join(args.out, "last.pt"), model, vocab_path, args.steps)


if __name__ == "__main__":
    main()
