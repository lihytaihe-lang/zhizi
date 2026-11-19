"""Export the frozen general 知句 weights to FP32 ONNX, with per-search beam caches."""
import argparse
import hashlib
import json
from pathlib import Path
import sys

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "zhizi"))
from model import load
from export_dist import FullDistribution, IncrementalDistribution, log_distribution

CHECKPOINT_SHA = "1e1a1485003391694a0fd44145d83e219c72e9258b0ae7c2fb75e25f89f0f261"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Prefill(FullDistribution):
    def forward(self, tokens):
        b, t = tokens.shape
        h, d, lm = self.h, self.d, self.lm
        z = lm.tok(tokens.long()) + lm.pos.weight[:t]
        keys, values = [], []
        for blk in lm.blocks:
            q, k, v = blk.qkv(blk.ln1(z)).split(d, dim=2)
            q, k, v = (x.reshape(b, t, h, d // h).transpose(1, 2) for x in (q, k, v))
            keys.append(k)
            values.append(v)
            att = torch.softmax(q @ k.transpose(-1, -2) * (1.0 / (d // h) ** 0.5)
                                + self.mask[:t, :t], dim=-1)
            z = z + blk.proj((att @ v).transpose(1, 2).reshape(b, t, d))
            z = z + blk.mlp(blk.ln2(z))
        return log_distribution(lm.head(lm.ln_f(z[:, -1]))), torch.stack(keys), torch.stack(values)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if sha(args.checkpoint) != CHECKPOINT_SHA:
        raise ValueError("checkpoint differs from the frozen general weights")
    if args.out.exists() and any(args.out.iterdir()):
        raise ValueError("refusing to overwrite an existing export")
    args.out.mkdir(parents=True, exist_ok=True)
    torch.set_num_threads(2)
    lm, vocab, _ = load(args.checkpoint)
    c = lm.config
    if (c.n_layer, c.n_head, c.d_model) != (8, 8, 512):
        raise ValueError("unsupported cache dimensions")
    prefill, step = Prefill(lm).eval(), IncrementalDistribution(lm).eval()
    x = torch.ones((8, 16), dtype=torch.int32)
    cache = torch.zeros((8, 8, 8, 16, 64))
    full_path, step_path = args.out / "zhiju30m_v3.onnx", args.out / "zhiju30m_v3_step.onnx"
    with torch.no_grad():
        torch.onnx.export(prefill, (x,), str(full_path), input_names=["tokens"],
            output_names=["logprobs", "new_keys", "new_values"], opset_version=17, dynamo=False,
            dynamic_axes={"tokens": {0: "rows", 1: "length"}, "logprobs": {0: "rows"},
                          "new_keys": {1: "rows", 3: "length"}, "new_values": {1: "rows", 3: "length"}})
        torch.onnx.export(step, (x[:, :1], cache, cache), str(step_path), input_names=["tokens", "keys", "values"],
            output_names=["logprobs", "new_keys", "new_values"], opset_version=17, dynamo=False,
            dynamic_axes={"tokens": {0: "rows"}, "logprobs": {0: "rows"},
                          "keys": {1: "rows", 3: "past"}, "values": {1: "rows", 3: "past"},
                          "new_keys": {1: "rows", 3: "present"}, "new_values": {1: "rows", 3: "present"}})
    (args.out / "itos.json").write_text(json.dumps(vocab.itos, ensure_ascii=False), encoding="utf-8")
    descriptor = {"format": "zhiju-onnx-cache/1", "checkpointSHA256": CHECKPOINT_SHA,
        "capacity": 40, "layers": 8, "heads": 8, "headDimension": 64,
        "prefillModel": full_path.name, "stepModel": step_path.name, "precision": "float32"}
    (args.out / "model.json").write_text(json.dumps(descriptor, indent=2) + "\n")

    import onnxruntime as ort
    options = ort.SessionOptions()
    options.intra_op_num_threads = 2
    options.inter_op_num_threads = 1
    full = ort.InferenceSession(str(full_path), options, providers=["CPUExecutionProvider"])
    incremental = ort.InferenceSession(str(step_path), options, providers=["CPUExecutionProvider"])
    report = {"checkpointSHA256": CHECKPOINT_SHA, "sourceSHA256": sha(Path(__file__)),
        "mathSourceSHA256": sha(Path(__file__).with_name("export_dist.py")), "newTraining": False,
        "quantized": False, "seed": 20261117, "rows": 0, "maxAbsoluteError": 0.0,
        "maxTotalVariation": 0.0, "top1Agreement": 0, "passed": True,
        "criteria": {"maxAbsoluteError": 0.001, "maxTotalVariation": 0.0001, "top1Agreement": "all rows"}}
    rng = np.random.default_rng(report["seed"])

    def verify(prefix, output):
        actual, keys, values = output
        rows, length = prefix.shape
        assert actual.shape == (rows, len(vocab)) and keys.shape == values.shape == (8, rows, 8, length, 64)
        assert all(np.isfinite(x).all() for x in output)
        with torch.no_grad():
            reference = torch.log_softmax(lm(torch.from_numpy(prefix).long())[:, -1], -1).numpy()
        absolute = float(np.abs(actual-reference).max())
        variation = float((np.abs(np.exp(actual)-np.exp(reference)).sum(-1)/2).max())
        agree = int((actual.argmax(-1) == reference.argmax(-1)).sum())
        report["rows"] += rows
        report["top1Agreement"] += agree
        report["maxAbsoluteError"] = max(report["maxAbsoluteError"], absolute)
        report["maxTotalVariation"] = max(report["maxTotalVariation"], variation)
        report["passed"] &= absolute <= 0.001 and variation <= 0.0001 and agree == rows

    for length in [1, 8, 16, 39]:
        prefix = rng.integers(3, len(vocab), (8, length), dtype=np.int32)
        prefix[:, 0] = 1
        verify(prefix, full.run(None, {"tokens": prefix}))
    for start in [1, 8, 20, 32]:
        prefix = rng.integers(3, len(vocab), (8, start), dtype=np.int32)
        prefix[:, 0] = 1
        output = full.run(None, {"tokens": prefix})
        for _ in range(4):
            parents = rng.integers(0, 8, 8)
            next_tokens = rng.integers(3, len(vocab), (8, 1), dtype=np.int32)
            prefix = np.concatenate((prefix[parents], next_tokens), axis=1)
            output = incremental.run(None, {"tokens": next_tokens,
                "keys": np.ascontiguousarray(output[1][:, parents]), "values": np.ascontiguousarray(output[2][:, parents])})
            verify(prefix, output)
    (args.out / "PARITY.json").write_text(json.dumps(report, indent=2) + "\n")
    files = sorted(p for p in args.out.iterdir() if p.is_file() and p.name != "SHA256SUMS")
    (args.out / "SHA256SUMS").write_text("".join(f"{sha(p)}  {p.name}\n" for p in files))
    print(json.dumps(report), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
