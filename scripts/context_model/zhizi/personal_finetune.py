"""Personal adaptation experiment: fine-tune a base 顺手·知字 on the owner's own
early records (choices + committed text) and score it on later, disjoint days.

Runs only on the Mac: the owner's records never leave the machine.
Signals used from the training days:
  - homophone choices: true homophone sets from each event, gold = what the owner committed
  - committed text: the owner's own sentences (commits joined per app session) for the LM loss
A general-corpus LM loss is mixed in so the small personal set does not erase ordinary Chinese.
"""

import argparse
import datetime
import json
import os
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

import numpy as np
import torch

sys.path.insert(0, str(ROOT / 'scripts/context_model'))
from probe_rerank import EVENTS, homophone_set, load_events  # noqa: E402

from model import load, save  # noqa: E402
from train import pick_device  # noqa: E402
from train_homophone import candidate_loss  # noqa: E402


def day(e):
    return datetime.datetime.fromtimestamp(e["recordedAtMs"] / 1000).strftime("%m-%d")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--init", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--until", default="07-22", help="train on days strictly before this")
    ap.add_argument("--corpus", default=str(WORK_DIR / 'corpus/v2'))
    ap.add_argument("--steps", type=int, default=300)
    ap.add_argument("--lr", type=float, default=1e-4)
    ap.add_argument("--owner-lm-weight", type=float, default=0.5)
    ap.add_argument("--general-lm-weight", type=float, default=0.5)
    ap.add_argument("--device", default="auto")
    args = ap.parse_args()

    device = pick_device(args.device)
    model, vocab, ck = load(args.init, device)
    model.train()
    events = [e for e in load_events(EVENTS) if day(e) < args.until]

    choices = []
    for e in events:
        s = homophone_set(e)
        if s:
            ctx = e.get("precedingText", e["_recon"])
            choices.append(([vocab.bos] + vocab.encode(ctx), [vocab.encode(c) for c in s],
                            s.index(e["selectedText"])))
    # owner's own sentences: commits joined per app session (same 120 s rule as the probe)
    sessions, cur, last = [], "", {}
    for e in events:
        prev = last.get(e["appHash"])
        text = e.get("selectedText") or ""
        if prev is not None and e["recordedAtMs"] - prev <= 120_000:
            cur += text
        else:
            if len(cur) >= 4:
                sessions.append(cur)
            cur = text
        last[e["appHash"]] = e["recordedAtMs"]
    if len(cur) >= 4:
        sessions.append(cur)
    owner_ids = []
    for s in sessions:
        owner_ids += [vocab.bos] + vocab.encode(s)
    owner_ids = np.array(owner_ids, dtype=np.int64)
    general = np.memmap(os.path.join(args.corpus, "synth.train.bin"), dtype=np.uint16, mode="r")
    wiki = np.memmap(os.path.join(args.corpus, "wikipedia.train.bin"), dtype=np.uint16, mode="r")
    print(f"device={device} train_days<{args.until} choices={len(choices)} owner_chars={len(owner_ids)}", flush=True)

    rng, nrng = random.Random(0), np.random.default_rng(0)
    block = 64

    def lm_batch(arr, n):
        rows = []
        for _ in range(n):
            o = nrng.integers(0, max(1, len(arr) - block - 1))
            rows.append(np.asarray(arr[o:o + block + 1], dtype=np.int64))
        t = torch.from_numpy(np.stack(rows)).to(device)
        return t[:, :-1], t[:, 1:]

    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, weight_decay=0.0)
    for step in range(args.steps):
        loss, _ = candidate_loss(model, rng.sample(choices, min(64, len(choices))), device, 8)
        x, y = lm_batch(owner_ids, 16)
        loss = loss + args.owner_lm_weight * model(x, y)[1]
        src = general if step % 2 else wiki
        x, y = lm_batch(src, 16)
        loss = loss + args.general_lm_weight * model(x, y)[1]
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        if step % 100 == 0:
            print(f"step {step} loss {loss.item():.3f}", flush=True)
    os.makedirs(args.out, exist_ok=True)
    save(os.path.join(args.out, "best.pt"), model, os.path.join(args.corpus, "vocab.json"), args.steps,
         {"personal_until": args.until})


if __name__ == "__main__":
    main()
