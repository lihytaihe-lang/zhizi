"""Recheck frozen S1 slot rules with the production Core ML scorer."""
import argparse
import copy
import json
import math
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import margin_gate as gate

ROOT = Path("tmp/delegation/rank-s2-coreml-recheck")
ARTIFACTS = Path(__file__).resolve().parents[3] / "tmp" / "rank-s1-artifacts"
MODES = (("current", False, True), ("fixed_model", True, False),
         ("gate_model", True, False), ("gate_current", False, False))


def distribution(values):
    import numpy as np
    if not values:
        return dict(count=0, median=None, p95=None, max=None)
    if not all(math.isfinite(v) for v in values):
        raise ValueError("nonfinite comparison value")
    median, p95 = np.quantile(values, [.5, .95])
    return dict(count=len(values), median=float(median), p95=float(p95), max=float(max(values)))


def validate_pool(row, coreml=False):
    texts = row["texts"]
    if len(texts) != len(set(texts)) or any(not 1 <= len(t) <= 39 for t in texts):
        raise ValueError("duplicate or invalid scored text")
    for key in ("scores", "coreml_scores") if coreml else ("scores",):
        if len(row[key]) != len(texts) or any(not math.isfinite(v) for v in row[key]):
            raise ValueError("missing or nonfinite scores")


def validate_pairs(current, scored):
    if len(current) != len(scored) or len({r["id"] for r in current}) != len(current):
        raise ValueError("missing or duplicate paired rows")
    for old, row in zip(current, scored):
        if any(old[k] != row[k] for k in ("id", "context")):
            raise ValueError("current/pool alignment mismatch")
        validate_pool(row, coreml=True)


def prepare_cases(pool, n):
    if n not in gate.LIMITS:
        raise ValueError("unexpected candidate limit")
    cases = []
    for row in pool:
        validate_pool(row)
        if row["texts"]:
            cases.append(dict(context=row["context"], candidates=row["texts"][:n]))
    return cases


def attach_coreml(pool, n, report):
    cases = prepare_cases(pool, n)
    if len(cases) != len(report["rows"]):
        raise ValueError("harness row count mismatch")
    result, index = [], 0
    for row in pool:
        texts = row["texts"][:n]
        scores = report["rows"][index]["scores"] if texts else []
        item = dict(id=row["id"], context=row["context"], texts=texts,
                    scores=row["scores"][:n], coreml_scores=scores)
        validate_pool(item, coreml=True)
        result.append(item)
        index += bool(texts)
    return result


def compare(current, scored, threshold):
    validate_pairs(current, scored)
    details = []
    for old, row in zip(current, scored):
        texts, onnx, coreml = row["texts"], row["scores"], row["coreml_scores"]
        head = old["candidates"][0] if old["candidates"] else None
        best = [texts[max(range(len(texts)), key=lambda i: scores[i])] if texts else None
                for scores in (onnx, coreml)]
        margins = [max(scores) - scores[texts.index(head)] if head in texts else None
                   for scores in (onnx, coreml)]
        outputs = [gate.arrange(old["candidates"], texts, scores, old["context"], threshold, False)
                   for scores in (onnx, coreml)]
        heads = [out[0] if out else None for out in outputs]
        swaps = [h != head for h in heads]
        direction = ("onnx_only" if swaps == [True, False] else
                     "coreml_only" if swaps == [False, True] else
                     "both_different" if all(swaps) and heads[0] != heads[1] else "same")
        details.append(dict(id=old["id"], current_head=head, onnx_best=best[0], coreml_best=best[1],
                            max_abs_score_delta=max((abs(a - b) for a, b in zip(onnx, coreml)), default=None),
                            onnx_margin=margins[0], coreml_margin=margins[1],
                            margin_delta=margins[1] - margins[0] if margins[0] is not None else None,
                            eligible=bool(threshold is not None and old["context"] and gate.han(head)
                                          and head in texts),
                            threshold=threshold, onnx_head=heads[0], coreml_head=heads[1],
                            onnx_swapped=swaps[0], coreml_swapped=swaps[1], direction=direction))
    nonempty = [r for r in details if r["onnx_best"] is not None]
    result = dict(total=len(details), nonempty=len(nonempty),
                  same_best=sum(r["onnx_best"] == r["coreml_best"] for r in nonempty),
                  eligible=sum(r["eligible"] for r in details),
                  onnx_only_swap=sum(r["direction"] == "onnx_only" for r in details),
                  coreml_only_swap=sum(r["direction"] == "coreml_only" for r in details),
                  both_swap_different_head=sum(r["direction"] == "both_different" for r in details))
    for key in ("max_abs_score_delta", "onnx_margin", "coreml_margin", "margin_delta"):
        result[key] = distribution([r[key] for r in details if r[key] is not None])
    result["abs_margin_delta"] = distribution([abs(r["margin_delta"]) for r in details
                                               if r["margin_delta"] is not None])
    return result, details


