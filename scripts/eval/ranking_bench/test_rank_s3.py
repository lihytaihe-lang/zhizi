"""Synthetic checks for the independent sets and frozen-only evaluator."""
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

import build_independent_set as build
import frozen_eval as frozen
import margin_gate as gate


class FrozenTests(unittest.TestCase):
    def selection(self):
        return dict(choices={f"{b}/{n}": 1 for b in gate.BUCKETS for n in gate.LIMITS})

    def test_frozen_choices_are_read_without_selection_or_defaults(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "selection.json"
            selection = self.selection()
            selection["choices"]["abbrev/40"] = 2
            selection["choices"]["mixed/20"] = None
            path.write_text(json.dumps(selection))
            before = path.read_bytes()
            with mock.patch.object(gate, "select_threshold", side_effect=AssertionError("must not tune")):
                self.assertEqual(frozen.load_selection(path), selection)
            self.assertEqual(path.read_bytes(), before)
            del selection["choices"]["full/20"]
            path.write_text(json.dumps(selection))
            with self.assertRaisesRegex(ValueError, "full/20"):
                frozen.load_selection(path)

    def test_invalid_frozen_threshold_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "selection.json"
            for bad in (True, "1", -1, float("nan")):
                selection = self.selection()
                selection["choices"]["full/20"] = bad
                path.write_text(json.dumps(selection))
                with self.assertRaises(ValueError):
                    frozen.load_selection(path)

    def test_external_fields_and_protection_buckets_are_preserved(self):
        row = dict(id="external", bucket="name", raw="liming", context="今天见到了",
                   target="李明", source="local", extra=dict(event=17))
        actual = frozen.normalize_cases([row])[0]
        self.assertEqual((actual["bucket"], actual["sub"], actual["src"]), ("full", "name", "local"))
        self.assertEqual(actual["extra"], row["extra"])
        self.assertEqual(actual["source"], "local")
        self.assertEqual(row["bucket"], "name")
        repeated_event = dict(row, id="another-event")
        self.assertEqual(len(frozen.normalize_cases([row, repeated_event])), 2)
        for changed in (dict(raw="LiMing"), dict(context="中" * 33), dict(bucket="other")):
            with self.assertRaises(ValueError):
                frozen.normalize_cases([dict(row, **changed)])
        with self.assertRaises(ValueError):
            frozen.normalize_cases([row, row])

    def fixture(self):
        current = [dict(id="name", bucket="full", sub="name", src="asap", source="ext",
                        raw="liming", context="今天见到了", target="李明", candidates=["李明", "黎明"]),
                   dict(id="english", bucket="full", sub="english", src="wiki", source="ext",
                        raw="api", context="这里使用", target="API", candidates=["API", "阿皮"]),
                   dict(id="short", bucket="full", sub="short", src="asap", source="ext",
                        raw="zai", context="他还没有", target="在", candidates=["再", "在"])]
        pool = [dict(id=r["id"], texts=r["candidates"], scores=[-5., 0.]) for r in current]
        return current, pool

    def test_strata_wilson_and_new_protection_errors(self):
        current, pool = self.fixture()
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(gate, "select_threshold", side_effect=AssertionError("must not tune")):
                results, errors, _ = frozen.evaluate_rows(current, pool, self.selection(), Path(directory))
        chosen = [r for r in results if r["n"] == 20 and r["mode"] == "gate_model"]
        strata = {(r["sub"], r["src"]): r for r in chosen if r["level"] == "stratum"}
        self.assertEqual(set(strata), {("name", "asap"), ("english", "wiki"), ("short", "asap")})
        self.assertEqual(strata["name", "asap"]["new_errors"], 1)
        self.assertEqual(strata["english", "wiki"]["new_errors"], 0)
        self.assertEqual(strata["short", "asap"]["fixed"], 1)
        aggregate = next(r for r in chosen if r["level"] == "bucket")
        self.assertEqual(aggregate["broken_original"]["denominator"], 2)
        self.assertEqual(aggregate["broken_original"]["ci95"], gate.wilson(1, 2))
        self.assertAlmostEqual(aggregate["mrr"]["value"], 5 / 6)
        self.assertEqual({r["id"] for r in errors}, {"name"})
        self.assertEqual(len(errors), 4)  # Two N values, two gated arrangements.
        self.assertEqual(gate.wilson(0, 0), (None, None))
        self.assertAlmostEqual(gate.wilson(0, 381)[1], .009981925628, places=12)

    def test_missing_baseline_and_bad_pool_fail_closed(self):
        current, pool = self.fixture()
        with tempfile.TemporaryDirectory() as directory:
            for bad in (pool[:-1], [dict(pool[0], scores=[0.]), *pool[1:]],
                        [dict(pool[0], scores=[0., float("nan")]), *pool[1:]]):
                with self.assertRaises(ValueError):
                    frozen.evaluate_rows(current, bad, self.selection(), Path(directory))

    def test_required_originally_correct_sample_estimate(self):
        estimate = frozen.required_correct(0, 100)
        self.assertEqual(estimate["required_total"], 381)
        self.assertEqual(estimate["additional"], 281)
        self.assertIsNone(frozen.required_correct(1, 100)["required_total"])
        self.assertIsNone(frozen.required_correct(0, 0)["required_total"])
        estimate = frozen.required_correct(1, 1000)
        n = estimate["required_total"]
        self.assertLessEqual(gate.wilson(.001 * n, n)[1], .01)
        self.assertGreater(gate.wilson(.001 * (n - 1), n - 1)[1], .01)

    def test_existing_output_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(FileExistsError):
                frozen.new_output(Path(directory))

    def test_current_precedes_pool_and_changed_model_is_rejected(self):
        current, pool = self.fixture()
        identity = dict(model={key: "synthetic" for key in ("ZHIZI_MODEL", "ZHIZI_ITOS", "ZHIZI_ORT")},
                        code={"native/host/examples/slot_rules.rs": "synthetic"})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            gate.write_jsonl(root / "cases.jsonl", [{k: v for k, v in current[0].items() if k != "candidates"}])
            selection = dict(self.selection(), identity=identity)
            gate.write_json(root / "selection.json", selection)
            (root / "binary").write_text("synthetic executable identity")
            args = SimpleNamespace(cases=root / "cases.jsonl", selection=root / "selection.json",
                                   binary=root / "binary", out=root / "out", shared=root, jobs=1)
            order = []

            def engine(binary, shared, output, cases_path, bucket, mode, baseline=None):
                order.append(mode)
                if mode == "pool":
                    self.assertEqual(order, ["current", "pool"])
                    self.assertEqual(baseline, output / "current-full/rows.jsonl")
                return current[:1] if mode == "current" else pool[:1]

            changed = dict(identity, model=dict(identity["model"], ZHIZI_MODEL="changed"))
            with mock.patch.object(gate, "identity", side_effect=[identity, changed]), mock.patch.object(gate, "run_engine", side_effect=engine):
                with self.assertRaisesRegex(ValueError, "identity changed"):
                    frozen.run(args)
            self.assertEqual(order, ["current", "pool"])
            self.assertFalse((args.out / "summary.json").exists())

    def test_each_n_uses_its_own_frozen_choice(self):
        current = [dict(id="abbrev", bucket="abbrev", sub="abbrev", src="wiki", source="ext",
                        raw="cs", context="这是一次", target="测试", candidates=["测试", "侧时"])]
        pool = [dict(id="abbrev", texts=["测试", "侧时"], scores=[-1.5, 0.])]
        selection = self.selection()
        selection["choices"]["abbrev/40"] = 2
        with tempfile.TemporaryDirectory() as directory:
            results, _, _ = frozen.evaluate_rows(current, pool, selection, Path(directory))
        chosen = {r["n"]: r for r in results if r["mode"] == "gate_model" and r["level"] == "bucket"}
        self.assertEqual(chosen[20]["broken"], 1)
        self.assertEqual(chosen[40]["broken"], 0)


class BuilderTests(unittest.TestCase):
    def test_exclusions_accept_both_case_formats(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "cases.jsonl"
            gate.write_jsonl(path, [dict(raw="nihao", context="上文", target="你好"),
                                    dict(keys="ceshi", context="另一段", target="测试")])
            self.assertEqual(build.exclusion_keys([path]),
                             {("nihao", "上文", "你好"), ("ceshi", "另一段", "测试")})
            with self.assertRaises(FileNotFoundError):
                build.exclusion_keys([Path(directory) / "missing"])

    def test_name_filter_rejects_dictionary_noise(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "dict.txt"
            path.write_text("李清照 100 nr\n王明 51 nr\n张三 2 nr\n高尔夫 100 nr\n杨家庄 100 nr\n"
                            "高宗 198 nr\n白吃 67 nr\n蔡家潭 256 nr\n白鹿仓 75 nr\n田赋 197 nr\n龙虎 59 nr\n白刃 85 nr\n"
                            "严重性 158 nr\n林子里 50 nr\n徐力里 78 nr\n黄蓉叹 60 nr\n高家岭 257 nr\n梁子湖 279 nr\n袁大头 140 nr\n"
                            "王家岗 256 nr\n蒋家冲 512 nr\n王大夫 448 nr\n卢员外 65 nr\n秦二世 75 nr\n黄果树 55 nr\n"
                            "叶绿素 128 nr\n卢布 334 nr\n郭大侠 156 nr\n余观主 77 nr\n黄蓉 100 nr\n黄蓉笑 60 nr\n"
                            "顾问 1000 nr\n李明 100 n\n伊丽莎白 500 nr\n李四五六 90 nr\n")
            names, _ = build.name_words(path)
            self.assertEqual({r["target"] for r in names}, {"李清照", "王明", "黄蓉"})

    def test_english_is_exact_ascii_with_canonical_case(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "terms.dict.yaml"
            path.write_text("---\nname: terms\n...\nAPI\tapi\t100\napi\tapi\t90\n"
                            "C++\tcpp\t80\n中文\tzw\t70\nPython\tpy\t60\nDocker\tdocker\t50\n")
            words = build.english_words([path])
            self.assertEqual([(r["target"], r["raw"]) for r in words], [("API", "api"), ("Docker", "docker")])

    def test_sampling_uses_only_authorized_sources_and_is_disjoint(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "asap.valid.txt").write_text("这里环境很好，我们还会再来。服务很热情，朋友非常喜欢。\n")
            (root / "wikipedia.valid.txt").write_text("这里原来有河流，后来建设城市。城市建设发展很快。\n")
            (root / "synth.valid.txt").write_text("不该读取的来源\n")
            words = root / "dict.txt"
            words.write_text("这里 100 r\n环境 100 n\n服务 100 n\n朋友 100 n\n城市 100 n\n建设 100 v\n"
                             "原来 100 d\n河流 100 n\n喜欢 100 v\n很好 100 a\n在 100 p\n再 100 d\n")
            quotas = dict(full=2, short=2, abbrev=2, mixed=2, typo=2, erhua=2)
            rows, _, _ = build.sample_independent(root, words, set(), 20261110, quotas)
            self.assertTrue(rows)
            self.assertEqual({r["src"] for r in rows}, {"asap", "wiki"})
            excluded = {gate.case_key(r) for r in rows}
            again, _, _ = build.sample_independent(root, words, excluded, 20261110, quotas)
            self.assertFalse(excluded & {gate.case_key(r) for r in again})
            repeated, _, _ = build.sample_independent(root, words, set(), 20261110, quotas)
            self.assertEqual(rows, repeated)
            self.assertEqual(len(rows), len({gate.case_key(r) for r in rows}))


if __name__ == "__main__":
    unittest.main()
