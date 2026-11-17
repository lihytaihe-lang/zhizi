"""Export one 知字 v3 graph with factory gather scores and the next-character distribution.

Input matches the shipped model: tokens Int32[8,40]. Outputs:
  logprobs Float16[8,39] — StaticScorer's per-position gather, same math as export_coreml.py
  next_logprobs Float32[8,V] — stable log-softmax at the last non-pad token (pad id 0).
  An all-pad row has no content token; its defined position is 0.

  ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dual.py export
  ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dual.py verify
  ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dual.py bench
  ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dual.py footprint
  ime_context_model/.venv/bin/python scripts/context_model/zhiju/export_dual.py plan

Artifacts stay in tmp/zhiju-dual/ and are not part of the product package.
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

B, T = 8, 40
SEED = 20261111
ONNX_A_SHA256 = "369ab0950dfbb64dddf5d7eeab543460bb3aaa49c668bed8fd7fd5e360d7c43f"
LOAD_LIMIT = 8.0

FOOTPRINT_CHILD = r"""
import json, os, subprocess
import numpy as np
def rss():
    text = subprocess.check_output(["ps", "-o", "rss=", "-p", str(os.getpid())], text=True)
    return int(text.strip())
import coremltools as ct
after_import = rss()
model = ct.models.CompiledMLModel(os.environ["MODEL_PATH"], compute_units=ct.ComputeUnit.ALL)
after_load = rss()
tokens = np.zeros((8, 40), np.int32)
tokens[:, 0] = 1
model.predict({"tokens": tokens})
after_predict = rss()
print(json.dumps({
    "after_import_kb": after_import,
    "after_load_kb": after_load,
    "after_predict_kb": after_predict,
    "delta_load_kb": after_load - after_import,
    "delta_predict_kb": after_predict - after_import,
}))
"""


class DualScorer(torch.nn.Module):
    """Factory StaticScorer plus the next-character distribution at the last non-pad token."""

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
        # Same gather as StaticScorer. Head still runs on every position; the slice drops the last one.
        logits_all = lm.head(lm.ln_f(z))
        logits = logits_all[:, :-1]
        maximum = logits.max(dim=-1, keepdim=True).values
        lse = maximum + torch.log(torch.exp(logits - maximum).sum(dim=-1, keepdim=True))
        picked = torch.gather(logits, 2, x[:, 1:].unsqueeze(-1))
        logprobs = (picked - lse).squeeze(-1)
        # Float one-hot, not argmax: coremltools 9 rejects the int cast on that index.
        # Reverse cumsum is 1 at the last non-zero; an all-zero row selects position 0.
        # Counts up to 40 stay exact in float16, so the selection does not move under FP16.
        mask = (x != 0).to(dtype=torch.float32)
        reverse = torch.flip(torch.cumsum(torch.flip(mask, dims=(1,)), dim=1), dims=(1,))
        selected = ((reverse == 1) & (mask == 1)).to(dtype=torch.float32)
        fallback = (selected.sum(dim=1) == 0).to(dtype=torch.float32)
        selected = torch.cat((selected[:, :1] + fallback.unsqueeze(1), selected[:, 1:]), dim=1)
        nxt = (logits_all * selected.unsqueeze(-1)).sum(dim=1)
        nmax = nxt.max(dim=-1, keepdim=True).values
        nlse = nmax + torch.log(torch.exp(nxt - nmax).sum(dim=-1, keepdim=True))
        return logprobs, nxt - nlse


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")


def sha256_file(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def directory_bytes(path):
    return sum(item.stat().st_size for item in Path(path).rglob("*") if item.is_file())


def diff_stats(error):
    error = np.asarray(error, dtype=np.float64).reshape(-1)
    return {"max": float(error.max()), "median": float(np.median(error)), "n": int(error.size)}


def snapshot():
    return {"time": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "load": list(os.getloadavg()),
            "uptime": subprocess.check_output(["uptime"], text=True).strip()}


def load_ok(snaps):
    return all(max(item["load"]) <= LOAD_LIMIT for item in snaps)


def compiled_model(path, units):
    import coremltools as ct

    return ct.models.CompiledMLModel(str(path), compute_units=getattr(ct.ComputeUnit, units))


def onnx_session(path):
    import onnxruntime as ort

    options = ort.SessionOptions()
    options.intra_op_num_threads = 1
    options.inter_op_num_threads = 1
    return ort.InferenceSession(str(path), options, providers=["CPUExecutionProvider"])


def onnx_a_path():
    candidates = [
        ROOT.parent / "zhiju-export-dist/tmp/zhiju-export/full.onnx",
        ROOT.parents[2] / "zhiju-export/full.onnx",
    ]
    found = []
    for path in candidates:
        if path.is_file():
            found.append((sha256_file(path), path))
    matched = [item for item in found if item[0] == ONNX_A_SHA256]
    if matched:
        return matched[0][1], matched[0][0]
    if found:
        return found[0][1], found[0][0]
    raise FileNotFoundError("previous ONNX A graph full.onnx was not found")


def export_onnx(lm, out):
    example = torch.zeros((B, T), dtype=torch.int32)
    example[0, 0] = 1
    path = out / "zhizi_dual.onnx"
    with torch.no_grad():
        torch.onnx.export(DualScorer(lm, B, T).eval(), (example,), str(path),
                          input_names=["tokens"], output_names=["logprobs", "next_logprobs"],
                          opset_version=17, dynamo=False)
    return path


def export_coreml(lm, vocab_size, out):
    import coremltools as ct

    example = torch.randint(3, vocab_size, (B, T))
    example[0, 1:] = 0
    example[-1] = 0
    with torch.no_grad():
        traced = torch.jit.trace(DualScorer(lm, B, T).eval(), example)
    model = ct.convert(traced, inputs=[ct.TensorType(name="tokens", shape=(B, T), dtype=np.int32)],
                       outputs=[ct.TensorType(name="logprobs", dtype=np.float16),
                                ct.TensorType(name="next_logprobs", dtype=np.float32)],
                       compute_precision=ct.precision.FLOAT16,
                       minimum_deployment_target=ct.target.macOS13)
    package = out / "zhizi_dual.mlpackage"
    shutil.rmtree(package, ignore_errors=True)
    model.save(str(package))
    return package


def compile_package(package, out):
    compiled_dir = out / "compiled"
    shutil.rmtree(compiled_dir, ignore_errors=True)
    compiled_dir.mkdir(parents=True)
    result = subprocess.run(["xcrun", "coremlcompiler", "compile", str(package), str(compiled_dir)],
                            capture_output=True, text=True)
    (out / "compile.log").write_text(result.stdout + "\n" + result.stderr, encoding="utf-8")
    produced = compiled_dir / "zhizi_dual.mlmodelc"
    if result.returncode != 0 or not produced.is_dir():
        raise SystemExit(f"coremlcompiler failed ({result.returncode}); see {out / 'compile.log'}")
    target = out / "zhizi_dual.mlmodelc"
    shutil.rmtree(target, ignore_errors=True)
    shutil.move(str(produced), str(target))
    shutil.rmtree(compiled_dir, ignore_errors=True)
    return target


def export_all(args):
    import coremltools as ct

    args.out.mkdir(parents=True, exist_ok=True)
    lm, vocab, _ = load(args.ckpt, "cpu")
    (args.out / "itos.json").write_text(json.dumps(vocab.itos, ensure_ascii=False), encoding="utf-8")
    manifest = {"checkpoint": str(args.ckpt), "checkpoint_sha256": sha256_file(args.ckpt),
                "config": vars(lm.config), "vocab": len(vocab), "torch": torch.__version__,
                "coremltools": ct.__version__, "numpy": np.__version__,
                "input": "tokens Int32[8,40]",
                "outputs": {"logprobs": "Float16[8,39]", "next_logprobs": "Float32[8,V]"},
                "compute_precision": "FLOAT16", "minimum_deployment_target": "macOS13",
                "next_index": "last token != 0; all-pad rows use position 0"}
    write_json(args.out / "export.json", manifest)
    print("exporting onnx", flush=True)
    onnx_path = export_onnx(lm, args.out)
    manifest["onnx"] = {"path": onnx_path.name, "bytes": onnx_path.stat().st_size,
                        "sha256": sha256_file(onnx_path)}
    write_json(args.out / "export.json", manifest)
    print("exporting coreml", flush=True)
    package = export_coreml(lm, len(vocab), args.out)
    print("compiling", flush=True)
    compiled = compile_package(package, args.out)
    manifest["coreml"] = {
        "mlpackage_bytes": directory_bytes(package),
        "mlmodelc_bytes": directory_bytes(compiled),
        "mlmodelc_sha256": sha256_file(compiled / "coremldata.bin") if (compiled / "coremldata.bin").is_file() else None,
    }
    metadata = compiled / "metadata.json"
    if metadata.is_file():
        schema = json.loads(metadata.read_text(encoding="utf-8"))
        manifest["coreml"]["output_schema"] = schema[0].get("outputSchema")
        manifest["coreml"]["input_schema"] = schema[0].get("inputSchema")
    write_json(args.out / "export.json", manifest)
    print(json.dumps({"onnx_bytes": manifest["onnx"]["bytes"],
                      "mlmodelc_bytes": manifest["coreml"]["mlmodelc_bytes"]}), flush=True)


def candidate_scores(logprobs, length):
    # export_coreml.candidate_scores: sum of log P(candidate chars | left context).
    # These rows differ only in the final character, so the sum is that one column.
    return np.asarray(logprobs[:, length - 2], dtype=np.float64)


def ranking(scores):
    return np.argsort(-scores, kind="mergesort")


def verify(args):
    manifest = json.loads((args.out / "export.json").read_text(encoding="utf-8"))
    vocab_size = manifest["vocab"]
    a_path, a_hash = onnx_a_path()
    factory = compiled_model(args.factory, "ALL")
    dual = compiled_model(args.out / "zhizi_dual.mlmodelc", "ALL")
    factory_onnx = onnx_session(args.factory_onnx)
    dual_onnx = onnx_session(args.out / "zhizi_dual.onnx")
    graph_a = onnx_session(a_path)
    rng = np.random.default_rng(SEED)
    coreml_err, coreml_score_err = [], []
    onnx_err, onnx_score_err = [], []
    next_onnx_err, next_coreml_err = [], []
    mismatches, onnx_mismatches = [], []
    rank_top1 = rank_full = onnx_rank_full = 0
    tie_groups = 0
    next_onnx_top1 = next_coreml_top1 = 0
    lengths = []
    for index in range(200):
        length = int(rng.integers(2, 40))
        lengths.append(length)
        prefix = rng.integers(3, vocab_size, (1, length - 1), dtype=np.int32)
        prefix[:, 0] = 1
        chars = rng.choice(np.arange(3, vocab_size), 8, replace=False)
        rows = np.zeros((8, length), np.int32)
        rows[:, :-1] = prefix
        rows[:, -1] = chars
        padded = np.zeros((B, T), np.int32)
        padded[:, :length] = rows
        got = dual.predict({"tokens": padded})
        ref = factory.predict({"tokens": padded})
        got_lp = np.asarray(got["logprobs"])
        ref_lp = np.asarray(ref["logprobs"])
        coreml_err.append(np.abs(got_lp.astype(np.float64) - ref_lp.astype(np.float64)).reshape(-1))
        got_scores, ref_scores = candidate_scores(got_lp, length), candidate_scores(ref_lp, length)
        coreml_score_err.append(np.abs(got_scores - ref_scores))
        got_order, ref_order = ranking(got_scores), ranking(ref_scores)
        rank_top1 += int(got_order[0] == ref_order[0])
        same = bool(np.array_equal(got_order, ref_order))
        rank_full += int(same)
        if np.unique(ref_scores).size < ref_scores.size:
            tie_groups += 1
        if not same:
            mismatches.append({"group": index, "length": length,
                               "factory_scores": ref_scores.tolist(), "dual_scores": got_scores.tolist(),
                               "factory_order": ref_order.tolist(), "dual_order": got_order.tolist(),
                               "factory_min_gap": float(np.min(np.diff(np.sort(ref_scores)))),
                               "max_abs_score_diff": float(np.max(np.abs(got_scores - ref_scores)))})
        onnx_got = dual_onnx.run(["logprobs", "next_logprobs"], {"tokens": padded})
        onnx_ref = factory_onnx.run(["logprobs"], {"tokens": padded})[0]
        onnx_err.append(np.abs(onnx_got[0].astype(np.float64) - onnx_ref.astype(np.float64)).reshape(-1))
        onnx_scores, onnx_ref_scores = candidate_scores(onnx_got[0], length), candidate_scores(onnx_ref, length)
        onnx_score_err.append(np.abs(onnx_scores - onnx_ref_scores))
        onnx_order, onnx_ref_order = ranking(onnx_scores), ranking(onnx_ref_scores)
        onnx_same = bool(np.array_equal(onnx_order, onnx_ref_order))
        onnx_rank_full += int(onnx_same)
        if not onnx_same:
            onnx_mismatches.append({"group": index, "length": length,
                                    "factory_order": onnx_ref_order.tolist(),
                                    "dual_order": onnx_order.tolist(),
                                    "max_abs_score_diff": float(np.max(np.abs(onnx_scores - onnx_ref_scores)))})
        expected = graph_a.run(["logprobs"], {"tokens": padded[:, :length]})[0]
        next_core = np.asarray(got["next_logprobs"], dtype=np.float64)
        next_onnx = onnx_got[1].astype(np.float64)
        expected = expected.astype(np.float64)
        next_coreml_err.append(np.abs(next_core - expected).reshape(-1))
        next_onnx_err.append(np.abs(next_onnx - expected).reshape(-1))
        next_coreml_top1 += int(np.sum(np.argmax(next_core, -1) == np.argmax(expected, -1)))
        next_onnx_top1 += int(np.sum(np.argmax(next_onnx, -1) == np.argmax(expected, -1)))
        if (index + 1) % 25 == 0:
            print(f"verify {index + 1}/200", flush=True)
    report = {
        "seed": SEED, "groups": 200, "rows": 1600,
        "sequence_length_min_max": [int(min(lengths)), int(max(lengths))],
        "generation": "same loop as measure_dist.verify: length 2..39, bos at 0, 8 distinct final tokens",
        "candidate_score": "sum of the candidate span; the span is the final character, column length-2",
        "onnx_a": {"path": str(a_path), "sha256": a_hash, "sha256_matches_prior_report": a_hash == ONNX_A_SHA256},
        "logprobs_coreml_vs_factory": diff_stats(np.concatenate(coreml_err)),
        "logprobs_coreml_scoring_position": diff_stats(np.concatenate(coreml_score_err)),
        "logprobs_onnx_vs_factory": diff_stats(np.concatenate(onnx_err)),
        "logprobs_onnx_scoring_position": diff_stats(np.concatenate(onnx_score_err)),
        "ranking_coreml": {"full_order": f"{rank_full}/200", "first_place": f"{rank_top1}/200",
                           "factory_tie_groups": tie_groups, "mismatches": mismatches},
        "ranking_onnx": {"full_order": f"{onnx_rank_full}/200", "mismatches": onnx_mismatches},
        "next_logprobs_coreml_vs_onnx_a": {**diff_stats(np.concatenate(next_coreml_err)),
                                           "top1": f"{next_coreml_top1}/1600"},
        "next_logprobs_onnx_vs_onnx_a": {**diff_stats(np.concatenate(next_onnx_err)),
                                         "top1": f"{next_onnx_top1}/1600"},
    }
    write_json(args.out / "verification.json", report)
    printable = {key: value for key, value in report.items() if key not in ("ranking_coreml", "ranking_onnx")}
    printable["ranking_coreml"] = {key: value for key, value in report["ranking_coreml"].items() if key != "mismatches"}
    printable["ranking_coreml"]["mismatch_groups"] = [item["group"] for item in mismatches]
    printable["ranking_onnx"] = {key: value for key, value in report["ranking_onnx"].items() if key != "mismatches"}
    print(json.dumps(printable, ensure_ascii=False, indent=2), flush=True)


def time_model(path, units, feed):
    model = compiled_model(path, units)

    def predict():
        return model.predict(feed)

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
        record = {"snapshots": snaps, "ms": times, **diff_stats(times)}
        # diff_stats names max/median/n; latency also needs p95. median is already there.
        record["p95"] = float(np.percentile(np.asarray(times, dtype=np.float64), 95))
        if load_ok(snaps):
            del model
            return record, None
        del model
        return None, record


def bench(args):
    manifest = json.loads((args.out / "export.json").read_text(encoding="utf-8"))
    rng = np.random.default_rng(SEED)
    tokens = rng.integers(3, manifest["vocab"], size=(B, T), dtype=np.int32)
    tokens[:, 0] = 1
    feed = {"tokens": tokens}
    report = {"platform": platform.platform(), "compute_units_primary": "ALL",
              "warmup": 10, "repeats": 200, "load_limit": LOAD_LIMIT,
              "rows": [], "invalid_rounds": []}
    jobs = [("factory", args.factory, "ALL"),
            ("dual", args.out / "zhizi_dual.mlmodelc", "ALL"),
            ("dual", args.out / "zhizi_dual.mlmodelc", "CPU_ONLY")]
    for name, path, units in jobs:
        print(f"bench {name} {units}", flush=True)
        attempt = 0
        while True:
            attempt += 1
            good, bad = time_model(path, units, feed)
            if good is not None:
                report["rows"].append({"model": name, "units": units, "attempt": attempt, **good})
                write_json(args.out / "latency.json", report)
                print(json.dumps({"model": name, "units": units, "median": good["median"],
                                  "p95": good["p95"]}, ensure_ascii=False), flush=True)
                break
            report["invalid_rounds"].append({"model": name, "units": units, "attempt": attempt, **bad})
            write_json(args.out / "latency.json", report)
            print(f"discarded overloaded round {name} {units}", flush=True)
            if attempt >= 12:
                raise SystemExit(f"load stayed above {LOAD_LIMIT} for {name} {units}")


def footprint(args):
    import sys as _sys

    pairs = [("factory", args.factory), ("dual", args.out / "zhizi_dual.mlmodelc")]
    report = {"unit": "KB from ps rss", "runs": []}
    for name, path in pairs:
        report[name] = {"bytes": directory_bytes(path)}
        for repeat in range(3):
            env = os.environ.copy()
            env["MODEL_PATH"] = str(path)
            result = subprocess.run([_sys.executable, "-c", FOOTPRINT_CHILD],
                                    capture_output=True, text=True, env=env)
            row = {"model": name, "repeat": repeat, "returncode": result.returncode}
            if result.returncode == 0:
                row.update(json.loads(result.stdout.strip().splitlines()[-1]))
            else:
                row["stderr"] = result.stderr[-1500:]
            report["runs"].append(row)
            print(json.dumps(row, ensure_ascii=False), flush=True)
    write_json(args.out / "memory.json", report)


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
            "operators_by_device": {name: dict(counts) for name, counts in by_device.items()}}


def plan(args):
    report = {}
    for name, path in (("factory", args.factory), ("dual", args.out / "zhizi_dual.mlmodelc")):
        try:
            report[name] = preferred_devices(path)
        except Exception as exc:
            report[name] = {"error": f"{type(exc).__name__}: {exc}"}
        print(name, json.dumps(report[name].get("preferred_operation_count", report[name]),
                               ensure_ascii=False), flush=True)
    write_json(args.out / "compute-plan.json", report)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("export", "verify", "bench", "footprint", "plan"))
    parser.add_argument("--ckpt", type=Path, default=ROOT / "ime_context_model/clean-v2/soup/best.pt")
    parser.add_argument("--out", type=Path, default=ROOT / "tmp/zhiju-dual")
    parser.add_argument("--factory", type=Path,
                        default=ROOT / "ime_context_model/release/zhizi-30m-v3/zhizi30m_v2.mlmodelc")
    parser.add_argument("--factory-onnx", type=Path,
                        default=ROOT / "ime_context_model/release/zhizi-30m-v3-onnx/zhizi30m_v2.onnx")
    args = parser.parse_args()
    torch.set_num_threads(1)
    {"export": export_all, "verify": verify, "bench": bench,
     "footprint": footprint, "plan": plan}[args.command](args)


if __name__ == "__main__":
    main()
