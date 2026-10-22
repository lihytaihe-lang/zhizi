"""Score a 顺手·知字 checkpoint on the owner's real records with the exact
protocol of the step-0 probe (scripts/context_model/probe_rerank.py), and dump
real_scores.jsonl so analyze_policies.py compares it line for line with Qwen."""

import argparse
import json
import os
import sys

import torch

sys.path.insert(0, os.path.dirname(os.path.dirname(__file__)))

from model import load  # noqa: E402


class ZhiziScorer:
    def __init__(self, path, device):
        self.model, self.vocab, _ = load(path, device)
        self.device = device

    @torch.no_grad()
    def score(self, context, cands):
        room = self.model.config.block_size - 1 - max(len(c) for c in cands)
        ctx = [self.vocab.bos] + self.vocab.encode(context[-room:] if room > 0 else "")
        seqs = [ctx + self.vocab.encode(c) for c in cands]
        width = max(len(s) for s in seqs)
        x = torch.tensor([s + [0] * (width - len(s)) for s in seqs], device=self.device)
        logp = torch.log_softmax(self.model(x[:, :-1]).float(), dim=-1)
        picked = logp.gather(-1, x[:, 1:, None])[..., 0]
        out = []
        for i, s in enumerate(seqs):
            out.append(picked[i, len(ctx) - 1:len(s) - 1].sum().item())
        return out


def main():
    from probe_rerank import EVENTS, homophone_set, load_events
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--since", default="", help="only score events on/after this MM-DD (held-out days)")
    ap.add_argument("--device", default="mps" if torch.backends.mps.is_available() else "cpu")
    args = ap.parse_args()
    scorer = ZhiziScorer(args.ckpt, args.device)
    rows, fixes, breaks, wrong = [], 0, 0, 0
    import datetime
    for e in load_events(EVENTS):
        if args.since and datetime.datetime.fromtimestamp(e["recordedAtMs"] / 1000).strftime("%m-%d") < args.since:
            continue
        s = homophone_set(e)
        if not s or "precedingText" not in e:
            continue
        sc = scorer.score(e["precedingText"], s)
        best = max(range(len(s)), key=lambda i: sc[i])
        gold = e["selectedText"]
        wrong += s[0] != gold
        fixes += s[0] != gold and s[best] == gold
        breaks += s[0] == gold and s[best] != gold
        rows.append({"context": e["precedingText"], "pinyin": e["pinyin"], "set": s,
                     "gold": gold, "scores": [round(v, 3) for v in sc]})
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "real_scores.jsonl"), "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(json.dumps({"ckpt": args.ckpt, "cases": len(rows), "ime_wrong": wrong,
                      "fixes": fixes, "breaks": breaks}, ensure_ascii=False))


if __name__ == "__main__":
    main()