def replay(current, scored, n, threshold, backend):
    validate_pairs(current, scored)
    if backend not in ("onnx", "coreml"):
        raise ValueError("unexpected backend")
    converted = [dict(r, scores=r["coreml_scores"] if backend == "coreml" else r["scores"]) for r in scored]
    results, outputs = [], {}
    for mode, tail, baseline in MODES:
        choice = threshold if mode.startswith("gate_") else None
        summary, output = gate.evaluate(current, converted, n, choice, tail, baseline)
        results.append(dict(mode=mode, threshold=choice, **summary))
        outputs[mode] = output
    return results, outputs


def reference_threshold(current, scored, n):
    validate_pairs(current, scored)
    before = [bool(r["candidates"]) and r["candidates"][0] == r["target"] for r in current]
    trials = {}
    for threshold in gate.THRESHOLDS:
        outputs = [gate.arrange(old["candidates"], row["texts"][:n], row["coreml_scores"][:n],
                                old["context"], threshold, True) for old, row in zip(current, scored)]
        after = [bool(out) and out[0] == old["target"] for old, out in zip(current, outputs)]
        trials[threshold] = gate.paired_counts(before, after, [r["target"] for r in current])
    return dict(threshold=gate.select_threshold(trials), trials=trials)


def load_inputs(artifacts):
    selection = json.loads((artifacts / "selection.json").read_text())
    manifest = json.loads((artifacts / "dataset.json").read_text())
    if gate.digest(artifacts / "dataset.json") != selection["dataset_sha256"]:
        raise ValueError("S1 dataset identity changed")
    inputs, hashes = {}, {}
    for phase in ("test", "tune"):
        path = artifacts / f"{phase}.jsonl"
        if gate.digest(path) != manifest["files"][path.name]:
            raise ValueError("S1 case file hash changed")
        cases = gate.read_jsonl(path)
        hashes[str(path)] = gate.digest(path)
        for bucket in gate.BUCKETS:
            expected = [r for r in cases if r["bucket"] == bucket]
            rows = []
            for mode in ("current", "pool"):
                path = artifacts / f"{phase}-run/{mode}-{bucket}/rows.jsonl"
                part = gate.read_jsonl(path)
                if len(part) != len(expected) or len({r["id"] for r in part}) != len(part):
                    raise ValueError("S1 row count or IDs changed")
                if any(any(a[k] != b[k] for k in ("id", "bucket", "raw", "context", "target", "source"))
                       for a, b in zip(expected, part)):
                    raise ValueError("S1 case/engine alignment mismatch")
                hashes[str(path)] = gate.digest(path)
                rows.append(part)
            inputs[phase, bucket] = (rows[0], rows[1], expected)
    hashes[str(artifacts / "selection.json")] = gate.digest(artifacts / "selection.json")
    return selection, inputs, hashes


