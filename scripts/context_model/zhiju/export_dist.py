"""Export 知字 v3 next-character distributions without changing the product.

ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dist.py

A: tokens[B,T] -> logprobs[B,V]. B: tokens[B,1], keys/values[L,B,H,S,D/H]
-> logprobs[B,V], new_keys/new_values[L,B,H,S+1,D/H]. Position is S, so
these caches can only be reused while the prefix and its positions stay intact.

Right-padded 8x40 graph. position is one-hot; a matmul picks that step.
Artifacts stay in tmp/zhiju-position/ and are not part of the product package.

ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dist.py --position export
ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dist.py --position verify
ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dist.py --position bench
ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dist.py --position plan
"""
import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

import numpy as np
import torch

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/context_model/zhizi"))
from model import load


def log_distribution(logits):
    # Same fp16-safe normalization as export_coreml.py's StaticScorer.
    maximum = logits.max(dim=-1, keepdim=True).values
    lse = maximum + torch.log(torch.exp(logits - maximum).sum(dim=-1, keepdim=True))
    return logits - lse


class FullDistribution(torch.nn.Module):
    def __init__(self, lm, shape=None):
        super().__init__()
        self.lm = lm
        self.h = lm.config.n_head
        self.d = lm.config.d_model
        self.shape = shape
        self.register_buffer("mask", torch.full((lm.config.block_size, lm.config.block_size), -1e4).triu(1))

    def forward(self, tokens):
        b, t = tokens.shape if self.shape is None else self.shape
        h, d, lm = self.h, self.d, self.lm
        z = lm.tok(tokens.long()) + lm.pos.weight[:t]
        for blk in lm.blocks:
            q, k, v = blk.qkv(blk.ln1(z)).split(d, dim=2)
            q = q.reshape(b, t, h, d // h).transpose(1, 2)
            k = k.reshape(b, t, h, d // h).transpose(1, 2)
            v = v.reshape(b, t, h, d // h).transpose(1, 2)
            att = torch.softmax(q @ k.transpose(-1, -2) * (1.0 / (d // h) ** 0.5)
                                + self.mask[:t, :t], dim=-1)
            z = z + blk.proj((att @ v).transpose(1, 2).reshape(b, t, d))
            z = z + blk.mlp(blk.ln2(z))
        # The vocabulary projection is needed for just this position.
        return log_distribution(lm.head(lm.ln_f(z[:, -1])))


class IncrementalDistribution(torch.nn.Module):
    def __init__(self, lm, shape=None):
        super().__init__()
        self.lm = lm
        self.h = lm.config.n_head
        self.d = lm.config.d_model
        self.shape = shape

    def forward(self, tokens, keys, values):
        b, pos = (tokens.shape[0], keys.shape[3]) if self.shape is None else self.shape
        h, d, lm = self.h, self.d, self.lm
        z = lm.tok(tokens.long()) + lm.pos.weight[pos:pos + 1]
        new_keys, new_values = [], []
        for layer, blk in enumerate(lm.blocks):
            q, k, v = blk.qkv(blk.ln1(z)).split(d, dim=2)
            q = q.reshape(b, 1, h, d // h).transpose(1, 2)
            k = k.reshape(b, 1, h, d // h).transpose(1, 2)
            v = v.reshape(b, 1, h, d // h).transpose(1, 2)
            k = torch.cat((keys[layer], k), dim=2)
            v = torch.cat((values[layer], v), dim=2)
            new_keys.append(k)
            new_values.append(v)
            # A single query is at the end: every cached position is causal.
            att = torch.softmax(q @ k.transpose(-1, -2) * (1.0 / (d // h) ** 0.5), dim=-1)
            z = z + blk.proj((att @ v).transpose(1, 2).reshape(b, 1, d))
            z = z + blk.mlp(blk.ln2(z))
        return (log_distribution(lm.head(lm.ln_f(z[:, 0]))),
                torch.stack(new_keys), torch.stack(new_values))


class PositionDistribution(torch.nn.Module):
    """Fixed or dynamic tokens[B,T] with a one-hot position[B,T].

    select:
      before_ln — matmul on the block output, then ln_f and the vocab head
      after_ln — ln_f on every step, then the same matmul
      mul_before_ln / mul_after_ln — one-hot multiply-sum instead of matmul
    The default is the matmul before ln_f. Later steps cannot affect the marked
    step: the causal mask is the same -1e4 triangle FullDistribution uses.
    """

    def __init__(self, lm, shape=None, select="before_ln"):
        super().__init__()
        if select not in ("before_ln", "after_ln", "mul_before_ln", "mul_after_ln"):
            raise ValueError(select)
        self.lm = lm
        self.h = lm.config.n_head
        self.d = lm.config.d_model
        self.shape = shape
        self.select = select
        self.register_buffer("mask", torch.full((lm.config.block_size, lm.config.block_size), -1e4).triu(1))

    def _selected(self, position, hidden):
        weight = position.to(dtype=hidden.dtype)
        if self.select.startswith("mul"):
            return (weight.unsqueeze(-1) * hidden).sum(dim=1)
        # position[:, None, :] @ hidden is [B, 1, D]. Not a gather: Core ML can keep the matmul.
        return (weight[:, None, :] @ hidden).squeeze(1)

    def forward(self, tokens, position):
        b, t = tokens.shape if self.shape is None else self.shape
        h, d, lm = self.h, self.d, self.lm
        z = lm.tok(tokens.long()) + lm.pos.weight[:t]
        for blk in lm.blocks:
            q, k, v = blk.qkv(blk.ln1(z)).split(d, dim=2)
            q = q.reshape(b, t, h, d // h).transpose(1, 2)
            k = k.reshape(b, t, h, d // h).transpose(1, 2)
            v = v.reshape(b, t, h, d // h).transpose(1, 2)
            att = torch.softmax(q @ k.transpose(-1, -2) * (1.0 / (d // h) ** 0.5)
                                + self.mask[:t, :t], dim=-1)
            z = z + blk.proj((att @ v).transpose(1, 2).reshape(b, t, d))
            z = z + blk.mlp(blk.ln2(z))
        if self.select.endswith("after_ln"):
            selected = self._selected(position, lm.ln_f(z))
        else:
            selected = lm.ln_f(self._selected(position, z))
        return log_distribution(lm.head(selected))


def examples(lm):
    c = lm.config
    tokens = torch.ones((2, 16), dtype=torch.int32)
    cache = torch.zeros((c.n_layer, 2, c.n_head, 8, c.d_model // c.n_head))
    return tokens, cache


def export_onnx(lm, out):
    out.mkdir(parents=True, exist_ok=True)
    tokens, cache = examples(lm)
    with torch.no_grad():
        torch.onnx.export(FullDistribution(lm).eval(), (tokens,), str(out / "full.onnx"),
                          input_names=["tokens"], output_names=["logprobs"], opset_version=17,
                          dynamo=False, dynamic_axes={"tokens": {0: "batch", 1: "length"},
                                                     "logprobs": {0: "batch"}})
        torch.onnx.export(IncrementalDistribution(lm).eval(), (tokens[:, :1], cache, cache),
                          str(out / "incremental.onnx"), input_names=["tokens", "keys", "values"],
                          output_names=["logprobs", "new_keys", "new_values"], opset_version=17,
                          dynamo=False, dynamic_axes={
                              "tokens": {0: "batch"}, "logprobs": {0: "batch"},
                              "keys": {1: "batch", 3: "past"}, "values": {1: "batch", 3: "past"},
                              "new_keys": {1: "batch", 3: "present"},
                              "new_values": {1: "batch", 3: "present"}})


def export_coreml(lm, out, incremental, shape=None):
    import coremltools as ct

    tokens, cache = examples(lm)
    batch = shape[0] if shape else ct.RangeDim(lower_bound=1, upper_bound=16, default=2)
    if shape:
        tokens = torch.ones((shape[0], 1 if incremental else shape[1]), dtype=torch.int32)
        c = lm.config
        cache = torch.zeros((c.n_layer, shape[0], c.n_head, shape[1], c.d_model // c.n_head))
    if incremental:
        c = lm.config
        past = shape[1] if shape else ct.RangeDim(lower_bound=0, upper_bound=39, default=8)
        cache_shape = (c.n_layer, batch, c.n_head, past, c.d_model // c.n_head)
        inputs = [ct.TensorType(name="tokens", shape=(batch, 1), dtype=np.int32),
                  ct.TensorType(name="keys", shape=cache_shape, dtype=np.float16),
                  ct.TensorType(name="values", shape=cache_shape, dtype=np.float16)]
        outputs = [ct.TensorType(name="logprobs", dtype=np.float32),
                   ct.TensorType(name="new_keys", dtype=np.float16),
                   ct.TensorType(name="new_values", dtype=np.float16)]
        module, example, name = IncrementalDistribution(lm, shape).eval(), (tokens[:, :1], cache, cache), "incremental"
    else:
        length = shape[1] if shape else ct.RangeDim(lower_bound=1, upper_bound=40, default=16)
        inputs = [ct.TensorType(name="tokens", shape=(batch, length), dtype=np.int32)]
        outputs = [ct.TensorType(name="logprobs", dtype=np.float32)]
        module, example, name = FullDistribution(lm, shape).eval(), (tokens,), "full"
    if shape:
        name += f"_b{shape[0]}_{'s' if incremental else 't'}{shape[1]}"
    with torch.no_grad():
        traced = torch.jit.trace(module, example)
    model = ct.convert(traced, inputs=inputs, outputs=outputs, convert_to="mlprogram",
                       compute_precision=ct.precision.FLOAT16,
                       minimum_deployment_target=ct.target.macOS13, skip_model_load=True)
    model.save(str(out / f"{name}.mlpackage"))
    # Never benchmark a compiled artifact left over from an earlier export.
    shutil.rmtree(out / "compiled" / f"{name}.mlmodelc", ignore_errors=True)
    (out / "compiled" / f"{name}-compile.json").unlink(missing_ok=True)


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


B, T = 8, 40
SEED = 20261111
ONNX_A_SHA256 = "369ab0950dfbb64dddf5d7eeab543460bb3aaa49c668bed8fd7fd5e360d7c43f"
LOAD_LIMIT = 8.0
SELECTS = ("before_ln", "after_ln", "mul_before_ln", "mul_after_ln")


def position_name(select):
    return "position_b8_t40" if select == "before_ln" else f"position_b8_t40_{select}"


def write_json(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")


def directory_bytes(path):
    return sum(item.stat().st_size for item in Path(path).rglob("*") if item.is_file())


def export_position_onnx(lm, out, select="before_ln"):
    out.mkdir(parents=True, exist_ok=True)
    tokens = torch.ones((B, T), dtype=torch.int32)
    position = torch.zeros((B, T), dtype=torch.float16)
    position[:, 0] = 1
    path = out / f"{position_name(select)}.onnx"
    with torch.no_grad():
        torch.onnx.export(PositionDistribution(lm, shape=(B, T), select=select).eval(),
                          (tokens, position), str(path), input_names=["tokens", "position"],
                          output_names=["logprobs"], opset_version=17, dynamo=False)
    return path


def export_position_coreml(lm, out, select="before_ln"):
    import coremltools as ct

    tokens = torch.ones((B, T), dtype=torch.int32)
    position = torch.zeros((B, T), dtype=torch.float16)
    position[:, 0] = 1
    module = PositionDistribution(lm, shape=(B, T), select=select).eval()
    with torch.no_grad():
        traced = torch.jit.trace(module, (tokens, position))
    model = ct.convert(traced, convert_to="mlprogram", compute_precision=ct.precision.FLOAT16,
                       minimum_deployment_target=ct.target.macOS13,
                       inputs=[ct.TensorType(name="tokens", shape=(B, T), dtype=np.int32),
                               ct.TensorType(name="position", shape=(B, T), dtype=np.float16)],
                       outputs=[ct.TensorType(name="logprobs", dtype=np.float32)])
    package = out / f"{position_name(select)}.mlpackage"
    shutil.rmtree(package, ignore_errors=True)
    model.save(str(package))
    return package


def compile_mlpackage(package, out):
    package = Path(package)
    produced = out / f"{package.stem}.mlmodelc"
    shutil.rmtree(produced, ignore_errors=True)
    result = subprocess.run(["xcrun", "coremlcompiler", "compile", str(package), str(out)],
                            capture_output=True, text=True)
    receipt = {"returncode": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
    write_json(out / f"{package.stem}-compile.json", receipt)
    if result.returncode != 0 or not produced.is_dir():
        raise SystemExit(f"coremlcompiler failed ({result.returncode}); see {package.stem}-compile.json")
    return produced


def position_spot_check(lm, select):
    generator = torch.Generator().manual_seed(0)
    lengths = (1, 40, 5, 17, 8, 33, 2, 21)
    tokens = torch.zeros((B, T), dtype=torch.int32)
    position = torch.zeros((B, T), dtype=torch.float16)
    expected = []
    vocab = lm.config.vocab_size
    with torch.no_grad():
        for row, length in enumerate(lengths):
            seq = torch.randint(1, vocab, (length,), generator=generator)
            tokens[row, :length] = seq
            if length < T:
                tokens[row, length:] = torch.randint(1, vocab, (T - length,), generator=generator)
            position[row, length - 1] = 1
            expected.append(torch.log_softmax(lm(seq.view(1, -1))[:, -1], -1))
        got = PositionDistribution(lm, shape=(B, T), select=select)(tokens, position)
        error = (got - torch.cat(expected, 0)).abs()
    return float(error.max()), float(error.median())


def export_position_cmd(args):
    import coremltools as ct

    lm, vocab, _ = load(args.ckpt, "cpu")
    name = position_name(args.select)
    (args.out / "itos.json").write_text(json.dumps(vocab.itos, ensure_ascii=False), encoding="utf-8")
    manifest_path = args.out / "export.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
    manifest.update(checkpoint=str(args.ckpt), checkpoint_sha256=sha256(args.ckpt),
                    config=vars(lm.config), vocab=len(vocab), torch=torch.__version__,
                    coremltools=ct.__version__, numpy=np.__version__)
    print("spot check", flush=True)
    spot_max, spot_median = position_spot_check(lm, args.select)
    print(json.dumps({"spot_max": spot_max, "spot_median": spot_median}), flush=True)
    if spot_max > 1e-3:
        raise SystemExit(f"position spot check max abs {spot_max}")
    graph = {"select": args.select, "spot_max_abs": spot_max, "spot_median_abs": spot_median,
             "inputs": {"tokens": "Int32[8,40]", "position": "Float16[8,40]"},
             "outputs": {"logprobs": "Float32[8,V]"},
             "compute_precision": "FLOAT16", "minimum_deployment_target": "macOS13"}
    if args.format in ("all", "onnx"):
        print("exporting onnx", flush=True)
        path = export_position_onnx(lm, args.out, args.select)
        graph["onnx"] = {"path": path.name, "bytes": path.stat().st_size, "sha256": sha256(path)}
    if args.format in ("all", "coreml"):
        print("exporting coreml", flush=True)
        package = export_position_coreml(lm, args.out, args.select)
        print("compiling", flush=True)
        compiled = compile_mlpackage(package, args.out)
        graph["coreml"] = {"mlpackage_bytes": directory_bytes(package),
                           "mlmodelc_bytes": directory_bytes(compiled)}
        blobs = [item for item in compiled.rglob("*") if item.is_file() and item.suffix == ".bin"]
        if blobs:
            blob = max(blobs, key=lambda item: item.stat().st_size)
            graph["coreml"]["weights_file"] = str(blob.relative_to(compiled))
            graph["coreml"]["weights_sha256"] = sha256(blob)
        metadata = compiled / "metadata.json"
        if metadata.is_file():
            schema = json.loads(metadata.read_text(encoding="utf-8"))
            graph["coreml"]["input_schema"] = schema[0].get("inputSchema")
            graph["coreml"]["output_schema"] = schema[0].get("outputSchema")
            graph["coreml"]["operation_histogram"] = schema[0].get("mlProgramOperationTypeHistogram")
    manifest.setdefault("graphs", {})[name] = graph
    write_json(manifest_path, manifest)
    print(json.dumps({"name": name, "onnx_bytes": graph.get("onnx", {}).get("bytes"),
                      "mlmodelc_bytes": graph.get("coreml", {}).get("mlmodelc_bytes")}), flush=True)


def find_onnx_a():
    candidates = [ROOT.parent / "zhiju-export-dist/tmp/zhiju-export/full.onnx",
                  ROOT.parents[1] / "zhiju-export/full.onnx"]
    found = [(sha256(path), path) for path in candidates if path.is_file()]
    matched = [item for item in found if item[0] == ONNX_A_SHA256]
    if matched:
        return matched[0][1], matched[0][0]
    if found:
        return found[0][1], found[0][0]
    raise FileNotFoundError("dynamic ONNX A full.onnx was not found")


def diff_stats(error):
    error = np.asarray(error, dtype=np.float64).reshape(-1)
    return {"max": float(np.max(error)), "median": float(np.median(error)), "n": int(error.size)}


def make_sequences(vocab_size, n=1600):
    rng = np.random.default_rng(SEED)
    sequences, lengths = [], []
    for _ in range(n):
        length = int(rng.integers(2, T + 1))
        seq = rng.integers(1, vocab_size, size=length, dtype=np.int32)
        seq[0] = 1
        sequences.append(seq)
        lengths.append(length)
    tokens = np.zeros((n, T), np.int32)
    position = np.zeros((n, T), np.float16)
    for index, seq in enumerate(sequences):
        tokens[index, :len(seq)] = seq
        position[index, len(seq) - 1] = np.float16(1)
    random_pad = tokens.copy()
    for index, length in enumerate(lengths):
        if length < T:
            random_pad[index, length:] = rng.integers(1, vocab_size, size=T - length, dtype=np.int32)
    return tokens, position, random_pad, sequences, lengths


def run_onnx_a(session, sequences):
    grouped = {}
    out = None
    for index, seq in enumerate(sequences):
        grouped.setdefault(len(seq), []).append(index)
    for indices in grouped.values():
        for start in range(0, len(indices), 32):
            chunk = indices[start:start + 32]
            batch = np.stack([sequences[i] for i in chunk]).astype(np.int32, copy=False)
            pred = session.run(["logprobs"], {"tokens": batch})[0].astype(np.float32, copy=False)
            if out is None:
                out = np.empty((len(sequences), pred.shape[-1]), np.float32)
            for row, index in enumerate(chunk):
                out[index] = pred[row]
    return out


def run_position_onnx(session, tokens, position):
    parts = []
    for start in range(0, len(tokens), B):
        pred = session.run(["logprobs"], {"tokens": tokens[start:start + B],
                                          "position": position[start:start + B]})[0]
        parts.append(np.asarray(pred, dtype=np.float32))
    return np.concatenate(parts, 0)


def run_position_coreml(model, tokens, position, label):
    parts = []
    steps = len(tokens) // B
    for start in range(0, len(tokens), B):
        pred = model.predict({"tokens": np.ascontiguousarray(tokens[start:start + B]),
                              "position": np.ascontiguousarray(position[start:start + B])})
        parts.append(np.asarray(pred["logprobs"], dtype=np.float32))
        done = start // B + 1
        if done % 25 == 0:
            print(f"{label} {done}/{steps}", flush=True)
    return np.concatenate(parts, 0)


def compare_pair(got, expected):
    error = np.abs(got.astype(np.float64) - expected.astype(np.float64))
    return {**diff_stats(error), "top1": int(np.sum(np.argmax(got, -1) == np.argmax(expected, -1))),
            "rows": int(got.shape[0])}


def top1_mismatches(got, expected, lengths):
    got_top = np.argmax(got, axis=-1)
    expected_top = np.argmax(expected, axis=-1)
    examples = []
    for index in np.flatnonzero(got_top != expected_top):
        chosen, truth = int(got_top[index]), int(expected_top[index])
        examples.append({"row": int(index), "length": int(lengths[index]),
                         "reference_id": truth, "other_id": chosen,
                         "reference_margin": float(expected[index, truth] - expected[index, chosen]),
                         "other_margin": float(got[index, chosen] - got[index, truth])})
    margins = [item["reference_margin"] for item in examples]
    return {"count": len(examples),
            "reference_margin_max": float(max(margins)) if margins else 0.0,
            "reference_margin_median": float(np.median(margins)) if margins else 0.0,
            "examples": examples}


def verify_position(args):
    import coremltools as ct
    import onnxruntime as ort

    name = position_name(args.select)
    manifest = json.loads((args.out / "export.json").read_text(encoding="utf-8"))
    vocab_size = manifest["config"]["vocab_size"]
    tokens, position, random_pad, sequences, lengths = make_sequences(vocab_size)
    a_path, a_hash = find_onnx_a()
    options = ort.SessionOptions()
    options.intra_op_num_threads = 1
    options.inter_op_num_threads = 1
    graph_a = ort.InferenceSession(str(a_path), options, providers=["CPUExecutionProvider"])
    graph = ort.InferenceSession(str(args.out / f"{name}.onnx"), options, providers=["CPUExecutionProvider"])
    print("onnx A", flush=True)
    reference = run_onnx_a(graph_a, sequences)
    print("onnx position", flush=True)
    onnx_got = run_position_onnx(graph, tokens, position)
    onnx_pad = run_position_onnx(graph, random_pad, position)
    ml = ct.models.CompiledMLModel(str(args.out / f"{name}.mlmodelc"), compute_units=ct.ComputeUnit.ALL)
    print("coreml position", flush=True)
    core_got = run_position_coreml(ml, tokens, position, "coreml")
    core_pad = run_position_coreml(ml, random_pad, position, "coreml-pad")
    report = {"seed": SEED, "rows": len(lengths), "select": args.select,
              "length_min_max": [int(min(lengths)), int(max(lengths))],
              "length_counts": {str(k): int(v) for k, v in sorted(Counter(lengths).items())},
              "generation": "1600 rows, length 2..40 inclusive, token 0 is BOS 1, other ids in 1..V-1, right pad 0",
              "reference": "dynamic ONNX A on the unpadded row; it reads the last position",
              "onnx_a": {"path": str(a_path), "sha256": a_hash,
                         "sha256_matches_prior_report": a_hash == ONNX_A_SHA256},
              "onnx_vs_onnx_a": compare_pair(onnx_got, reference),
              "coreml_vs_onnx_a": compare_pair(core_got, reference),
              "coreml_top1_mismatches": top1_mismatches(core_got, reference, lengths),
              "coreml_vs_onnx": compare_pair(core_got, onnx_got),
              "pad_onnx_zero_vs_random": compare_pair(onnx_pad, onnx_got),
              "pad_coreml_zero_vs_random": compare_pair(core_pad, core_got)}
    write_json(args.out / f"verification-{name}.json", report)
    print(json.dumps(report, ensure_ascii=False, indent=2), flush=True)


def snapshot():
    return {"time": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "load": list(os.getloadavg()),
            "uptime": subprocess.check_output(["uptime"], text=True).strip()}


def load_ok(snaps):
    return all(max(item["load"]) <= LOAD_LIMIT for item in snaps)


def versions():
    import coremltools as ct
    import onnxruntime as ort

    info = {"python": platform.python_version(), "platform": platform.platform(),
            "torch": torch.__version__, "numpy": np.__version__,
            "coremltools": ct.__version__, "onnxruntime": ort.__version__,
            "cpu": subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip(),
            "mem_bytes": int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True)),
            "sw_vers": subprocess.check_output(["sw_vers"], text=True).strip()}
    try:
        import onnx
        info["onnx"] = onnx.__version__
    except Exception as exc:
        info["onnx_error"] = f"{type(exc).__name__}: {exc}"
    return info


def time_predict(predict):
    while True:
        start = snapshot()
        if not load_ok([start]):
            print(f"waiting for load: {start['uptime']}", flush=True)
            time.sleep(30)
            continue
        snaps = [start]
        for _ in range(10):
            predict()
        snaps.append(snapshot())
        times = []
        for index in range(200):
            started = time.perf_counter_ns()
            predict()
            times.append((time.perf_counter_ns() - started) / 1e6)
            if (index + 1) % 25 == 0:
                snaps.append(snapshot())
        values = np.asarray(times, dtype=np.float64)
        record = {"snapshots": snaps, "ms": times, "median": float(np.median(values)),
                  "p95": float(np.percentile(values, 95)), "min": float(values.min()),
                  "max": float(values.max())}
        if load_ok(snaps):
            return record, None
        return None, record


def ensure_baseline(args):
    compiled = args.out / "full_b8_t40.mlmodelc"
    if compiled.is_dir() and (compiled / "metadata.json").is_file():
        return compiled
    if not Path(args.baseline).exists():
        raise FileNotFoundError(args.baseline)
    print(f"compiling baseline {args.baseline}", flush=True)
    return compile_mlpackage(args.baseline, args.out)


def bench_position(args):
    import coremltools as ct

    name = position_name(args.select)
    manifest = json.loads((args.out / "export.json").read_text(encoding="utf-8"))
    vocab = manifest["config"]["vocab_size"]
    rng = np.random.default_rng(SEED)
    tokens = rng.integers(1, vocab, size=(B, T), dtype=np.int32)
    tokens[:, 0] = 1
    lengths = (4, 8, 15, 40, 2, 21, 33, 11)
    position = np.zeros((B, T), np.float16)
    for index, length in enumerate(lengths):
        position[index, length - 1] = np.float16(1)
        tokens[index, length:] = 0
    baseline = ensure_baseline(args)
    report_path = args.out / f"latency-{name}.json"
    report = json.loads(report_path.read_text()) if report_path.exists() else {
        "warmup": 10, "repeats": 200, "load_limit": LOAD_LIMIT, "compute_units": "ALL",
        "lengths": list(lengths), "rows": [], "invalid_rounds": [], "versions": versions()}
    jobs = [("full_b8_t40", baseline, {"tokens": tokens}),
            ("position", args.out / f"{name}.mlmodelc", {"tokens": tokens, "position": position})]
    done = {(row["model"], row.get("units", "ALL")) for row in report["rows"]}
    for label, path, feed in jobs:
        if (label, "ALL") in done:
            print(f"skip {label}", flush=True)
            continue
        model = ct.models.CompiledMLModel(str(path), compute_units=ct.ComputeUnit.ALL)

        def predict(model=model, feed=feed):
            model.predict(feed)

        print(f"bench {label}", flush=True)
        attempt = 0
        while True:
            attempt += 1
            good, bad = time_predict(predict)
            if good is not None:
                report["rows"].append({"model": label, "units": "ALL", "attempt": attempt, **good})
                write_json(report_path, report)
                print(json.dumps({"model": label, "median": good["median"], "p95": good["p95"],
                                  "uptime": good["snapshots"][-1]["uptime"]}), flush=True)
                break
            report["invalid_rounds"].append({"model": label, "units": "ALL", "attempt": attempt, **bad})
            write_json(report_path, report)
            print(f"discarded overloaded round {label}", flush=True)
            if attempt >= 12:
                raise SystemExit(f"load stayed above {LOAD_LIMIT} for {label}")
        del model


def preferred_devices(model_path):
    import coremltools as ct
    from coremltools.models.compute_plan import MLComputePlan

    plan = MLComputePlan.load_from_path(str(model_path), compute_units=ct.ComputeUnit.ALL)
    preferred, operators = Counter(), Counter()
    by_device = {}

    def visit(block):
        for operation in block.operations:
            operators[operation.operator_name] += 1
            usage = plan.get_compute_device_usage_for_mlprogram_operation(operation)
            device = type(usage.preferred_compute_device).__name__ if usage else "unknown"
            preferred[device] += 1
            by_device.setdefault(device, Counter())[operation.operator_name] += 1
            for nested in operation.blocks:
                visit(nested)

    for function in plan.model_structure.program.functions.values():
        visit(function.block)
    return {"preferred_operation_count": dict(preferred), "operators": dict(operators),
            "operators_by_device": {key: dict(counts) for key, counts in by_device.items()}}


def plan_position(args):
    name = position_name(args.select)
    baseline = ensure_baseline(args)
    report = {}
    for label, path in (("full_b8_t40", baseline), ("position", args.out / f"{name}.mlmodelc")):
        try:
            report[label] = preferred_devices(path)
        except Exception as exc:
            report[label] = {"error": f"{type(exc).__name__}: {exc}"}
        print(label, json.dumps(report[label].get("preferred_operation_count", report[label]),
                                ensure_ascii=False), flush=True)
    write_json(args.out / f"compute-plan-{name}.json", report)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--ckpt", type=Path, default=ROOT / "ime_context_model/clean-v2/soup/best.pt")
    ap.add_argument("--out", type=Path, default=None)
    ap.add_argument("--format", choices=("all", "onnx", "coreml"), default="all")
    ap.add_argument("--position", choices=("export", "verify", "bench", "plan"), default=None)
    ap.add_argument("--select", choices=SELECTS, default="before_ln")
    ap.add_argument("--baseline", type=Path,
                    default=ROOT.parent / "zhiju-export-dist/tmp/zhiju-export/full_b8_t40.mlpackage")
    args = ap.parse_args()
    torch.set_num_threads(1)
    if args.position:
        args.out = args.out or (ROOT / "tmp/zhiju-position")
        args.out.mkdir(parents=True, exist_ok=True)
        {"export": export_position_cmd, "verify": verify_position,
         "bench": bench_position, "plan": plan_position}[args.position](args)
        return
    args.out = args.out or (ROOT / "tmp/zhiju-export")
    args.out.mkdir(parents=True, exist_ok=True)
    lm, vocab, _ = load(args.ckpt, "cpu")
    (args.out / "itos.json").write_text(json.dumps(vocab.itos, ensure_ascii=False), encoding="utf-8")
    manifest_path = args.out / "export.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}
    manifest.update(checkpoint=str(args.ckpt), checkpoint_sha256=sha256(args.ckpt),
                    config=vars(lm.config), torch=torch.__version__)
    if args.format in ("all", "onnx"):
        export_onnx(lm, args.out)
        manifest["onnx"] = {name: sha256(args.out / f"{name}.onnx") for name in ("full", "incremental")}
    if args.format in ("all", "coreml"):
        manifest["coreml"] = {}
        variants = [(False, None), (True, None)]
        variants += [(False, (b, t)) for t in (16, 40) for b in (1, 4, 8, 16)]
        variants += [(True, (b, s)) for s in (8, 24, 39) for b in (4, 8, 16)]
        for incremental, shape in variants:
            name = "incremental" if incremental else "full"
            if shape:
                name += f"_b{shape[0]}_{'s' if incremental else 't'}{shape[1]}"
            try:
                export_coreml(lm, args.out, incremental, shape)
                manifest["coreml"][name] = {"status": "exported"}
            except Exception as exc:
                manifest["coreml"][name] = {"status": "failed", "error": f"{type(exc).__name__}: {exc}"}
            manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
            print(json.dumps({name: manifest["coreml"][name]}), flush=True)
    manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    if any(item["status"] == "failed" for item in manifest.get("coreml", {}).values()):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
