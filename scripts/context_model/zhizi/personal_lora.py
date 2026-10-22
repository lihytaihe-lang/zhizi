"""Personal LoRA experiment for 顺手·知字: does a few-MB adapter trained on the owner's own cleaned
choices beat the shared base model on LATER days it never saw?

Runs only on the Mac; the owner's records never leave it. Base weights stay frozen.
Training signal: homophone choices (same length, same reading, as in probe_rerank.homophone_set), gold = what the
owner committed.  Cleaning: events followed by a redo or a delete, and abandoned ones, are dropped (they are
the likeliest silent wrong characters).  A general-corpus LM loss anchors the adapter so ordinary Chinese holds.
Report (test = days >= --cutoff): accuracy of the IME's own order, the base model, the base + adapter, and the
fixes / breaks between base and base + adapter, split into "owner picked a non-first candidate" and the rest.

  python personal_lora.py --init train-v3/soup30/best.pt --out runs/lora-exp1 --cutoff 10-13
"""
import argparse
import datetime
import json
import math
import os
import random
import sys
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

import numpy as np

os.environ.setdefault("PYTORCH_MPS_HIGH_WATERMARK_RATIO", "0.5")
os.environ.setdefault("PYTORCH_MPS_LOW_WATERMARK_RATIO", "0.3")
import torch  # noqa: E402
import torch.nn as nn  # noqa: E402
import torch.nn.functional as F  # noqa: E402

sys.path.insert(0, str(WORK_DIR / "src"))
sys.path.insert(0, str(ROOT / "scripts/context_model"))
for name in ("mlx", "mlx.core", "mlx_lm"):  # probe_rerank imports mlx at module level; not needed here
    sys.modules.setdefault(name, types.ModuleType(name))
sys.modules["mlx_lm"].load = None
from probe_rerank import EVENTS, homophone_set, load_events  # noqa: E402

from model import load  # noqa: E402
from train import pick_device  # noqa: E402

CTX_CHARS = 32


class LoRALinear(nn.Module):
    def __init__(self, base: nn.Linear, rank: int, alpha: float):
        super().__init__()
        self.base = base
        self.a = nn.Parameter(torch.randn(rank, base.in_features) * (1.0 / math.sqrt(base.in_features)))
        self.b = nn.Parameter(torch.zeros(base.out_features, rank))
        self.scale = alpha / rank
        self.enabled = True

    def forward(self, x):
        y = self.base(x)
        if self.enabled:
            y = y + (x @ self.a.t() @ self.b.t()) * self.scale
        return y


def add_lora(model, rank, alpha):
    for p in model.parameters():
        p.requires_grad_(False)
    for blk in model.blocks:
        blk.qkv = LoRALinear(blk.qkv, rank, alpha)
        blk.proj = LoRALinear(blk.proj, rank, alpha)
        blk.mlp[0] = LoRALinear(blk.mlp[0], rank, alpha)
        blk.mlp[2] = LoRALinear(blk.mlp[2], rank, alpha)
    return [p for n, p in model.named_parameters() if n.endswith(".a") or n.endswith(".b")]


def set_lora(model, on):
    for m in model.modules():
        if isinstance(m, LoRALinear):
            m.enabled = on


def day(e):
    return datetime.datetime.fromtimestamp(e["recordedAtMs"] / 1000).strftime("%m-%d")


def clean_ok(e):
    return not (e.get("redoAfterCommit") or e.get("deletedAfterCommit") or e.get("outcome") == "abandoned")


def make_rows(events, vocab):
    rows = []
    for e in events:
        s = homophone_set(e)
        if not s:
            continue
        ctx = (e.get("precedingText") or e["_recon"])[-CTX_CHARS:]
        first_shown = s[0]
        rows.append({"ctx": [vocab.bos] + vocab.encode(ctx), "cands": [vocab.encode(c) for c in s],
                     "gold": s.index(e["selectedText"]), "ok": clean_ok(e), "day": day(e)})
        rows[-1]["nonfirst"] = rows[-1]["gold"] > 0 and s[0] != e["selectedText"]
    return rows


