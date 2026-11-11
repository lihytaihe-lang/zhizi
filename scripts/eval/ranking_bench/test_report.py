"""Synthetic-only ranking bench checks; no model files or user data are read."""
import contextlib
import copy
import io
import json
import tempfile
import unittest
from pathlib import Path

from scripts.eval.ranking_bench import report


def record(case_id, bucket="full", rank=0, ready=True):
    candidates = [f"候选{i}" for i in range(min(max(rank + 1, 1), 60))]
    if 0 <= rank < 60:
        candidates[rank] = "目标"
    return {
        "id": case_id, "bucket": bucket, "raw": "ceshi", "context": "合成上文",
        "target": "目标", "candidates": candidates, "rank_all": rank,
        "model_ready": ready, "model_ms": 12.5, "source": "ext",
    }


class ReportTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)

    def write_rows(self, name, rows):
        path = self.root / name
        path.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows), encoding="utf-8")
        return path

    def test_bucket_metrics_and_default_weighting(self):
        rows = [record("a", "full", 0), record("b", "full", 4),
                record("c", "abbrev", 1), record("d", "mixed", -1),
                record("e", "typo", 5), record("f", "english", 0),
                record("g", "name", 60)]
        summary = report.summarize(rows)
        full = summary["buckets"]["full"]
        self.assertEqual(full["count"], 2)
        self.assertEqual(full["top1"], 0.5)
        self.assertEqual(full["top5"], 1)
        self.assertAlmostEqual(full["mrr"], 0.6)
        self.assertEqual(summary["buckets"]["mixed"]["pool_rate"], 0)
        self.assertEqual(summary["buckets"]["name"]["pool_rate"], 1)
        self.assertEqual(summary["buckets"]["name"]["mrr"], 1 / 61)
        self.assertAlmostEqual(summary["weighted"]["top1"], 0.275)
        self.assertAlmostEqual(summary["weighted"]["top5"], 0.775)
        self.assertAlmostEqual(summary["weighted"]["mrr"], 0.3 + 0.125 + 0.1 / 6 + 0.025 + 0.025 / 61)
        self.assertAlmostEqual(summary["weighted"]["pool_rate"], 0.9)
        self.assertEqual(summary["weight_coverage"], 1)
        self.assertEqual(summary["missing_buckets"], [])

    def test_missing_buckets_are_visible_and_weights_normalize(self):
        summary = report.summarize([record("a", "full"), record("b", "abbrev", -1)])
        self.assertAlmostEqual(summary["weight_coverage"], 0.75)
        self.assertAlmostEqual(summary["weighted"]["top1"], 2 / 3)
        self.assertEqual(summary["missing_buckets"], ["mixed", "typo", "english", "name"])
        weights = {bucket: 0 for bucket in report.BUCKETS}
        weights.update(full=1, abbrev=3)
        self.assertEqual(report.summarize([record("a"), record("b", "abbrev", -1)], weights)["weighted"]["top1"], 0.25)
        self.assertIn("缺少桶", report.render_report(summary))

    def test_invalid_weights_are_rejected(self):
        for value in ({"full": 1}, dict.fromkeys(report.BUCKETS, 0),
                      dict.fromkeys(report.BUCKETS, -1), dict.fromkeys(report.BUCKETS, float("nan"))):
            with self.subTest(value=value), self.assertRaises(ValueError):
                report.summarize([record("a")], value)

    def test_comparison_pairs_by_id_and_counts_top1_changes(self):
        before = [record("fixed", "abbrev", 5), record("broken", "full", 0),
                  record("same", "typo", 3)]
        after = [record("same", "typo", 1), record("broken", "full", 2),
                 record("fixed", "abbrev", 0)]
        result = report.compare_records(before, after)
        self.assertEqual((result["total"], result["fixed"], result["broken"], result["unchanged"]), (3, 1, 1, 1))
        self.assertEqual(result["by_bucket"]["abbrev"]["fixed"], 1)
        self.assertEqual(result["by_bucket"]["full"]["broken"], 1)
        self.assertIn("修好 1 / 弄坏 1", report.render_report(report.summarize(after), result))

    def test_comparison_rejects_changed_case_definition_or_population(self):
        before = [record("a")]
        with self.assertRaises(ValueError):
            report.compare_records(before, [])
        for field, value in (("bucket", "name"), ("raw", "other"), ("context", "changed"),
                             ("target", "changed"), ("source", "friend")):
            after = copy.deepcopy(before)
            after[0][field] = value
            if field == "target":
                after[0]["candidates"][0] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                report.compare_records(before, after)

    def test_readiness_gate_includes_exact_98_percent_boundary(self):
        rows = [record(str(i), ready=i < 98) for i in range(100)]
        readiness = report.model_readiness(rows)
        self.assertEqual(readiness, {"total": 100, "ready": 98, "rate": 0.98,
                                     "no_task": 0, "no_task_ratio": 0.0, "passed": True})
        self.assertEqual(report.report_exit_code(report.summarize(rows)), 0)
        rows[97]["model_ready"] = False
        summary = report.summarize(rows)
        self.assertEqual(report.report_exit_code(summary), 1)
        first = report.render_report(summary).splitlines()[0]
        self.assertTrue(first.startswith("\x1b[31mFAIL"), first)
        self.assertIn("97.00%", first)
        self.assertEqual(report.report_exit_code(summary, require_model=False), 0)
        self.assertNotIn("FAIL", report.render_report(summary, require_model=False).splitlines()[0])

    def test_no_group_without_scoring_task_does_not_lower_readiness(self):
        rows = [record(str(i)) for i in range(50)]
        rows.extend(dict(record(f"ng{i}", ready=False), outcome="no_group") for i in range(50))
        readiness = report.model_readiness(rows)
        self.assertEqual(readiness, {"total": 50, "ready": 50, "rate": 1,
                                     "no_task": 50, "no_task_ratio": 0.5, "passed": True})
        self.assertEqual(report.report_exit_code(report.summarize(rows)), 0)
        first = report.render_report(report.summarize(rows)).splitlines()[0]
        self.assertIn("100.00%", first)
        self.assertIn("无评分任务 50 条（50.00%）", first)

    def test_no_group_with_ready_model_still_counts_as_ready(self):
        rows = [dict(record("a"), outcome="no_group")]
        readiness = report.model_readiness(rows)
        self.assertEqual((readiness["total"], readiness["ready"], readiness["no_task"]), (1, 1, 0))
        self.assertTrue(readiness["passed"])

    def test_pending_and_unavailable_outcomes_count_as_not_ready(self):
        rows = [record("a"),
                dict(record("b", ready=False), outcome="pending"),
                dict(record("c", ready=False), outcome="unavailable")]
        readiness = report.model_readiness(rows)
        self.assertEqual((readiness["total"], readiness["ready"], readiness["no_task"]), (3, 1, 0))
        self.assertFalse(readiness["passed"])
        self.assertEqual(report.report_exit_code(report.summarize(rows)), 1)
        self.assertIn("33.33%", report.render_report(report.summarize(rows)).splitlines()[0])

    def test_98_percent_boundary_applies_to_scored_subset_only(self):
        rows = [record(str(i)) for i in range(49)]
        rows.append(dict(record("miss", ready=False), outcome="composed"))
        rows.extend(dict(record(f"ng{i}", ready=False), outcome="no_group") for i in range(50))
        readiness = report.model_readiness(rows)
        self.assertEqual((readiness["total"], readiness["ready"], readiness["no_task"]), (50, 49, 50))
        self.assertTrue(readiness["passed"])
        self.assertEqual(report.report_exit_code(report.summarize(rows)), 0)
        rows[0]["model_ready"] = False
        self.assertFalse(report.model_readiness(rows)["passed"])
        self.assertEqual(report.report_exit_code(report.summarize(rows)), 1)

    def test_legacy_rows_without_outcome_keep_strict_denominator(self):
        rows = [record(str(i), ready=i < 97) for i in range(100)]
        readiness = report.model_readiness(rows)
        self.assertEqual((readiness["total"], readiness["ready"], readiness["no_task"]), (100, 97, 0))
        self.assertFalse(readiness["passed"])
        self.assertEqual(report.report_exit_code(report.summarize(rows)), 1)
        rows.append(dict(record("extra", ready=False), outcome="no_group"))
        self.assertFalse(report.model_readiness(rows)["passed"])
        legacy = [record(str(i), ready=i < 98) for i in range(100)]
        self.assertTrue(report.model_readiness(legacy)["passed"])
        self.assertEqual(report.model_readiness(legacy)["no_task"], 0)

    def test_empty_input_cannot_pass(self):
        summary = report.summarize([])
        self.assertFalse(summary["model_readiness"]["passed"])
        self.assertEqual(summary["model_readiness"]["rate"], 0)
        self.assertEqual(report.report_exit_code(summary), 1)
        self.assertEqual(report.report_exit_code(summary, require_model=False), 1)
        self.assertIsNone(summary["weighted"]["top1"])

    def test_load_round_trip_and_rank_outside_top60(self):
        rows = [record("a", rank=70)]
        self.assertEqual(report.load_records(self.write_rows("ok.jsonl", rows)), rows)

    def test_load_rejects_malformed_duplicate_and_inconsistent_fields(self):
        for field, value in (("bucket", "unknown"), ("source", "owner"), ("id", ""),
                             ("rank_all", True), ("rank_all", -2), ("model_ready", 1),
                             ("model_ms", float("inf")), ("model_ms", -1),
                             ("candidates", ["候选"] * 61), ("candidates", ["不匹配"]),
                             ("context", None)):
            rows = [record("a")]
            rows[0][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                report.load_records(self.write_rows("bad.jsonl", rows))
        with self.assertRaises(ValueError):
            report.load_records(self.write_rows("duplicate.jsonl", [record("a"), record("a")]))
        path = self.root / "invalid.jsonl"
        path.write_text("{oops}\n", encoding="utf-8")
        with self.assertRaises(ValueError):
            report.load_records(path)

    def test_cli_prints_red_first_line_and_nonzero_for_low_readiness(self):
        path = self.write_rows("low.jsonl", [record("a", ready=False)])
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            status = report.main([str(path)])
        self.assertEqual(status, 1)
        self.assertTrue(stdout.getvalue().splitlines()[0].startswith("\x1b[31mFAIL"))

    def test_cli_custom_weights_and_pool_mode(self):
        path = self.write_rows("pool.jsonl", [record("a", ready=False)])
        weights = self.root / "weights.json"
        weights.write_text(json.dumps(dict.fromkeys(report.BUCKETS, 1)), encoding="utf-8")
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            status = report.main([str(path), "--weights", str(weights), "--no-model"])
        self.assertEqual(status, 0)
        self.assertIn("模型就绪门不适用", stdout.getvalue().splitlines()[0])

    def test_nullable_model_ms_is_explicit_and_required(self):
        row = dict(record("a", ready=False), model_ms=None)
        self.assertIsNone(report.load_records(self.write_rows("nullable.jsonl", [row]))[0]["model_ms"])
        del row["model_ms"]
        with self.assertRaises(ValueError):
            report.load_records(self.write_rows("missing-ms.jsonl", [row]))

    def test_comparison_rejects_unready_baseline_even_if_current_is_ready(self):
        before, after = [record("a", ready=False)], [record("a")]
        comparison = report.compare_records(before, after)
        summary = report.summarize(after)
        self.assertEqual(report.report_exit_code(summary, comparison=comparison), 1)
        self.assertTrue(report.render_report(summary, comparison).startswith("\x1b[31mFAIL"))
        self.assertEqual(report.report_exit_code(summary, require_model=False, comparison=comparison), 0)

    def test_samples_with_no_positive_weight_fail(self):
        weights = dict.fromkeys(report.BUCKETS, 0)
        weights["abbrev"] = 1
        with self.assertRaises(ValueError):
            report.summarize([record("a", "full")], weights)


class RunnerTests(unittest.TestCase):
    def setUp(self):
        from scripts.eval.ranking_bench import run_bench
        self.bench = run_bench
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.blank = self.root / "blank"
        self.blank.mkdir()
        (self.blank / "seed").write_text("blank fixture", encoding="utf-8")
        self.shared = self.root / "shared"
        self.shared.mkdir()

    def arguments(self, rows, extra=()):
        cases = self.root / "cases.jsonl"
        cases.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows), encoding="utf-8")
        return self.bench.build_parser().parse_args([
            "--cases", str(cases), "--shared-dir", str(self.shared),
            "--blank-user-dir", str(self.blank), "--work-dir", str(self.root / "run"),
            "--pool-bin", "/synthetic/pool", "--replay-bin", "/synthetic/replay",
            "--ort", "/synthetic/runtime", "--model", "/synthetic/model",
            "--itos", "/synthetic/itos", *extra,
        ])

    def engine(self, calls, ready=True, drop=False):
        import subprocess

        def fake(command, **kwargs):
            user = Path(command[2])
            self.assertEqual((user / "seed").read_text(encoding="utf-8"), "blank fixture")
            (user / "seed").write_text("changed by this process", encoding="utf-8")
            calls.append((command, kwargs["env"].copy()))
            events = [json.loads(line) for line in Path(command[3]).read_text(encoding="utf-8").splitlines()]
            output = [] if drop else [{
                "raw": event["pinyin"], "context": event["precedingText"],
                "chosen": event["selectedText"], "candidates": [event["selectedText"]],
                "rank_all": 0, "model_ready": ready and command[0].endswith("replay"),
                "model_ms": 2.5 if command[0].endswith("replay") else 0,
            } for event in events]
            Path(command[4]).write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in output), encoding="utf-8")
            return subprocess.CompletedProcess(command, 0)

        return fake

    def test_parallel_copies_env_and_duplicate_case_ids_survive(self):
        from unittest.mock import patch
        rows = [record(str(i), bucket, ready=False) for i, bucket in enumerate(report.BUCKETS)]
        rows[2]["raw"] = "nh"
        rows[3]["context"] = "另一条上文"
        rows[4]["target"] = "英语"
        rows[5]["target"] = "姓名"
        calls = []
        with patch.dict("os.environ", {"ZHIZI_MIN_KEYS": "4"}, clear=True):
            args = self.arguments(rows, ["--jobs", "3"])
            with patch.object(self.bench.subprocess, "run", self.engine(calls)), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(self.bench.run_bench(args), 0)
        self.assertEqual(len(calls), 6)
        users = [command[2] for command, _ in calls]
        self.assertEqual(len(users), len(set(users)))
        self.assertEqual((self.blank / "seed").read_text(encoding="utf-8"), "blank fixture")
        for command, env in calls:
            self.assertEqual(env["ZHIZI_MIN_KEYS"], "2")
            self.assertEqual(env["ZHIZI_WAIT_MS"], "3000")
            if command[0].endswith("pool"):
                self.assertNotIn("ZHIZI_MODEL", env)
            else:
                self.assertEqual(env["ZHIZI_MODEL"], "/synthetic/model")
        for mode in ("pool", "replay"):
            result = report.load_records(self.root / "run" / f"{mode}.jsonl")
            self.assertEqual([row["id"] for row in result], [row["id"] for row in rows])
            self.assertEqual([row["bucket"] for row in result], list(report.BUCKETS))

    def test_low_readiness_preserves_artifact_and_fails_red_first_line(self):
        from unittest.mock import patch
        args = self.arguments([record("one")], ["--mode", "replay"])
        stdout = io.StringIO()
        with patch.object(self.bench.subprocess, "run", self.engine([], ready=False)), contextlib.redirect_stdout(stdout):
            self.assertEqual(self.bench.run_bench(args), 1)
        self.assertTrue(stdout.getvalue().splitlines()[0].startswith("\x1b[31mFAIL"))
        self.assertTrue((self.root / "run" / "replay.jsonl").is_file())
        self.assertTrue((self.root / "run" / "replay.report.txt").read_text(encoding="utf-8").startswith("\x1b[31mFAIL"))

    def test_missing_engine_rows_fail_instead_of_shrinking_denominator(self):
        from unittest.mock import patch
        args = self.arguments([record("one")], ["--mode", "pool"])
        with patch.object(self.bench.subprocess, "run", self.engine([], drop=True)), self.assertRaises(ValueError):
            self.bench.run_bench(args)
        self.assertFalse((self.root / "run" / "pool.jsonl").exists())

    def test_input_validation_rejects_filters_empty_and_duplicate_ids(self):
        for rows in ([], [record("same"), record("same")],
                     [dict(record("one"), raw="a")], [dict(record("one"), raw="Hello")],
                     [dict(record("one"), raw="ni'hao")], [dict(record("one"), bucket="short")]):
            args = self.arguments(rows)
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                self.bench.load_cases(args.cases)

    def test_jobs_and_wait_precedence(self):
        from unittest.mock import patch
        with patch.dict("os.environ", {"ZHIZI_WAIT_MS": "4500"}, clear=True):
            self.assertEqual(self.arguments([record("one")]).wait_ms, 4500)
            self.assertEqual(self.arguments([record("one")], ["--wait-ms", "25"]).wait_ms, 25)
        for jobs in ("0", "4"):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                self.arguments([record("one")], ["--jobs", jobs])

    def test_existing_work_dir_is_rejected_before_spawning(self):
        from unittest.mock import patch
        args = self.arguments([record("one")])
        args.work_dir.mkdir()
        with patch.object(self.bench.subprocess, "run") as process, self.assertRaises(FileExistsError):
            self.bench.run_bench(args)
        process.assert_not_called()


if __name__ == "__main__":
    unittest.main()
