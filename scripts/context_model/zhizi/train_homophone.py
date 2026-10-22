"""Stage 2: sharpen a pretrained 顺手·知字 on homophone choice.

Each example scores its candidate set exactly the way the IME will
(sum of log P(candidate chars | <bos> + context)) and takes a softmax over the
candidates, so the training objective is the deployment decision. A plain LM
loss on the pretraining streams is mixed in so the model does not forget
ordinary Chinese while it specialises.
"""

import argparse
import json
import math
import os
import random
import time

import numpy as np
import torch
import torch.nn.functional as F

from model import load, save
from train import pick_device


def encode_examples(path, vocab, limit=0):
    rows = []
    with open(path, encoding="utf-8") as f:
        for i, line in enumerate(f):
            if limit and i >= limit:
                break
            r = json.loads(line)
            rows.append(([vocab.bos] + vocab.encode(r["ctx"]), [vocab.encode(c) for c in r["cands"]], r["gold"]))
    return rows


def candidate_loss(model, batch, device, k_max):
    seqs, owners, spans, golds = [], [], [], []
    for b, (ctx, cands, gold) in enumerate(batch):
        cands = cands[:k_max]
        if gold >= len(cands):
            continue
        golds.append(gold)
        for c in cands:
            seqs.append(ctx + c)
            owners.append((len(golds) - 1, len(cands)))
            spans.append((len(ctx), len(ctx) + len(c)))
    width = max(len(s) for s in seqs)
    x = torch.tensor([s + [0] * (width - len(s)) for s in seqs], device=device)
    logp = torch.log_softmax(model(x[:, :-1]).float(), dim=-1)
    tok = logp.gather(-1, x[:, 1:, None])[..., 0]
    pos = torch.arange(width - 1, device=device)[None, :]
    start = torch.tensor([s - 1 for s, _ in spans], device=device)[:, None]
    end = torch.tensor([e - 1 for _, e in spans], device=device)[:, None]
    scores = (tok * ((pos >= start) & (pos < end))).sum(1)
    n = len(golds)
    table = torch.full((n, k_max), float("-inf"), device=device)
    counters = [0] * n
    for i, (e, _) in enumerate(owners):
        table[e, counters[e]] = scores[i]
        counters[e] += 1
    gold_t = torch.tensor(golds, device=device)
    loss = F.cross_entropy(table, gold_t)
    acc = (table.argmax(1) == gold_t).float().mean().item()
    return loss, acc


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--init", required=True, help="pretrained checkpoint")
    ap.add_argument("--data", required=True, help="dir with train.jsonl / valid.jsonl")
    ap.add_argument("--corpus", required=True, help="pretraining corpus dir for the LM loss")
    ap.add_argument("--out", required=True)
    ap.add_argument("--steps", type=int, default=6000)
    ap.add_argument("--batch", type=int, default=64)
    ap.add_argument("--lm-batch", type=int, default=32)
    ap.add_argument("--lm-weight", type=float, default=0.5)
    ap.add_argument("--lr", type=float, default=3e-4)
    ap.add_argument("--k-max", type=int, default=8)
    ap.add_argument("--eval-every", type=int, default=500)
    ap.add_argument("--mix", default="wikipedia:0.4,un:0.1,asap:0.2,tatoeba:0.05,synth:0.25")
    ap.add_argument("--device", default="auto")
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()

    device = pick_device(args.device)
    model, vocab, ck = load(args.init, device)
    model.train()
    train_rows = encode_examples(os.path.join(args.data, "train.jsonl"), vocab)
    valid_rows = encode_examples(os.path.join(args.data, "valid.jsonl"), vocab, limit=4000)
    mix = {k: float(v) for k, v in (p.split(":") for p in args.mix.split(","))}
    streams = {s: np.memmap(os.path.join(args.corpus, f"{s}.train.bin"), dtype=np.uint16, mode="r")
               for s in mix if os.path.exists(os.path.join(args.corpus, f"{s}.train.bin"))}
    names = list(streams)
    weights = np.array([mix[s] for s in names]) / sum(mix[s] for s in names)
    rng, nrng = random.Random(args.seed), np.random.default_rng(args.seed)
    block = model.config.block_size
    print(f"device={device} train={len(train_rows)} valid={len(valid_rows)} lm_streams={names}", flush=True)

    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, betas=(0.9, 0.95), weight_decay=0.1)
    amp = torch.autocast("cuda", dtype=torch.bfloat16) if device == "cuda" else torch.autocast("cpu", enabled=False)

    @torch.no_grad()
    def evaluate():
        model.eval()
        accs = []
        for i in range(0, len(valid_rows), 128):
            with amp:
                accs.append(candidate_loss(model, valid_rows[i:i + 128], device, args.k_max)[1])
        model.train()
        return round(sum(accs) / len(accs), 4)

    os.makedirs(args.out, exist_ok=True)
    log = open(os.path.join(args.out, "log.jsonl"), "a")
    best, started = -1.0, time.time()
    for step in range(args.steps + 1):
        if step % args.eval_every == 0:
            acc = evaluate()
            rec = {"step": step, "valid_homophone_acc": acc, "elapsed": round(time.time() - started)}
            print(json.dumps(rec), flush=True)
            log.write(json.dumps(rec) + "\n")
            log.flush()
            if acc > best:
                best = acc
                save(os.path.join(args.out, "best.pt"), model, os.path.join(args.corpus, "vocab.json"), step, {"valid_homophone_acc": acc})
        if step == args.steps:
            break
        lr = args.lr * min(1.0, (step + 1) / 200) * (0.1 + 0.9 * 0.5 * (1 + math.cos(math.pi * step / args.steps)))
        for g in opt.param_groups:
            g["lr"] = lr
        with amp:
            loss, _ = candidate_loss(model, rng.sample(train_rows, args.batch), device, args.k_max)
            if args.lm_weight > 0 and names:
                rows = []
                for s in nrng.choice(names, size=args.lm_batch, p=weights):
                    arr = streams[s]
                    o = nrng.integers(0, len(arr) - block - 1)
                    rows.append(np.asarray(arr[o:o + block + 1], dtype=np.int64))
                t = torch.from_numpy(np.stack(rows)).to(device)
                loss = loss + args.lm_weight * model(t[:, :-1], t[:, 1:])[1]
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()


if __name__ == "__main__":
    main()
