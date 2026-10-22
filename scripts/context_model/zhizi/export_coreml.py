"""Export a 顺手·知字 checkpoint to the Core ML model the Mac input method ships (fp16, [8,40] int32 tokens -> [8,39] log-probs), compile it,
and check it picks the same first candidate as PyTorch on the owner's held-out homophone choices (aggregate counts only).

  python export_coreml.py --ckpt best.pt --out DIR [--rows heldout/S/real_scores.jsonl]

Writes DIR/zhizi30m_v2.mlpackage and DIR/zhizi30m_v2.mlmodelc (the file names the app and the build manifest expect), DIR/itos.json.
The graph is the same StaticScorer the first release used (export_coreml_spike.py): static shapes, explicit -1e4 causal mask,
hand-written numerically stable log-softmax - without these the Neural Engine computes wrong values.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys

import numpy as np
import torch

sys.path.insert(0, os.path.dirname(__file__))
from export_coreml_spike import B, T, Scorer, StaticScorer  # noqa: E402
from model import load  # noqa: E402


def encode_batch(vocab, context, cands):
    """[bos] + context + candidate, left-truncated to T (keeping bos), right-padded with 0; returns tokens and each candidate's span."""
    rows, spans = [], []
    for cand in cands[:B]:
        c = vocab.encode(cand)
        room = T - 1 - len(c)
        ctx = vocab.encode(context)[-room:] if room > 0 else []
        seq = [vocab.bos] + ctx + c
        spans.append((len(seq) - len(c), len(seq)))
        rows.append(seq + [0] * (T - len(seq)))
    while len(rows) < B:
        rows.append([0] * T)
        spans.append((0, 0))
    return np.array(rows, dtype=np.int32), spans


def candidate_scores(logprobs, spans, n):
    # logprobs[i, p] is log P(token p+1 | tokens <= p)
    return [float(sum(logprobs[i, s - 1:e - 1])) for i, (s, e) in enumerate(spans[:n])]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--rows", default="", help="real_scores.jsonl of the held-out choices, for the agreement check")
    ap.add_argument("--limit", type=int, default=1500)
    args = ap.parse_args()
    import coremltools as ct

    os.makedirs(args.out, exist_ok=True)
    lm, vocab, _ = load(args.ckpt, "cpu")
    static = StaticScorer(lm, B, T).eval()
    x = torch.randint(3, len(vocab), (B, T))
    with torch.no_grad():
        traced = torch.jit.trace(static, x)
    mlmodel = ct.convert(traced, inputs=[ct.TensorType(name="tokens", shape=(B, T), dtype=np.int32)],
                         outputs=[ct.TensorType(name="logprobs")],
                         compute_precision=ct.precision.FLOAT16, minimum_deployment_target=ct.target.macOS13)
    package = os.path.join(args.out, "zhizi30m_v2.mlpackage")
    shutil.rmtree(package, ignore_errors=True)
    mlmodel.save(package)
    compiled_dir = os.path.join(args.out, "compiled")
    shutil.rmtree(compiled_dir, ignore_errors=True)
    subprocess.run(["xcrun", "coremlcompiler", "compile", package, compiled_dir], check=True, capture_output=True)
    target = os.path.join(args.out, "zhizi30m_v2.mlmodelc")
    shutil.rmtree(target, ignore_errors=True)
    shutil.move(os.path.join(compiled_dir, "zhizi30m_v2.mlmodelc"), target)
    shutil.rmtree(compiled_dir, ignore_errors=True)
    json.dump(vocab.itos, open(os.path.join(args.out, "itos.json"), "w", encoding="utf-8"), ensure_ascii=False)
    print("wrote", target)

    if args.rows:
        scorer = Scorer(lm).eval()
        model = ct.models.CompiledMLModel(target, compute_units=ct.ComputeUnit.CPU_AND_NE)
        same = total = 0
        worst = 0.0
        for line in open(args.rows, encoding="utf-8"):
            r = json.loads(line)
            if len(r["set"]) < 2:
                continue
            tokens, spans = encode_batch(vocab, r["context"], r["set"])
            with torch.no_grad():
                ref = scorer(torch.from_numpy(tokens).long()).numpy()
            got = list(model.predict({"tokens": tokens}).values())[0]
            n = min(len(r["set"]), B)
            a, b = candidate_scores(ref, spans, n), candidate_scores(got, spans, n)
            worst = max(worst, max(abs(p - q) for p, q in zip(a, b)))
            same += int(np.argmax(a) == np.argmax(b))
            total += 1
            if total >= args.limit:
                break
        print(json.dumps({"agreement_first_pick": f"{same}/{total}", "max_abs_score_diff": round(worst, 3)}))


if __name__ == "__main__":
    main()