def score_table(model, batch, device, k_max=8):
    seqs, owners, spans, golds = [], [], [], []
    for ctx, cands, gold in batch:
        cands = cands[:k_max]
        if gold >= len(cands):
            continue
        golds.append(gold)
        for c in cands:
            seqs.append(ctx + c)
            owners.append(len(golds) - 1)
            spans.append((len(ctx), len(ctx) + len(c)))
    width = max(len(s) for s in seqs)
    x = torch.tensor([s + [0] * (width - len(s)) for s in seqs], device=device)
    logp = torch.log_softmax(model(x[:, :-1]).float(), dim=-1)
    tok = logp.gather(-1, x[:, 1:, None])[..., 0]
    pos = torch.arange(width - 1, device=device)[None, :]
    start = torch.tensor([s - 1 for s, _ in spans], device=device)[:, None]
    end = torch.tensor([e - 1 for _, e in spans], device=device)[:, None]
    scores = (tok * ((pos >= start) & (pos < end))).sum(1)
    table = torch.full((len(golds), k_max), float("-inf"), device=device)
    counters = [0] * len(golds)
    for i, e in enumerate(owners):
        table[e, counters[e]] = scores[i]
        counters[e] += 1
    return table, torch.tensor(golds, device=device)


@torch.no_grad()
def predict(model, rows, device, lora):
    set_lora(model, lora)
    model.eval()
    out = []
    for i in range(0, len(rows), 64):
        chunk = rows[i:i + 64]
        valid = [r for r in chunk if r["gold"] < 8]
        table, _ = score_table(model, [(r["ctx"], r["cands"], r["gold"]) for r in valid], device)
        out += table.cpu().tolist()
    set_lora(model, True)
    return out


