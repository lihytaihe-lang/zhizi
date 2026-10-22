"""Inference-runtime spike for the IME: how long does one rerank decision take?

One decision = score up to 8 candidates after a 32-char context: a [8, 40] token
batch through the 30M model, then gather the candidate log-probs. Measures
PyTorch CPU (fp32) and Core ML (fp16) on CPU only / CPU+GPU / CPU+Neural Engine,
plus model size on disk. The IME's key handling budget is a few milliseconds.
"""

import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

import numpy as np
import torch

sys.path.insert(0, os.path.dirname(__file__))
from model import load  # noqa: E402

CKPT = str(WORK_DIR / 'runs/gpu-v2-30m/runs/v2-30m-homo/best.pt')
OUT = str(WORK_DIR / 'spike')
B, T = 8, 40


class StaticScorer(torch.nn.Module):
    """Same math as model.ZhiziLM with every shape a Python constant and an explicit causal
    mask, so the Core ML converter sees no shape arithmetic."""

    def __init__(self, lm, b, t):
        super().__init__()
        self.lm, self.b, self.t = lm, b, t
        c = lm.config
        self.h, self.d = c.n_head, c.d_model
        self.register_buffer("mask", torch.full((t, t), -1e4).triu(1))
        self.register_buffer("pos", lm.pos.weight[:t].detach().clone())

    def forward(self, x):
        b, t, h, d = self.b, self.t, self.h, self.d
        lm = self.lm
        z = lm.tok(x) + self.pos
        for blk in lm.blocks:
            q, k, v = blk.qkv(blk.ln1(z)).split(d, dim=2)
            q = q.reshape(b, t, h, d // h).transpose(1, 2)
            k = k.reshape(b, t, h, d // h).transpose(1, 2)
            v = v.reshape(b, t, h, d // h).transpose(1, 2)
            att = torch.softmax(q @ k.transpose(-1, -2) * (1.0 / (d // h) ** 0.5) + self.mask, dim=-1)
            z = z + blk.proj((att @ v).transpose(1, 2).reshape(b, t, d))
            z = z + blk.mlp(blk.ln2(z))
        logits = lm.head(lm.ln_f(z))[:, :-1]
        # fp16-safe log-softmax: never take log of an underflowed probability
        m = logits.max(dim=-1, keepdim=True).values
        lse = m + torch.log(torch.exp(logits - m).sum(dim=-1, keepdim=True))
        picked = torch.gather(logits, 2, x[:, 1:].unsqueeze(-1))
        return (picked - lse).squeeze(-1)


class Scorer(torch.nn.Module):
    """Returns per-position log-prob of the next token, the only thing the IME needs."""

    def __init__(self, lm):
        super().__init__()
        self.lm = lm

    def forward(self, x):
        lm = self.lm
        h = lm.tok(x) + lm.pos.weight[: x.shape[1]].unsqueeze(0)  # trace-friendly positions
        for blk in lm.blocks:
            h = blk(h)
        logits = lm.head(lm.ln_f(h))
        logp = torch.log_softmax(logits[:, :-1].float(), dim=-1)
        return torch.gather(logp, 2, x[:, 1:].unsqueeze(-1)).squeeze(-1)


def bench(fn, n=200):
    for _ in range(10):
        fn()
    ts = []
    for _ in range(n):
        t = time.perf_counter()
        fn()
        ts.append((time.perf_counter() - t) * 1000)
    ts.sort()
    return f"p50={ts[n // 2]:.2f}ms p95={ts[int(n * 0.95)]:.2f}ms"


def main():
    os.makedirs(OUT, exist_ok=True)
    lm, vocab, _ = load(CKPT, "cpu")
    scorer = Scorer(lm).eval()
    x = torch.randint(3, len(vocab), (B, T))
    torch.set_num_threads(4)
    with torch.no_grad():
        print("torch cpu fp32 4 threads:", bench(lambda: scorer(x)))
    x1 = torch.randint(3, len(vocab), (1, T))
    with torch.no_grad():
        print("torch cpu fp32 4 threads, ONE sequence (prefix-shared lower bound):", bench(lambda: scorer(x1)))
    torch.set_num_threads(1)
    with torch.no_grad():
        print("torch cpu fp32 1 thread: ", bench(lambda: scorer(x), 100))

    import coremltools as ct
    static = StaticScorer(lm, B, T).eval()
    with torch.no_grad():
        print("static vs dynamic max|diff|:", float((static(x) - scorer(x)).abs().max()))
    traced = torch.jit.trace(static, x)
    mlmodel = ct.convert(traced, inputs=[ct.TensorType(name="tokens", shape=(B, T), dtype=np.int32)],
                         outputs=[ct.TensorType(name="logprobs")],
                         compute_precision=ct.precision.FLOAT16, minimum_deployment_target=ct.target.macOS13)
    path = os.path.join(OUT, "zhizi30m_fp16.mlpackage")
    mlmodel.save(path)
    size = sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(path) for f in fs)
    print(f"coreml fp16 package size: {size / 1e6:.1f} MB")
    feed = {"tokens": x.numpy().astype(np.int32)}
    ref = scorer(x).detach().numpy()
    for name, unit in [("CPU_ONLY", ct.ComputeUnit.CPU_ONLY), ("CPU_AND_GPU", ct.ComputeUnit.CPU_AND_GPU),
                       ("CPU_AND_NE", ct.ComputeUnit.CPU_AND_NE), ("ALL", ct.ComputeUnit.ALL)]:
        m = ct.models.MLModel(path, compute_units=unit)
        out = list(m.predict(feed).values())[0]
        err = float(np.abs(out - ref).max())
        print(f"coreml {name:<12} {bench(lambda: m.predict(feed))}  max|diff| vs torch={err:.3f}")


if __name__ == "__main__":
    main()
