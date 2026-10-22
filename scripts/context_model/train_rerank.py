"""Memory-lean rerank trainer for 顺手·知字 (MODEL-V3-PLAN, S2 trial).

Same objective as ime_context_model/src/train_homophone.py: score each candidate as the sum of
log P(candidate chars | <bos> + context), softmax over the row's candidates, cross-entropy to the gold one.
What differs is memory. The original builds logits for every position of every candidate over the whole
vocabulary (4000 sequences x 62 positions x 7745 x 4 bytes = 7.7 GB per 128-row batch), which on 2026-10-12
filled the Mac's memory and froze it. Here the vocabulary projection is applied ONLY at the candidate
positions, rows are processed in small groups, MPS allocations are capped, and a watchdog stops the run when the
machine starts swapping. Local use: public / synthetic rows only, no owner data.

  python train_rerank.py --init best.pt --data DIR(train.jsonl,valid.jsonl) --out RUNDIR [--steps 600]
"""
import argparse
import json
import math
import os
import random
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, str(WORK_DIR / 'src'))
os.environ.setdefault("PYTORCH_MPS_HIGH_WATERMARK_RATIO", "0.5")
os.environ.setdefault("PYTORCH_MPS_LOW_WATERMARK_RATIO", "0.3")
import torch  # noqa: E402
import torch.nn.functional as F  # noqa: E402

from model import load, save  # noqa: E402
from train import pick_device  # noqa: E402

SWAP_GROWTH_LIMIT_MB = 1500  # stop when the run itself makes the machine swap (swap left over from before does not count)
SWAP_AT_START = None
CTX_TOKENS = 24


def swap_used_mb():
    out = subprocess.run(["sysctl", "-n", "vm.swapusage"], capture_output=True, text=True).stdout
    for part in out.split("  "):
        if part.startswith("used = "):
            return float(part.split("=")[1].strip().rstrip("M"))
    return 0.0


def guard():
    global SWAP_AT_START
    used = swap_used_mb()
    if SWAP_AT_START is None:
        SWAP_AT_START = used
    if used - SWAP_AT_START > SWAP_GROWTH_LIMIT_MB:
        print(f"swap grew {used - SWAP_AT_START:.0f} MB during the run: stopping so the machine does not stall", flush=True)
        sys.exit(3)


def encode_rows(path, vocab, k_max, limit=0):
    rows = []
    with open(path, encoding="utf-8") as fh:
        for i, line in enumerate(fh):
            if limit and i >= limit:
                break
            r = json.loads(line)
            cands, gold = r["cands"], r["gold"]
            if gold >= len(cands):
                continue
            if len(cands) > k_max:  # keep the gold and the first (hardest, core-ordered) others
                keep = [c for j, c in enumerate(cands) if j != gold][: k_max - 1]
                keep.insert(min(gold, len(keep)), cands[gold])
                gold, cands = keep.index(cands[gold]) if cands[gold] in keep else 0, keep
            ctx = ([vocab.bos] + vocab.encode(r["ctx"]))[-CTX_TOKENS:]
            rows.append({"ctx": ctx, "cands": [vocab.encode(c) for c in cands], "gold": gold, "bucket": r.get("bucket", "")})
    return rows


def hidden(model, x):
    t = x.shape[1]
    h = model.tok(x) + model.pos(torch.arange(t, device=x.device))
    for blk in model.blocks:
        h = blk(h)
    return model.ln_f(h)


def row_scores(model, rows, device):
    """Sum of log P over each candidate's own characters, for every candidate of every row."""
    seqs, owner, spans = [], [], []
    for b, r in enumerate(rows):
        for c in r["cands"]:
            seqs.append(r["ctx"] + c)
            owner.append(b)
            spans.append((len(r["ctx"]), len(r["ctx"]) + len(c)))
    width = max(len(s) for s in seqs)
    x = torch.tensor([s + [0] * (width - len(s)) for s in seqs], device=device)
    h = hidden(model, x[:, :-1])
    # Position j predicts token j + 1: the candidate's characters sit at positions start-1 .. end-2.
    pos = torch.arange(width - 1, device=device)[None, :]
    start = torch.tensor([s - 1 for s, _ in spans], device=device)[:, None]
    end = torch.tensor([e - 1 for _, e in spans], device=device)[:, None]
    mask = (pos >= start) & (pos < end)
    picked = h[mask]  # [candidate characters, d_model]
    logits = model.head(picked).float()
    target = x[:, 1:][mask]
    lp = logits.log_softmax(-1).gather(-1, target[:, None])[:, 0]
    seq_index = mask.nonzero()[:, 0]
    scores = torch.zeros(len(seqs), device=device).index_add_(0, seq_index, lp)
    k = max(len(r["cands"]) for r in rows)
    table = torch.full((len(rows), k), float("-inf"), device=device)
    counters = [0] * len(rows)
    for i, b in enumerate(owner):
        table[b, counters[b]] = scores[i]
        counters[b] += 1
    return table


