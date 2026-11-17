"""Serial correctness, latency, and 700-case generation checks for export_dist.

Use the model venv. Commands: verify, evaluate, bench (all write under --out).
The benchmark accepts a round only if every sampled 1/5/15-minute load is <=8.
"""
import argparse
from collections import Counter
import importlib.util
import json
import os
import platform
import subprocess
import time
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[3]
SEED = 20261111
STYLES = ("all_initials", "head_full", "random_mix", "tail_full")


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")


def select_cases(rows):
    selected = []
    for sub in STYLES:
        bucket = sorted((r for r in rows if r["sub"] == sub), key=lambda r: r["id"])
        if len(bucket) < 175:
            raise ValueError(f"{sub}: need 175 cases, got {len(bucket)}")
        selected.extend(bucket[:175])
    return sorted(selected, key=lambda r: r["id"])


def accepted_round(loads):
    return all(max(load) <= 8.0 for load in loads)


def load_snapshot():
    return {"time": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "load": list(os.getloadavg()),
            "uptime": subprocess.check_output(["uptime"], text=True).strip()}


def stats(values):
    values = np.asarray(values, dtype=np.float64)
    return {"max": float(values.max()), "median": float(np.median(values)),
            "p95": float(np.percentile(values, 95)), "n": int(values.size)}


def session(path, threads):
    import onnxruntime as ort

    options = ort.SessionOptions()
    options.intra_op_num_threads = threads
    options.inter_op_num_threads = 1
    return ort.InferenceSession(str(path), options, providers=["CPUExecutionProvider"])


def coreml(path):
    import coremltools as ct

    path = Path(path)
    if path.suffix == ".mlpackage":
        destination = path.parent / "compiled"
        destination.mkdir(exist_ok=True)
        compiled = destination / f"{path.stem}.mlmodelc"
        receipt = destination / f"{path.stem}-compile.json"
        if not receipt.exists():
            result = subprocess.run(["xcrun", "coremlcompiler", "compile", str(path), str(destination)],
                                    capture_output=True, text=True)
            write_json(receipt, {"returncode": result.returncode, "stdout": result.stdout, "stderr": result.stderr})
        # On this sandbox coremlcompiler emits a usable model but cannot write
        # metadata.json. Keep its nonzero exit receipt; predict must still work
        # before any measurement can be recorded.
        path = compiled
    return ct.models.CompiledMLModel(str(path), compute_units=ct.ComputeUnit.ALL)


def torch_model():
    import torch
    from export_dist import load

    torch.set_num_threads(1)
    return load(ROOT / "ime_context_model/clean-v2/soup/best.pt", "cpu")[0]