def run_harness(harness, model_directory, cases_path, report_path, log_path):
    # Foundation's atomic writes on the external worktree are denied in this sandbox.
    with tempfile.TemporaryDirectory(prefix="rank-s2-coreml-report-", dir="/private/tmp") as temporary:
        temporary_report = Path(temporary) / "report.json"
        command = [str(harness.resolve()), str(model_directory), str(cases_path.resolve()), str(temporary_report)]
        with log_path.open("xb") as log:
            done = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT)
        if done.returncode:
            raise ValueError(f"Core ML exited {done.returncode}; see {log_path}")
        with report_path.open("xb") as stream:
            stream.write(temporary_report.read_bytes())
    return json.loads(report_path.read_text())


def score_all(args):
    _, inputs, hashes = load_inputs(args.artifacts)
    args.root.mkdir(parents=True, exist_ok=True)
    gate.write_json(args.root / "score-inputs.json", dict(files=hashes, model_directory=str(args.model_dir),
                    harness_sha256=gate.digest(args.harness),
                    scorer_source_sha256=gate.digest("native/macos/Sources/AgentInputMethodHostKit/ContextRankingModel.swift")))
    for phase in ("test", "tune"):
        for bucket in gate.BUCKETS:
            _, pool, _ = inputs[phase, bucket]
            for n in gate.LIMITS:
                directory = args.root / f"{phase}-{bucket}-n{n}"
                directory.mkdir()
                cases = prepare_cases(pool, n)
                gate.write_json(directory / "cases.json", cases)
                gate.write_jsonl(directory / "case-index.jsonl", [dict(id=r["id"], context=r["context"],
                                      candidates=r["texts"][:n]) for r in pool if r["texts"]])
                report = run_harness(args.harness, args.model_dir, directory / "cases.json",
                                     directory / "harness-report.json", directory / "harness.log")
                scored = attach_coreml(pool, n, report)
                gate.write_jsonl(directory / "scored.jsonl", scored)
                print(f"{phase}/{bucket}/{n}: {len(scored)} rows, {len(cases)} nonempty pools", flush=True)
    gate.write_json(args.root / "score-complete.json", dict(groups=16, input_files=hashes))


def verify_onnx_summary(summary, historical):
    if json.loads(json.dumps(summary)) != {k: historical[k] for k in summary}:
        raise ValueError("recomputed ONNX summary differs from S1")