def loss_and_acc(model, rows, device):
    table = row_scores(model, rows, device)
    gold = torch.tensor([r["gold"] for r in rows], device=device)
    return F.cross_entropy(table, gold), (table.argmax(1) == gold).float().mean().item(), table.argmax(1).tolist()


@torch.no_grad()
def evaluate(model, rows, device, chunk):
    model.eval()
    # Only rows with a context count: without one the model does not decide in the product (Rime's frequency does).
    hits, per = 0, {}
    for i in range(0, len(rows), chunk):
        part = rows[i:i + chunk]
        _, _, picks = loss_and_acc(model, part, device)
        for r, pick in zip(part, picks):
            if len(r["ctx"]) <= 1:  # just <bos>
                continue
            ok = pick == r["gold"]
            hits += ok
            h = per.setdefault(r["bucket"], [0, 0])
            h[0] += ok
            h[1] += 1
    model.train()
    total = sum(h[1] for h in per.values())
    return hits / max(1, total), {b: round(h[0] / h[1], 4) for b, h in per.items()} | {"rows_with_context": total}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--init", required=True)
    ap.add_argument("--data", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--steps", type=int, default=600)
    ap.add_argument("--rows", type=int, default=6, help="rows per optimiser micro-batch")
    ap.add_argument("--accum", type=int, default=2)
    ap.add_argument("--lr", type=float, default=1e-4)
    ap.add_argument("--k-max", type=int, default=40)
    ap.add_argument("--eval-every", type=int, default=100)
    ap.add_argument("--eval-rows", type=int, default=600)
    ap.add_argument("--eval-chunk", type=int, default=6)
    ap.add_argument("--train-limit", type=int, default=0)
    ap.add_argument("--device", default="auto")
    args = ap.parse_args()

    guard()
    device = pick_device(args.device)
    model, vocab, _ = load(args.init, device)
    model.train()
    train = encode_rows(os.path.join(args.data, "train.jsonl"), vocab, args.k_max, args.train_limit)
    valid = encode_rows(os.path.join(args.data, "valid.jsonl"), vocab, 64, args.eval_rows)
    print(f"device={device} train={len(train)} valid={len(valid)} swap={swap_used_mb():.0f}MB", flush=True)
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, betas=(0.9, 0.95), weight_decay=0.1)
    rng = random.Random(0)
    os.makedirs(args.out, exist_ok=True)
    log = open(os.path.join(args.out, "log.jsonl"), "a")
    best, started = -1.0, time.time()
    for step in range(args.steps + 1):
        if step % args.eval_every == 0:
            guard()
            acc, per = evaluate(model, valid, device, args.eval_chunk)
            rec = {"step": step, "valid_pick_acc": round(acc, 4), "by_bucket": per, "elapsed": round(time.time() - started),
                   "swap_mb": round(swap_used_mb())}
            print(json.dumps(rec, ensure_ascii=False), flush=True)
            log.write(json.dumps(rec, ensure_ascii=False) + "\n")
            log.flush()
            if acc > best:
                best = acc
                save(os.path.join(args.out, "best.pt"), model, str(WORK_DIR / 'corpus/v2/vocab.json'), step,
                     {"valid_pick_acc": acc})
        if step == args.steps:
            break
        guard()
        lr = args.lr * min(1.0, (step + 1) / 50) * (0.1 + 0.9 * 0.5 * (1 + math.cos(math.pi * step / args.steps)))
        for g in opt.param_groups:
            g["lr"] = lr
        opt.zero_grad(set_to_none=True)
        for _ in range(args.accum):
            loss, _, _ = loss_and_acc(model, rng.sample(train, args.rows), device)
            (loss / args.accum).backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        if step % 10 == 0:
            print(f"step {step} loss {loss.item():.3f} elapsed {round(time.time() - started)}s swap {round(swap_used_mb())}MB", flush=True)


if __name__ == "__main__":
    main()