def prefill(lm, tokens):
    import torch
    from export_dist import IncrementalDistribution

    c = lm.config
    b = tokens.shape[0]
    keys = values = torch.empty((c.n_layer, b, c.n_head, 0, c.d_model // c.n_head))
    with torch.no_grad():
        for pos in range(tokens.shape[1]):
            _, keys, values = IncrementalDistribution(lm)(torch.from_numpy(tokens[:, pos:pos + 1]), keys, values)
    return keys.numpy(), values.numpy()


def verify(out, threads):
    import torch
    from export_dist import FullDistribution, IncrementalDistribution

    lm = torch_model()
    vocab_size = lm.config.vocab_size
    full = session(out / "full.onnx", threads)
    step = session(out / "incremental.onnx", threads)
    shipped = session(ROOT / "ime_context_model/release/zhizi-30m-v3-onnx/zhizi30m_v2.onnx", threads)
    ml_full = coreml(out / "full.mlpackage")
    ml_step = coreml(out / "incremental.mlpackage")
    ml_shipped = coreml(ROOT / "ime_context_model/release/zhizi-30m-v3/zhizi30m_v2.mlmodelc")
    rng = np.random.default_rng(SEED)
    comparisons = {name: {"errors": [], "top1": 0, "rounded_top1": 0} for name in
                   ("onnx_vs_shipped_onnx", "coreml_vs_shipped_coreml", "coreml_vs_onnx_candidates")}
    full_errors, full_top1 = [], 0
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
        padded = np.zeros((8, 40), np.int32)
        padded[:, :length] = rows
        a = full.run(["logprobs"], {"tokens": prefix})[0][0]
        m = ml_full.predict({"tokens": prefix})["logprobs"][0]
        ref = shipped.run(["logprobs"], {"tokens": rows})[0][:, length - 2]
        ml_ref = ml_shipped.predict({"tokens": padded})["logprobs"][:, length - 2]
        for name, got, expected in (("onnx_vs_shipped_onnx", a[chars], ref),
                                     ("coreml_vs_shipped_coreml", m[chars], ml_ref),
                                     ("coreml_vs_onnx_candidates", m[chars], a[chars])):
            record = comparisons[name]
            record["errors"].extend(np.abs(np.asarray(got, np.float64) - expected).tolist())
            record["top1"] += int(np.argmax(got) == np.argmax(expected))
            record["rounded_top1"] += int(np.argmax(np.round(got.astype(np.float64), 3)) ==
                                          np.argmax(np.round(expected.astype(np.float64), 3)))
        full_errors.extend(np.abs(m.astype(np.float64) - a).tolist())
        full_top1 += int(np.argmax(m) == np.argmax(a))
        if (index + 1) % 25 == 0:
            print(f"gather {index + 1}/200", flush=True)
    report = {"seed": SEED, "groups": 200, "candidates_per_group": 8,
              "sequence_length_min_max": [min(lengths), max(lengths)], "round_decimals": 3,
              "comparisons": {name: {**stats(r.pop("errors")), **r} for name, r in comparisons.items()},
              "coreml_vs_onnx_full_vocab": {**stats(full_errors), "top1": full_top1}}
    write_json(out / "verification.json", report)
    report["incremental"] = []
    for b, length in ((1, 2), (4, 9), (8, 25), (16, 40)):
        tokens = rng.integers(3, vocab_size, (b, length), dtype=np.int32)
        tokens[:, 0] = 1
        keys = values = np.empty((lm.config.n_layer, b, lm.config.n_head, 0,
                                  lm.config.d_model // lm.config.n_head), np.float32)
        mk = mv = keys.astype(np.float16)
        row = {"batch": b, "length": length, "onnx_step_max_abs": [], "coreml_step_max_abs": []}
        for pos in range(length):
            feed = {"tokens": tokens[:, pos:pos + 1], "keys": keys, "values": values}
            dist, keys, values = step.run(None, feed)
            a = full.run(["logprobs"], {"tokens": tokens[:, :pos + 1]})[0]
            row["onnx_step_max_abs"].append(float(np.max(np.abs(dist - a))))
            try:
                got = ml_step.predict({"tokens": feed["tokens"], "keys": mk, "values": mv})
            except Exception as exc:
                if pos != 0:
                    raise
                row["empty_cache_error"] = f"{type(exc).__name__}: {exc}"
                # BOS is constant; test the remaining steps even if Core ML
                # refuses empty tensors. This fallback is recorded, not hidden.
                mk, mv = prefill(lm, tokens[:, :1])
                mk, mv = mk.astype(np.float16), mv.astype(np.float16)
                continue
            mk, mv = got["new_keys"], got["new_values"]
            ma = ml_full.predict({"tokens": tokens[:, :pos + 1]})["logprobs"]
            row["coreml_step_max_abs"].append(float(np.max(np.abs(got["logprobs"] - ma))))
        with torch.no_grad():
            original = torch.log_softmax(lm(torch.from_numpy(tokens).long())[:, -1], -1).numpy()
        row["onnx_vs_original_max_abs"] = float(np.max(np.abs(dist - original)))
        row["coreml_vs_original_max_abs"] = float(np.max(np.abs(got["logprobs"] - original)))
        row["coreml_final_top1_vs_full"] = int(np.sum(np.argmax(got["logprobs"], -1) == np.argmax(ma, -1)))
        report["incremental"].append(row)
        write_json(out / "verification.json", report)
        print(f"incremental B={b} T={length}: {row}", flush=True)


def benchmark(out, threads):
    import coremltools as ct
    import onnxruntime as ort
    import torch
    from export_dist import FullDistribution

    lm = torch_model()
    rng = np.random.default_rng(SEED)
    report_path = out / "latency.json"
    report = json.loads(report_path.read_text()) if report_path.exists() else {
        "platform": platform.platform(), "onnxruntime": ort.__version__, "coremltools": ct.__version__,
        "compute_units": "ALL", "onnx_intra_threads": threads, "onnx_inter_threads": 1,
        "coreml_shapes": "fixed shapes, as in export_coreml.py",
        "warmup": 10, "repeats": 200, "load_limit": "all of 1/5/15 minute averages <=8",
        "rows": [], "invalid_rounds": [], "waits": []}
    for variant in ("full", "incremental"):
        shapes = ([(b, t) for t in (16, 40) for b in (1, 4, 8, 16)] if variant == "full"
                  else [(b, s) for s in (8, 24, 39) for b in (4, 8, 16)])
        for backend in ("coreml", "onnx"):
            model = None if backend == "coreml" else session(out / f"{variant}.onnx", threads)
            for b, size in shapes:
                tag = {"variant": variant, "backend": backend, "batch": b, "size": size}
                if any(all(r[k] == v for k, v in tag.items()) for r in report["rows"]):
                    continue
                if backend == "coreml":
                    name = f"{variant}_b{b}_{'t' if variant == 'full' else 's'}{size}"
                    model = coreml(out / f"{name}.mlpackage")
                tokens = rng.integers(3, lm.config.vocab_size, (b, size if variant == "full" else size + 1),
                                      dtype=np.int32)
                tokens[:, 0] = 1
                if variant == "full":
                    feed = {"tokens": tokens}
                else:
                    keys, values = prefill(lm, tokens[:, :-1])
                    dtype = np.float16 if backend == "coreml" else np.float32
                    feed = {"tokens": tokens[:, -1:], "keys": keys.astype(dtype), "values": values.astype(dtype)}

                def predict():
                    return model.predict(feed) if backend == "coreml" else model.run(None, feed)

                with torch.no_grad():
                    reference = FullDistribution(lm)(torch.from_numpy(tokens)).numpy()
                first = predict()
                first = first["logprobs"] if backend == "coreml" else first[0]
                numerical = {**stats(np.abs(first.astype(np.float64) - reference)),
                             "top1": int(np.sum(np.argmax(first, -1) == np.argmax(reference, -1))),
                             "rows": b}
                while True:
                    snapshot = load_snapshot()
                    if not accepted_round([snapshot["load"]]):
                        report["waits"].append({**tag, **snapshot})
                        write_json(report_path, report)
                        print(f"waiting for load: {snapshot['uptime']}", flush=True)
                        time.sleep(30)
                        continue
                    snapshots = [snapshot]
                    for _ in range(10):
                        predict()
                    snapshots.append(load_snapshot())
                    times = []
                    for index in range(200):
                        started = time.perf_counter_ns()
                        predict()
                        times.append((time.perf_counter_ns() - started) / 1e6)
                        if (index + 1) % 25 == 0:
                            snapshots.append(load_snapshot())
                    record = {**tag, "ms": times, "snapshots": snapshots,
                              "numeric_vs_fp32": numerical, **stats(times)}
                    if accepted_round([snap["load"] for snap in snapshots]):
                        report["rows"].append(record)
                        write_json(report_path, report)
                        print(json.dumps({**tag, **stats(times)}), flush=True)
                        break
                    report["invalid_rounds"].append(record)
                    write_json(report_path, report)
                    print(f"discarded overloaded round: {tag}", flush=True)
            del model


def generator():
    spec = importlib.util.spec_from_file_location("zhiju_generate", ROOT / "scripts/eval/zhizi_generate/generate.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CountedModel:
    def __init__(self, model):
        self.model = model
        self.steps = []

    def logprobs(self, prefixes):
        self.steps.append({"batch": len(prefixes), "length": len(prefixes[0])})
        return self.model.logprobs(prefixes)


def evaluate(out, threads):
    from export_dist import sha256

    gen = generator()
    path = ROOT / "tmp/sent-abbrev/tune.jsonl"
    cases = select_cases(gen.read_jsonl(path))
    write_json(out / "case_ids.json", [r["id"] for r in cases])
    summary = {"seed": SEED, "input_sha256": sha256(path), "case_ids_sha256": sha256(out / "case_ids.json"),
               "beam": 8, "mu": 0.0, "threads": threads, "models": {}}
    for version in ("v2", "v3"):
        vocab_path = (out / "itos.json" if version == "v3" else
                      ROOT / "ime_context_model/release/zhizi-30m-v2/itos.json")
        vocab = gen.Vocab(json.loads(vocab_path.read_text(encoding="utf-8")))
        readings, lexicon, table_info = gen.load_tables(vocab)
        sess, model_name = gen.open_session(threads, out / "full.onnx" if version == "v3" else None)
        if version == "v2":
            summary["v2_gather_max_abs"] = gen.check_matches_gather(sess)
        model = CountedModel(gen.OnnxNext(sess, distribution=version == "v3"))
        groups = {sub: {"n": 0, "ok1": 0, "ok5": 0} for sub in (*STYLES, "total")}
        started = time.perf_counter()
        with (out / f"evaluation-{version}.jsonl").open("w", encoding="utf-8") as handle:
            for index, case in enumerate(cases, 1):
                model.steps = []
                ranked, timing = gen.beam_search(case["raw"], case["context"], model, vocab, readings,
                                                lexicon, beam=8, mu=0.0)
                rank = gen.rank_of(case["target"], ranked)
                for sub in (case["sub"], "total"):
                    groups[sub]["n"] += 1
                    groups[sub]["ok1"] += int(rank == 0)
                    groups[sub]["ok5"] += int(0 <= rank < 5)
                record = {"id": case["id"], "sub": case["sub"], "rank": rank,
                          "top": [h.text for h in ranked[:5]], "steps": model.steps, **timing}
                handle.write(json.dumps(record, ensure_ascii=False) + "\n")
                if index % 25 == 0:
                    handle.flush()
                    print(f"{version} {index}/700 {time.perf_counter() - started:.1f}s", flush=True)
        summary["models"][version] = {"model": model_name, "groups": groups, "tables": table_info,
                                         "elapsed_s": time.perf_counter() - started}
        write_json(out / "accuracy.json", summary)
        del model, sess


def compute_plan(out, threads):
    import coremltools as ct
    from coremltools.models.compute_plan import MLComputePlan

    report = {}
    for name in ("full", "incremental", "full_b8_t40", "incremental_b8_s24"):
        try:
            model = coreml(out / f"{name}.mlpackage")
            plan = MLComputePlan.load_from_path(str(out / "compiled" / f"{name}.mlmodelc"),
                                                compute_units=ct.ComputeUnit.ALL)
            preferred, weighted, operators = Counter(), Counter(), Counter()

            def visit(block):
                for operation in block.operations:
                    operators[operation.operator_name] += 1
                    usage = plan.get_compute_device_usage_for_mlprogram_operation(operation)
                    device = type(usage.preferred_compute_device).__name__ if usage else "unknown"
                    preferred[device] += 1
                    cost = plan.get_estimated_cost_for_mlprogram_operation(operation)
                    if cost is not None:
                        weighted[device] += cost.weight
                    for nested in operation.blocks:
                        visit(nested)

            for function in plan.model_structure.program.functions.values():
                visit(function.block)
            report[name] = {"preferred_operation_count": dict(preferred),
                            "estimated_cost_weight": dict(weighted), "operators": dict(operators)}
            del model
        except Exception as exc:
            report[name] = {"error": f"{type(exc).__name__}: {exc}"}
    write_json(out / "compute-plan.json", report)
    print(json.dumps(report, indent=2), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("verify", "bench", "evaluate", "plan"))
    parser.add_argument("--out", type=Path, default=ROOT / "tmp/zhiju-export")
    parser.add_argument("--threads", type=int, default=4)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    {"verify": verify, "bench": benchmark, "evaluate": evaluate, "plan": compute_plan}[args.command](
        args.out, args.threads)


if __name__ == "__main__":
    main()