def analyze_all(args):
    selection, inputs, hashes = load_inputs(args.artifacts)
    if json.loads((args.root / "score-complete.json").read_text())["input_files"] != hashes:
        raise ValueError("inputs changed since Core ML scoring")
    old_summary = json.loads((args.artifacts / "test-run/summary.json").read_text())
    old_results = {(r["bucket"], r["n"], r["mode"]): r for r in old_summary["results"]}
    results, comparisons, references, switches, different_heads = [], [], [], [], []
    diagnostics = []
    for phase in ("test", "tune"):
        for bucket in gate.BUCKETS:
            current, pool, cases = inputs[phase, bucket]
            for n in gate.LIMITS:
                directory = args.root / f"{phase}-{bucket}-n{n}"
                scored = gate.read_jsonl(directory / "scored.jsonl")
                validate_pairs(current, scored)
                if any(r["texts"] != p["texts"][:n] or r["scores"] != p["scores"][:n]
                       for r, p in zip(scored, pool)):
                    raise ValueError("scored candidates or ONNX scores changed")
                threshold = selection["choices"][f"{bucket}/{n}"]
                comparison, details = compare(current, scored, threshold)
                comparisons.append(dict(phase=phase, bucket=bucket, n=n, **comparison))
                gate.write_jsonl(directory / "comparison.jsonl", details)
                if phase == "tune":
                    references.append(dict(bucket=bucket, n=n, frozen_threshold=threshold,
                                           **reference_threshold(current, scored, n)))
                    continue
                for detail, old, row in zip(details, current, scored):
                    if detail["direction"] != "same":
                        item = dict(bucket=bucket, n=n, **detail, raw=old["raw"], context=old["context"],
                                    target=old["target"], texts=row["texts"], scores=row["scores"],
                                    coreml_scores=row["coreml_scores"])
                        (different_heads if detail["direction"] == "both_different" else switches).append(item)
                for backend in ("onnx", "coreml"):
                    summaries, outputs = replay(current, scored, n, threshold, backend)
                    for summary in summaries:
                        if backend == "onnx":
                            historical = old_results[bucket, n, summary["mode"]]
                            verify_onnx_summary(summary, historical)
                        results.append(dict(backend=backend, bucket=bucket, n=n, **summary))
                        gate.write_jsonl(directory / f"{backend}-{summary['mode']}.jsonl",
                                         [dict(id=r["id"], candidates=out) for r, out in zip(current, outputs[summary["mode"]])])
                    if bucket == "full":
                        for origin in ("full", "short", "erhua"):
                            indices = [i for i, c in enumerate(cases) if c["origin_bucket"] == origin]
                            diagnostic = gate.summarize([current[i] for i in indices],
                                                       [outputs["gate_model"][i] for i in indices])
                            diagnostics.append(dict(backend=backend, n=n, origin=origin, **diagnostic))
                print(f"analyzed {phase}/{bucket}/{n}", flush=True)
    gate.write_jsonl(args.root / "test-switches.jsonl", switches)
    gate.write_jsonl(args.root / "test-both-swapped-different-head.jsonl", different_heads)
    summary = dict(selection_sha256=gate.digest(args.artifacts / "selection.json"), choices=selection["choices"],
                   comparisons=comparisons, results=results, tune_reference=references,
                   switches=len(switches), different_heads=len(different_heads), full_diagnostics=diagnostics,
                   onnx_s1_summaries_exact_match=32, files=hashes)
    gate.write_json(args.root / "summary.json", summary)
    print(json.dumps(dict(onnx_s1_summaries_exact_match=32, switches=len(switches),
                          both_swapped_different_head=len(different_heads)), ensure_ascii=False), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("score", "analyze"))
    parser.add_argument("--artifacts", type=Path, default=ARTIFACTS)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--harness", type=Path)
    parser.add_argument("--model-dir", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "score":
            if args.harness is None or args.model_dir is None:
                parser.error("score requires --harness and --model-dir")
            score_all(args)
        else:
            analyze_all(args)
    except (OSError, ValueError, KeyError) as exc:
        print(f"Core ML 复核失败：{exc}", file=sys.stderr)
        raise SystemExit(2)


class CoreMLRecheckTests(unittest.TestCase):
    def row(self, name="fixture", scores=None, coreml=None, context="上文", candidates=None):
        texts = ["甲", "乙", "丙"]
        old = dict(id=name, context=context, candidates=candidates or texts, target="乙")
        pool = dict(id=name, context=context, texts=texts,
                    scores=scores if scores is not None else [-4.0, -2.0, -3.0],
                    coreml_scores=coreml if coreml is not None else [-4.0, -2.0, -3.0])
        return old, pool

    def test_distribution_uses_linear_quantiles_and_empty_denominator(self):
        result = distribution([0, 1, 2, 10])
        self.assertEqual(result["count"], 4)
        self.assertEqual(result["median"], 1.5)
        self.assertAlmostEqual(result["p95"], 8.8)
        self.assertEqual(result["max"], 10)
        self.assertEqual(distribution([]), dict(count=0, median=None, p95=None, max=None))

    def test_cases_keep_prefix_order_context_and_separate_n_windows(self):
        texts = [chr(0x4e00 + i) for i in range(39)] + ["末" * 39]
        pool = [dict(id="long", context="上文" * 30, texts=texts, scores=[-1.0] * 40),
                dict(id="empty", context="空", texts=[], scores=[])]
        before = copy.deepcopy(pool)
        self.assertEqual(prepare_cases(pool, 20), [dict(context=pool[0]["context"], candidates=texts[:20])])
        self.assertEqual(prepare_cases(pool, 40), [dict(context=pool[0]["context"], candidates=texts)])
        self.assertEqual(pool, before)

    def test_attach_skips_empty_pools_without_shifting_scores(self):
        pool = [dict(id="empty", context="", texts=[], scores=[]),
                dict(id="a", context="上文", texts=["乙", "甲"], scores=[-3, -4]),
                dict(id="b", context="", texts=["丙"], scores=[-5])]
        result = attach_coreml(pool, 20, dict(rows=[dict(scores=[-2.5, -4.1]), dict(scores=[-5.2])]))
        self.assertEqual([r["coreml_scores"] for r in result], [[], [-2.5, -4.1], [-5.2]])
        self.assertEqual([r["texts"] for r in result], [[], ["乙", "甲"], ["丙"]])
        self.assertEqual(result[1]["scores"], [-3, -4])
        self.assertNotIn("coreml_scores", pool[0])

    def test_attach_rejects_missing_extra_nonfinite_and_wrong_width_reports(self):
        _, pool = self.row()
        for rows in ([], [dict(scores=[1, 2])], [dict(scores=[1, 2, math.nan])],
                     [dict(scores=[1, 2, 3]), dict(scores=[1, 2, 3])]):
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                attach_coreml([pool], 20, dict(rows=rows))
        with self.assertRaises(ValueError):
            prepare_cases([dict(pool, texts=["甲", "甲", "乙"])], 20)

    def test_compare_separates_score_offset_from_gate_margin(self):
        old, pool = self.row(scores=[-5, -1, -3], coreml=[-4.5, -.5, -3])
        summary, details = compare([old], [pool], 1)
        self.assertEqual(summary["max_abs_score_delta"]["median"], .5)
        self.assertEqual(summary["onnx_margin"]["median"], 4)
        self.assertEqual(summary["coreml_margin"]["median"], 4)
        self.assertEqual(summary["margin_delta"]["median"], 0)
        self.assertEqual(summary["abs_margin_delta"]["max"], 0)
        self.assertEqual(summary["same_best"], 1)
        self.assertEqual(details[0]["onnx_head"], "乙")

    def test_switches_use_strict_threshold_in_both_directions(self):
        old1, pool1 = self.row("coreml-only", scores=[-4, -3, -4], coreml=[-4, -2.9, -4])
        old2, pool2 = self.row("onnx-only", scores=[-4, -2.9, -4], coreml=[-4, -3, -4])
        old3, pool3 = self.row("new-winner", scores=[-4, -2, -3], coreml=[-4, -3, -2])
        summary, details = compare([old1, old2, old3], [pool1, pool2, pool3], 1)
        self.assertEqual(summary["coreml_only_swap"], 1)
        self.assertEqual(summary["onnx_only_swap"], 1)
        self.assertEqual(summary["both_swap_different_head"], 1)
        self.assertEqual(summary["same_best"], 2)
        self.assertEqual([r["direction"] for r in details], ["coreml_only", "onnx_only", "both_different"])

    def test_guards_and_missing_head_do_not_count_as_threshold_switches(self):
        rows = []
        for name, context, candidates in (("empty-context", "", ["甲", "乙"]),
                                           ("latin-head", "上文", ["AI", "甲"]),
                                           ("missing-head", "上文", ["丁", "甲"])):
            rows.append(self.row(name, scores=[-4, -2, -3], coreml=[-4, -3, -4],
                                 context=context, candidates=candidates))
        summary, details = compare([r[0] for r in rows], [r[1] for r in rows], 1)
        self.assertEqual(summary["onnx_only_swap"], 0)
        self.assertEqual(summary["coreml_only_swap"], 0)
        self.assertTrue(all(not r["eligible"] for r in details))
        old, pool = self.row()
        summary, _ = compare([old], [pool], None)
        self.assertEqual(summary["onnx_only_swap"] + summary["coreml_only_swap"], 0)

    def test_empty_pool_has_no_best_or_margin_denominator(self):
        old, pool = self.row()
        pool = dict(pool, texts=[], scores=[], coreml_scores=[])
        summary, details = compare([old], [pool], 1)
        self.assertEqual(summary["total"], 1)
        self.assertEqual(summary["nonempty"], 0)
        self.assertEqual(summary["margin_delta"]["count"], 0)
        self.assertIsNone(details[0]["max_abs_score_delta"])

    def test_pairing_and_finite_scores_are_checked(self):
        old, pool = self.row()
        for bad in (dict(pool, id="other"), dict(pool, context="别的"),
                    dict(pool, coreml_scores=[1]), dict(pool, coreml_scores=[1, 2, math.inf])):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                compare([old], [bad], 1)
        with self.assertRaises(ValueError):
            compare([old, old], [pool, pool], 1)

    def test_replay_wires_coreml_scores_into_unchanged_arrange_and_evaluate(self):
        old, pool = self.row(scores=[-4, -2, -3], coreml=[-4, -3.5, -3.8])
        before = copy.deepcopy(pool)
        results, outputs = replay([old], [pool], 20, 1, "coreml")
        converted = [dict(pool, scores=pool["coreml_scores"])]
        for result in results:
            mode = result["mode"]
            threshold = 1 if mode.startswith("gate_") else None
            expected, expected_outputs = gate.evaluate([old], converted, 20, threshold,
                                                       mode in ("fixed_model", "gate_model"), mode == "current")
            self.assertEqual(outputs[mode], expected_outputs)
            self.assertEqual({k: result[k] for k in expected}, expected)
        self.assertEqual(outputs["gate_current"][0][0], "甲")
        self.assertEqual(replay([old], [pool], 20, 1, "onnx")[1]["gate_current"][0][0], "乙")
        self.assertEqual(pool, before)

    def test_reference_tuning_is_separate_and_uses_s1_tie_break(self):
        old, pool = self.row(scores=[-10, -1, -3], coreml=[-10, -1, -3])
        correct, correct_pool = self.row("correct", candidates=["乙", "甲", "丙"])
        selection = dict(choices={"full/20": 1})
        before = copy.deepcopy(selection)
        reference = reference_threshold([old, correct], [pool, correct_pool], 20)
        self.assertEqual(reference["threshold"], 8)
        self.assertEqual(set(reference["trials"]), set(gate.THRESHOLDS))
        self.assertEqual(selection, before)
        self.assertIsNone(reference_threshold([old], [pool], 20)["threshold"])

    def test_saved_s1_summary_accepts_json_intervals_but_rejects_metric_changes(self):
        old, pool = self.row()
        summary = replay([old], [pool], 20, 1, "onnx")[0][0]
        saved = dict(json.loads(json.dumps(summary)), bucket="full", n=20)
        verify_onnx_summary(summary, saved)
        saved["top1"]["count"] += 1
        with self.assertRaises(ValueError):
            verify_onnx_summary(summary, saved)

    def test_harness_stages_atomic_output_and_archives_it_in_task_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            seen = []

            def fake(command, **kwargs):
                destination = Path(command[3])
                self.assertTrue(str(destination).startswith("/private/tmp/"))
                self.assertEqual(command[1], "/opaque-model-directory")
                self.assertEqual(command[2], str((root / "cases.json").resolve()))
                seen.append(destination)
                destination.write_text(json.dumps(dict(rows=[dict(scores=[-1.25])])))
                return subprocess.CompletedProcess(command, 0)

            with mock.patch.object(subprocess, "run", side_effect=fake):
                report = run_harness(root / "harness", Path("/opaque-model-directory"),
                                     root / "cases.json", root / "report.json", root / "harness.log")
            self.assertEqual(report, dict(rows=[dict(scores=[-1.25])]))
            self.assertEqual(json.loads((root / "report.json").read_text()), report)
            self.assertFalse(seen[0].exists())


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "self-test":
        unittest.main(argv=[sys.argv[0], *sys.argv[2:]])
    else:
        main()
