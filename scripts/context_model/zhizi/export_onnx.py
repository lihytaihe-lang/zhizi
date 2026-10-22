"""Export 顺手·知字 to ONNX for the Windows host (ONNX Runtime), and check it picks the
same first candidate as the PyTorch reference.

tokens Int32[rows, length] -> logprobs Float32[rows, length - 1] (length up to 40), the
log-probability of each next token. Writes the float32 model the Windows host ships and
reports how often it picks the same first candidate as the PyTorch model (window capped
at 40, as on macOS) on the public synthetic cases and, when present, the owner's local
held-out records (aggregate counts only). An int8 variant agreed on 3,049 of 3,086 owner
cases on 2026-10-10 and is not produced.

Usage: python export_onnx.py --ckpt best.pt --itos itos.json --out DIR
"""
import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0, os.path.dirname(__file__))
from model import load  # noqa: E402

B, T = 8, 40


class Scorer(torch.nn.Module):
    """Per-position log-probability of the next token for any batch and length up to 40
    (the host sends only the rows and length it needs)."""

    def __init__(self, lm):
        super().__init__()
        self.lm = lm

    def forward(self, tokens):
        x = tokens.long()
        lm = self.lm
        h = lm.tok(x) + lm.pos.weight[: x.shape[1]].unsqueeze(0)
        for blk in lm.blocks:
            h = blk(h)
        logits = lm.head(lm.ln_f(h))[:, :-1]
        logp = torch.log_softmax(logits, dim=-1)
        return torch.gather(logp, 2, x[:, 1:].unsqueeze(-1)).squeeze(-1)


def batch(vocab, context, cands):
    """The Windows host's batching (native/host/src/ranking.rs): bos + context tail +
    candidate, one row per candidate, as long as the longest row, padded with 0."""
    room = T - 1 - max(len(c) for c in cands)
    ctx = [vocab.bos] + vocab.encode(context[-room:] if room > 0 else "")
    seqs = [(ctx + vocab.encode(c))[:T] for c in cands]
    width = max(len(s) for s in seqs)
    x = np.zeros((len(seqs), width), dtype=np.int32)
    for i, s in enumerate(seqs):
        x[i, : len(s)] = s
    return x, ctx, seqs


def sums(logprobs, ctx, seqs):
    return [float(logprobs[i, len(ctx) - 1 : len(s) - 1].sum()) for i, s in enumerate(seqs)]


def cases():
    sys.path.insert(0, os.path.dirname(__file__))
    from check_host_alignment import heldout_cases, synthetic_cases  # noqa: E402

    synthetic = [(c["context"], c["candidates"]) for c in synthetic_cases()]
    owner = heldout_cases() or []
    owner = [(c["context"], c["candidates"]) for c in owner if 2 <= len(c["candidates"]) <= B]
    return synthetic, owner


def agreement(run, reference, vocab, items):
    agree = 0
    for context, cands in items:
        x, ctx, seqs = batch(vocab, context, cands)
        mine = sums(run(x), ctx, seqs)
        ref = sums(reference(x), ctx, seqs)
        agree += int(np.argmax(mine) == np.argmax(ref))
    return agree, len(items)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    lm, vocab, _ = load(args.ckpt, "cpu")
    lm.eval()
    scorer = Scorer(lm).eval()
    model = out / "zhizi30m_v2.onnx"
    example = torch.zeros((4, 24), dtype=torch.int32)
    torch.onnx.export(scorer, (example,), str(model), input_names=["tokens"], output_names=["logprobs"],
                      opset_version=17, dynamo=False,
                      dynamic_axes={"tokens": {0: "rows", 1: "length"}, "logprobs": {0: "rows", 1: "positions"}})
    import onnxruntime as ort

    session = ort.InferenceSession(str(model), providers=["CPUExecutionProvider"])

    def run(x):
        return session.run(None, {"tokens": x})[0]

    def torch_run(x):
        with torch.no_grad():
            return scorer(torch.from_numpy(x)).numpy()

    synthetic, owner = cases()
    x, _, _ = batch(vocab, "这是一个公开的合成用例", ["在", "再", "载"])
    report = {
        "file": model.name,
        "bytes": model.stat().st_size,
        "sha256": hashlib.sha256(model.read_bytes()).hexdigest(),
        "max_abs_diff": float(np.abs(run(x) - torch_run(x)).max()),
        "synthetic": "%d/%d" % agreement(run, torch_run, vocab, synthetic),
        "owner": "%d/%d" % agreement(run, torch_run, vocab, owner) if owner else "absent",
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