def pick(table_row, beta):
    """The deployed policy: lm score minus beta * position in the IME's own order (beta=0 is the bare model)."""
    return max(range(len(table_row)), key=lambda i: table_row[i] - beta * i)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--init", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--cutoff", default="10-13", help="train on days strictly before, test on days from here")
    ap.add_argument("--rank", type=int, default=8)
    ap.add_argument("--alpha", type=float, default=16)
    ap.add_argument("--steps", type=int, default=300)
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument("--nonfirst-weight", type=float, default=1.0, help="repeat non-first picks this many times")
    ap.add_argument("--keep-dirty", action="store_true", help="also train on events followed by redo/delete")
    ap.add_argument("--lm-weight", type=float, default=0.5)
    ap.add_argument("--corpus", default=ROOT + "/ime_context_model/corpus/v2")
    ap.add_argument("--device", default="auto")
    args = ap.parse_args()

    device = pick_device(args.device)
    model, vocab, ck = load(args.init, device)
    events = load_events(EVENTS)
    rows = make_rows(events, vocab)
    train = [r for r in rows if r["day"] < args.cutoff and (r["ok"] or args.keep_dirty)]
    # 07-xx days and 10-xx days compare as strings; "07-.." < "10-.." < cutoff keeps both eras in training
    test = [r for r in rows if r["day"] >= args.cutoff and r["day"] >= "10-00"]
    train = [r for r in train]
    pool = []
    for r in train:
        pool += [r] * (int(args.nonfirst_weight) if r["nonfirst"] else 1)
    print(f"device={device} train={len(train)} (nonfirst {sum(r['nonfirst'] for r in train)}) pool={len(pool)} "
          f"test={len(test)} (nonfirst {sum(r['nonfirst'] for r in test)})", flush=True)

    params = add_lora(model, args.rank, args.alpha)
    model.to(device)
    print(f"lora params {sum(p.numel() for p in params)/1e6:.2f}M ({sum(p.numel() for p in params)*4/1e6:.1f} MB fp32)", flush=True)

    base_tab = predict(model, test, device, lora=False)

    general = [np.memmap(os.path.join(args.corpus, f"{s}.train.bin"), dtype=np.uint16, mode="r") for s in ("wikipedia", "synth")]
    rng, nrng = random.Random(0), np.random.default_rng(0)
    opt = torch.optim.AdamW(params, lr=args.lr, weight_decay=0.0)
    block = 64
    for step in range(args.steps):
        model.train()
        lr = args.lr * min(1.0, (step + 1) / 30) * (0.1 + 0.9 * 0.5 * (1 + math.cos(math.pi * step / args.steps)))
        for g in opt.param_groups:
            g["lr"] = lr
        batch = rng.sample(pool, 32)
        table, gold = score_table(model, [(r["ctx"], r["cands"], r["gold"]) for r in batch if r["gold"] < 8], device)
        loss = F.cross_entropy(table, gold)
        if args.lm_weight > 0:
            arr = general[step % 2]
            rws = [np.asarray(arr[o:o + block + 1], dtype=np.int64) for o in nrng.integers(0, len(arr) - block - 1, 16)]
            t = torch.from_numpy(np.stack(rws)).to(device)
            loss = loss + args.lm_weight * model(t[:, :-1], t[:, 1:])[1]
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(params, 1.0)
        opt.step()
        if step % 50 == 0:
            print(f"step {step} loss {loss.item():.3f}", flush=True)

    lora_tab = predict(model, test, device, lora=True)
    gold = [r["gold"] for r in test]
    shown_first = [0] * len(test)  # the IME's own order puts index 0 first

    def acc(pred, idx):
        return sum(pred[i] == gold[i] for i in idx) / max(1, len(idx))

    allx = list(range(len(test)))
    nonfirst = [i for i in allx if test[i]["nonfirst"]]
    rest = [i for i in allx if not test[i]["nonfirst"]]
    base_pred = [pick(t, 0) for t in base_tab]
    lora_pred = [pick(t, 0) for t in lora_tab]
    fixed = [i for i in allx if base_pred[i] != gold[i] and lora_pred[i] == gold[i]]
    broken = [i for i in allx if base_pred[i] == gold[i] and lora_pred[i] != gold[i]]
    by_beta = {}
    for beta in (0, 1, 2, 3, 4):
        bp = [pick(t, beta) for t in base_tab]
        lp = [pick(t, beta) for t in lora_tab]
        by_beta[beta] = {"base": round(acc(bp, allx), 4), "lora": round(acc(lp, allx), 4),
                         "fixed": sum(bp[i] != gold[i] and lp[i] == gold[i] for i in allx),
                         "broken": sum(bp[i] == gold[i] and lp[i] != gold[i] for i in allx),
                         "nonfirst_base": round(acc(bp, nonfirst), 4), "nonfirst_lora": round(acc(lp, nonfirst), 4)}
    res = {
        "args": vars(args), "by_beta": by_beta, "train": len(train), "test": len(test), "test_nonfirst": len(nonfirst),
        "ime_order_acc": acc(shown_first, allx), "base_acc": acc(base_pred, allx), "lora_acc": acc(lora_pred, allx),
        "nonfirst_base": acc(base_pred, nonfirst), "nonfirst_lora": acc(lora_pred, nonfirst),
        "rest_base": acc(base_pred, rest), "rest_lora": acc(lora_pred, rest),
        "fixed": len(fixed), "broken": len(broken),
    }
    print(json.dumps(res, ensure_ascii=False, indent=1), flush=True)
    os.makedirs(args.out, exist_ok=True)
    json.dump(res, open(os.path.join(args.out, "result.json"), "w"), ensure_ascii=False, indent=1)
    torch.save({n: p.detach().cpu() for n, p in model.named_parameters() if n.endswith(".a") or n.endswith(".b")},
               os.path.join(args.out, "lora.pt"))


if __name__ == "__main__":
    main()
