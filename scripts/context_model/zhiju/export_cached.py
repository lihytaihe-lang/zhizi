"""Export explicit per-search caches; independent prefix reference gates remain fixed."""
import argparse
import json
from pathlib import Path
import sys
import time
import shutil
import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "zhizi"))
from model import load
from cached_dist import CachedPrefill, CachedStep
from export_dist import compile_mlpackage
from export_finetuned import sha
from export_mixed import TailPrecision
from check_quantized import statistics, accepts


def main():
    import coremltools as ct
    ap = argparse.ArgumentParser(description=__doc__)
    for key in ["checkpoint", "initial", "out", "protocol"]:
        ap.add_argument("--" + key, type=Path, required=True)
    ap.add_argument("--role", choices=["general", "trained"], required=True)
    ap.add_argument("--widths", type=int, nargs="+", choices=[16, 24, 32, 40], default=[16, 40])
    ap.add_argument("--reuse-from", type=Path, help="reuse checksum-verified full/step graphs for the same weights and cache format")
    ap.add_argument("--step-fp32", action="store_true", help="use FP32 arithmetic and CPU_ONLY for the one-token step; keep FP16 cache IO")
    a = ap.parse_args()
    if a.out.exists() and any(a.out.iterdir()):
        raise ValueError("refusing to overwrite export")
    a.out.mkdir(parents=True, exist_ok=True)
    torch.set_num_threads(2)
    lm, vocab, ck = load(a.checkpoint)
    initial = torch.load(a.initial, map_location="cpu")
    assert ck["itos"] == initial["itos"]
    changed = sum(not torch.equal(value, initial["state"][key]) for key, value in ck["state"].items())
    if a.role == "general":
        assert sha(a.checkpoint) == sha(a.initial) and changed == 0
    else:
        assert ck.get("zhijuTraining") and changed > 0 and ck["sourceCheckpointSHA256"] == sha(a.initial)
    protocol = json.loads(a.protocol.read_text())
    c = lm.config
    cache_shape = (c.n_layer, 8, c.n_head, 40, c.d_model // c.n_head)
    report = {"checkpointSHA256": sha(a.checkpoint), "initialSHA256": sha(a.initial),
              "sourceSHA256": sha(Path(__file__)), "cacheMathSHA256": sha(Path(__file__).with_name("cached_dist.py")),
              "protocolSHA256": sha(a.protocol), "role": a.role, "changedStateTensors": changed,
              "trainingAdded": 0, "cacheShape": list(cache_shape), "precision": "FP16-GEMM/FP32-norm-tail/FP16-cache",
              "scope": "numerical feasibility and direct-call diagnostics;not native40ms acceptance", "passed": True}
    report["stepComputePrecision"] = "float32" if a.step_fp32 else "mixed"
    report["stepComputeUnits"] = "cpuOnly" if a.step_fp32 else "all"
    (a.out / "itos.json").write_text(json.dumps(vocab.itos, ensure_ascii=False))
    if a.reuse_from:
        prior = json.loads((a.reuse_from / "PARITY.json").read_text())
        assert prior["passed"] and prior["checkpointSHA256"] == sha(a.checkpoint)
        assert prior["role"] == a.role and prior["cacheShape"] == list(cache_shape) and prior["precision"] == report["precision"]
        report["reusedManifestSHA256"] = sha(a.reuse_from / "SHA256SUMS")
        for line in (a.reuse_from / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split("  ", 1)
            relative = Path(name)
            assert not relative.is_absolute() and ".." not in relative.parts
            source = a.reuse_from / relative
            assert not source.is_symlink() and sha(source) == digest
            if a.step_fp32 and relative.parts[0] == "zhiju30m_v3_step.mlmodelc":
                continue
            target = a.out / relative; target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
    runtimes = {}
    jobs = [(width, "zhiju30m_v3_dist" + ("" if width == 40 else f"_t{width}")) for width in sorted(set(a.widths))]
    jobs.append((1, "zhiju30m_v3_step"))
    for width, name in jobs:
        existing = a.out / (name + ".mlmodelc")
        if existing.exists():
            runtimes[width] = ct.models.CompiledMLModel(str(existing), compute_units=ct.ComputeUnit.CPU_ONLY if width == 1 and a.step_fp32 else ct.ComputeUnit.ALL)
            continue
        tokens = torch.ones((8, width), dtype=torch.int32)
        position = torch.zeros((8, width if width != 1 else 40), dtype=torch.float16)
        position[:, 0] = 1
        inputs = [ct.TensorType(name="tokens", shape=tokens.shape, dtype=np.int32),
                  ct.TensorType(name="position", shape=position.shape, dtype=np.float16)]
        args = (tokens, position)
        module = CachedPrefill(lm, width=width)
        if width == 1:
            valid = torch.ones((8, 40), dtype=torch.float16)
            keys = values = torch.zeros(cache_shape)
            args = (tokens, position, valid, keys, values)
            module = CachedStep(lm)
            inputs += [ct.TensorType(name="valid", shape=valid.shape, dtype=np.float16),
                       ct.TensorType(name="keys", shape=cache_shape, dtype=np.float16),
                       ct.TensorType(name="values", shape=cache_shape, dtype=np.float16)]
        with torch.no_grad():
            traced = torch.jit.trace(module.eval(), args)
        policy = TailPrecision(stream_mode="norm")
        fp32_step = width == 1 and a.step_fp32
        model = ct.convert(traced, convert_to="mlprogram", skip_model_load=True,
                           compute_precision=ct.precision.FLOAT32 if fp32_step else ct.transform.FP16ComputePrecision(op_selector=policy),
                           minimum_deployment_target=ct.target.macOS13, inputs=inputs,
                           outputs=[ct.TensorType(name="logprobs", dtype=np.float32),
                                    ct.TensorType(name="new_keys", dtype=np.float16),
                                    ct.TensorType(name="new_values", dtype=np.float16)])
        assert fp32_step or policy.found_norm and policy.found_head
        model.user_defined_metadata.update({"zhijuCheckpointSHA256": sha(a.checkpoint), "zhijuModelRole": a.role,
                                            "zhijuComputePrecision": "fp32-step-fp16-cache" if fp32_step else "fp16-norm-cache", "zhijuCacheFormat": "zhiju-cache/1"})
        package = a.out / (name + ".mlpackage"); model.save(str(package))
        compiled = compile_mlpackage(package, a.out)
        runtimes[width] = ct.models.CompiledMLModel(str(compiled), compute_units=ct.ComputeUnit.CPU_ONLY if fp32_step else ct.ComputeUnit.ALL)
    assert 40 in runtimes, "full graph required for bounded fallback"

    def prefill(x, lengths, random_padding=False, force_full=False):
        width = 40 if force_full else next(width for width in sorted(runtimes) if width != 1 and width >= max(lengths))
        x = x[:, :width].copy(); p = np.zeros((8, width), np.float16)
        for row, length in enumerate(lengths):
            p[row, length-1] = 1
            if random_padding:
                rng = np.random.default_rng(9000 + int(x[row, :length].sum()))
                x[row, length:] = rng.integers(3, len(vocab), size=width-length)
        return runtimes[width].predict({"tokens": x, "position": p})

    report["prefill"] = {}
    rng = np.random.default_rng(protocol["seed"])
    for width in [16, 40]:
        got, ref, padded = [], [], []
        for _ in range(protocol["rowsPerShape"] // 8):
            lengths = rng.integers(1, width+1, size=8); x = np.zeros((8, 40), np.int32)
            for row, length in enumerate(lengths):
                x[row, :length] = np.r_[1, rng.integers(3, len(vocab), size=length-1)]
                with torch.no_grad():
                    ref.append(torch.log_softmax(lm(torch.from_numpy(x[row:row+1, :length]).long())[0, -1], -1).numpy())
            got.extend(prefill(x, lengths)["logprobs"])
            padded.extend(prefill(x, lengths, True)["logprobs"])
        metrics = statistics(np.asarray(got), np.asarray(ref))
        metrics["randomPaddingMaxAbsoluteError"] = float(np.abs(np.asarray(got)-np.asarray(padded)).max())
        metrics["passed"] = bool(accepts(metrics, protocol) and metrics["randomPaddingMaxAbsoluteError"] <= 1e-5)
        report["prefill"][str(width)] = metrics; report["passed"] &= metrics["passed"]
        print(json.dumps({"prefill": width, **metrics}), flush=True)

    short, full = [], []
    for _ in range(8):
        lengths = rng.integers(1, 17, size=8); x = np.zeros((8, 40), np.int32)
        for row, length in enumerate(lengths):
            x[row, :length] = np.r_[1, rng.integers(3, len(vocab), size=length-1)]
        short.extend(prefill(x, lengths)["logprobs"])
        full.extend(prefill(x, lengths, force_full=True)["logprobs"])
    metrics = statistics(np.asarray(short), np.asarray(full)); metrics["passed"] = bool(accepts(metrics, protocol))
    report["shortVsFull"] = metrics; report["passed"] &= metrics["passed"]

    # New independent sequences exercise repeated steps, duplicated/reordered
    # parents, different start lengths and the last accepted absolute position.
    rng = np.random.default_rng(20261125)
    got, ref, step_ms = [], [], []
    for group in range(32):
        start = int(rng.integers(1, 33))
        prefix = rng.integers(3, len(vocab), size=(8, start), dtype=np.int32); prefix[:, 0] = 1
        x = np.zeros((8, 40), np.int32); x[:, :start] = prefix
        output = prefill(x, [start]*8)
        for length in range(start+1, min(start+8, 40)):
            parents = rng.integers(0, 8, size=8)
            prefix = np.concatenate((prefix[parents], rng.integers(3, len(vocab), size=(8, 1), dtype=np.int32)), axis=1)
            position = np.zeros((8, 40), np.float16); position[:, length-1] = 1
            valid = np.zeros((8, 40), np.float16); valid[:, :length] = 1
            inputs = {"tokens": prefix[:, -1:].copy(), "position": position, "valid": valid,
                      "keys": np.ascontiguousarray(output["new_keys"][:, parents]),
                      "values": np.ascontiguousarray(output["new_values"][:, parents])}
            started = time.perf_counter_ns(); output = runtimes[1].predict(inputs)
            step_ms.append((time.perf_counter_ns()-started)/1e6)
            got.extend(output["logprobs"])
            with torch.no_grad():
                ref.extend(torch.log_softmax(lm(torch.from_numpy(prefix).long())[:, -1], -1).numpy())
    metrics = statistics(np.asarray(got), np.asarray(ref)); metrics["passed"] = bool(accepts(metrics, protocol))
    report["incremental"] = metrics; report["passed"] &= metrics["passed"]
    report["directStepTiming"] = {"count": len(step_ms), "medianMs": float(np.median(step_ms)),
                                  "p95Ms": float(np.percentile(step_ms, 95)), "includesPythonBridge": True}
    descriptor = {"format": "zhiju-cache/1", "stepModel": "zhiju30m_v3_step.mlmodelc", "capacity": 40,
                  "layers": c.n_layer, "heads": c.n_head, "headDimension": c.d_model // c.n_head,
                  "cacheDataType": "float16", "checkpointSHA256": sha(a.checkpoint)}
    if a.step_fp32:
        descriptor["stepComputeUnits"] = "cpuOnly"
    (a.out / "cache.json").write_text(json.dumps(descriptor, indent=2)+'\n')
    (a.out / "PARITY.json").write_text(json.dumps(report, indent=2)+'\n')
    (a.out / "SHA256SUMS").write_text(''.join(f'{sha(p)}  {p.relative_to(a.out)}\n' for p in sorted(a.out.rglob('*'))
        if p.is_file() and ('.mlmodelc' in str(p.relative_to(a.out)) or p.name in ['itos.json', 'cache.json'])))
    print(json.dumps({"incremental": metrics, "timing": report["directStepTiming"], "passed": report["passed"]}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
