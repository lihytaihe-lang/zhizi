"""Synthetic-fixture tests for owner_replay.py (all events fictional, no real owner data).

Run: cd scripts/eval && python3 -m unittest test_owner_replay -v
"""
import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from argparse import Namespace
from contextlib import redirect_stdout
from datetime import datetime
from io import StringIO
from pathlib import Path

SCRIPT = Path(__file__).with_name("owner_replay.py")
SPEC = importlib.util.spec_from_file_location("owner_replay", SCRIPT)
replay = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(replay)

FIXTURE = Path(__file__).with_name("fixtures") / "owner_replay_events.jsonl"
# 45 synthetic events plus 2 bad lines; hit pool is 29 first-choice commits.
KIND_COUNTS = {"hit": 5, "miss": 3, "segments": 1}


def call(command, **kwargs):
    with redirect_stdout(StringIO()):
        return getattr(replay, command)(Namespace(**kwargs))


def synth_event(ms, pinyin, selected, index, **extra):
    row = {"outcome": "committed_candidate", "recordedAtMs": ms, "pinyin": pinyin,
           "candidates": [selected], "candidateComments": [""], "selectedIndex": index,
           "selectedText": selected, "backspaces": 0, "committedText": selected}
    row.update(extra)
    return row


def local_ms(*args):
    """Epoch milliseconds of a local wall-clock time, the way `since_ms` reads windows."""
    return int(datetime(*args).timestamp() * 1000)


class OwnerReplayTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)

    def run_sample(self, out="cases", since="2026-10-19", ratio=7, seed=1,
                   events=str(FIXTURE), until=None):
        manifest = call("cmd_sample", events=events, since=since, hit_ratio=ratio,
                        seed=seed, out=str(self.root / out), until=until)
        cases = [json.loads(line) for line in
                 (self.root / out / "cases.jsonl").read_text(encoding="utf-8").splitlines()]
        return cases, manifest

    def write_events(self, rows, name="events.jsonl"):
        path = self.root / name
        path.write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows),
                        encoding="utf-8")
        return str(path)

    def write_outs(self, cases, spec, name="out"):
        """run_eval.sh-style outputs; spec maps id -> (old_first, new_first), default both target."""
        paths = []
        for side, index in (("old", 0), ("new", 1)):
            path = self.root / f"{name}-{side}.jsonl"
            with open(path, "w", encoding="utf-8") as fh:
                for case in cases:
                    first = spec.get(case["id"], (case["target"], case["target"]))[index]
                    fh.write(json.dumps({"id": case["id"], "bucket": case["bucket"],
                                         "keys": case["keys"], "target": case["target"],
                                         "context": case["context"], "first": first == case["target"],
                                         "onPage": True, "page": [first, "垫片一", "垫片二"],
                                         "model": "done"}, ensure_ascii=False) + "\n")
            paths.append(path)
        return str(paths[0]), str(paths[1])

    def run_compare(self, cases_dir, cases, spec, report=None):
        old, new = self.write_outs(cases, spec)
        return call("cmd_compare", cases=str(cases_dir), old=old, new=new, report=report)

    def pick(self, cases, kind, target=None):
        for case in cases:
            if case["kind"] == kind and target in (None, case["target"]):
                return case
        raise AssertionError(f"no {kind} case with target {target}")

    def test_sample_is_reproducible_with_the_same_seed(self):
        cases_a, manifest_a = self.run_sample(out="a")
        cases_b, manifest_b = self.run_sample(out="b")
        self.assertEqual((self.root / "a" / "cases.jsonl").read_bytes(),
                         (self.root / "b" / "cases.jsonl").read_bytes())
        self.assertEqual(manifest_a, manifest_b)
        self.assertEqual(manifest_a["kindCounts"], KIND_COUNTS)
        self.assertEqual(manifest_a["lines"], 47)
        self.assertEqual(manifest_a["badLines"], 2)
        self.assertEqual(manifest_a["since"], "2026-10-19")
        self.assertEqual(manifest_a["hitRatio"], 7)
        self.assertEqual(manifest_a["seed"], 1)
        self.assertEqual(len(cases_a), sum(KIND_COUNTS.values()))

    def test_sample_filters_and_manifest(self):
        cases, manifest = self.run_sample()
        ids = [case["id"] for case in cases]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertTrue(all(case["keys"].isalpha() and case["keys"].islower() for case in cases))
        # Region-A events (before since) and filtered rows must not become cases.
        banned = {"锦豪", "毯子", "什么", "。", "忘记", "你好哇", "你三号", ""}
        self.assertFalse(any(case["target"] in banned for case in cases))
        for case in cases:
            self.assertEqual(case["weight"], 7 if case["kind"] == "hit" else 1)
        by_target = {case["target"]: case for case in cases}
        self.assertEqual(by_target["得"]["bucket"], "short")    # 2 letters
        self.assertEqual(by_target["么嘛"]["bucket"], "mid")    # 4 letters
        self.assertEqual(by_target["签字嘛"]["bucket"], "long")  # 9 letters
        self.assertEqual(manifest["eventsSha256"],
                         hashlib.sha256(FIXTURE.read_bytes()).hexdigest())
        self.assertEqual(manifest["topWords"][0], ["得", 10])   # ten "de" hits outrank the rest
        self.assertIn(["柠檬", 10], manifest["topWords"][:500])
        self.assertNotIn("签字嘛", [word for word, _ in manifest["topWords"]])  # no committedText

    def test_sample_lists_segments_separately(self):
        cases, _ = self.run_sample()
        segments = [case for case in cases if case["kind"] == "segments"]
        self.assertEqual([case["target"] for case in segments], ["喵薄荷"])
        misses = [case for case in cases if case["kind"] == "miss"]
        self.assertEqual(len(misses), 3)

    def test_sample_preserves_segments_and_event_order(self):
        cases, _ = self.run_sample(ratio=1)
        events, _, _ = replay.read_events(FIXTURE)
        expected = [row for row in events
                    if row.get("outcome") == "committed_candidate"
                    and type(row.get("recordedAtMs")) is int
                    and row["recordedAtMs"] >= replay.since_ms("2026-10-19")
                    and replay.PINYIN.match(row.get("pinyin") or "")
                    and row.get("selectedText")
                    and type(row.get("selectedIndex")) is int and row["selectedIndex"] >= 0]
        self.assertEqual([case["target"] for case in cases],
                         [row["selectedText"] for row in expected])
        for case, row in zip(cases, expected):
            self.assertEqual(case.get("segments"), row.get("segments"))

    def test_curve_groups_500_in_file_order_and_keeps_final_partial_group(self):
        path = self.root / "sequential.jsonl"
        # Reversed IDs must not sort the chronological stream. Unselectable rows still count.
        rows = [{"id": f"case-{1002 - i}", "first": i < 250 or i == 1000,
                 "selectable": None} for i in range(1002)]
        path.write_text("\n" + "\n\n".join(json.dumps(row) for row in rows) + "\n",
                        encoding="utf-8")
        output = StringIO()
        with redirect_stdout(output):
            result = replay.cmd_curve(Namespace(input=str(path)))
        self.assertEqual(result, [
            {"start": 1, "end": 500, "cases": 500, "first": 250, "rate": 0.5},
            {"start": 501, "end": 1000, "cases": 500, "first": 0, "rate": 0.0},
            {"start": 1001, "end": 1002, "cases": 2, "first": 1, "rate": 0.5},
        ])
        self.assertEqual(output.getvalue().splitlines(), [
            "range\tcases\tfirst\tfirst-rate", "1-500\t500\t250\t50.0%",
            "501-1000\t500\t0\t0.0%", "1001-1002\t2\t1\t50.0%",
        ])

    def test_curve_empty_input_and_cli(self):
        path = self.root / "empty.jsonl"
        path.write_text("\n", encoding="utf-8")
        self.assertEqual(call("cmd_curve", input=str(path)), [])
        result = subprocess.run([sys.executable, str(SCRIPT), "curve", "--input", str(path)],
                                check=True, capture_output=True, text=True)
        self.assertEqual(result.stdout, "range\tcases\tfirst\tfirst-rate\n")

    def test_compare_weights_sampled_hits_back_to_the_population(self):
        # One broken hit weighs 7 (sampled 1-in-7); two fixed misses weigh 2 together:
        # 2 < 3 x 7, so G1 must FAIL even though raw counts are 2 vs 1.
        cases_dir = self.root / "cases"
        cases, _ = self.run_sample()
        hit, m1, m2 = self.pick(cases, "hit"), self.pick(cases, "miss"), self.pick(cases, "miss", "得")
        stats = self.run_compare(cases_dir, cases, {
            hit["id"]: (hit["target"], "完全不同的长词"),
            m1["id"]: ("旧首选", m1["target"]),
            m2["id"]: ("旧首选", m2["target"]),
        })
        self.assertFalse(stats["pass"])
        self.assertEqual(stats["fixedWeighted"], 2)
        self.assertEqual(stats["brokeWeighted"], 7)
        self.assertEqual((len(stats["fixed"]), len(stats["broke"])), (2, 1))
        self.assertEqual(stats["both"], len(cases) - 3)
        self.assertEqual(stats["silent"], [])

    def test_compare_passes_when_fixes_outweigh_breaks(self):
        cases_dir = self.root / "cases"
        cases, _ = self.run_sample()
        report = self.root / "report.md"
        hit, m1, m2 = self.pick(cases, "hit"), self.pick(cases, "miss"), self.pick(cases, "miss", "得")
        quiet = self.pick(cases, "miss", "签字嘛")  # not among the committed top words
        stats = self.run_compare(cases_dir, cases, {
            quiet["id"]: (quiet["target"], "弄坏结果"),
            hit["id"]: ("旧首选", hit["target"]),
            m1["id"]: ("旧首选", m1["target"]),
            m2["id"]: ("旧首选", m2["target"]),
        }, report=str(report))
        self.assertTrue(stats["pass"])
        self.assertEqual((stats["fixedWeighted"], stats["brokeWeighted"]), (9, 1))
        self.assertEqual(stats["hot"], [])
        text = report.read_text(encoding="utf-8")
        self.assertIn("**PASS**", text)
        self.assertIn("弄坏清单", text)
        self.assertIn("静默错字", text)

    def test_compare_flags_silent_typos_by_length(self):
        cases_dir = self.root / "cases"
        cases, _ = self.run_sample()
        m1 = self.pick(cases, "miss", "么嘛")  # 2 characters
        same_len = self.run_compare(cases_dir, cases, {m1["id"]: ("么嘛", "什么")})
        self.assertEqual([c["id"] for c in same_len["silent"]], [m1["id"]])
        self.assertEqual(same_len["brokeWeighted"], 1)
        diff_len = self.run_compare(cases_dir, cases, {m1["id"]: ("么嘛", "别的词")})
        self.assertEqual(diff_len["silent"], [])
        self.assertEqual(len(diff_len["broke"]), 1)

    def test_compare_flags_breaks_of_high_frequency_words(self):
        # Breaking a top-500 word fails G1 even when fixes outweigh breaks 9 to 1.
        cases_dir = self.root / "cases"
        cases, _ = self.run_sample()
        hit, m1, de = self.pick(cases, "hit"), self.pick(cases, "miss"), self.pick(cases, "miss", "得")
        stats = self.run_compare(cases_dir, cases, {
            de["id"]: ("得", "得到"),          # longer than the target: not a silent typo
            hit["id"]: ("旧首选", hit["target"]),
            m1["id"]: ("旧首选", m1["target"]),
        })
        self.assertEqual([c["id"] for c in stats["hot"]], [de["id"]])
        self.assertTrue(stats["fixedWeighted"] >= 3 * stats["brokeWeighted"])
        self.assertFalse(stats["pass"])
        self.assertEqual(stats["silent"], [])

    def test_redo_classes(self):
        counts = call("cmd_redo", events=str(FIXTURE), since="2026-10-15")
        self.assertEqual(counts, {"retype_same_reading": 1, "punct_only": 2, "repick": 1, "other": 1})

    def test_since_ms_accepts_bare_dates_and_minutes(self):
        self.assertEqual(replay.since_ms("2026-10-20"),
                         int(datetime(2026, 10, 20).timestamp() * 1000))
        self.assertEqual(replay.since_ms("2026-10-20T10:30"),
                         int(datetime(2026, 10, 20, 10, 30).timestamp() * 1000))

    def test_sample_since_with_minutes_cuts_inside_a_day(self):
        rows = [synth_event(local_ms(2026, 10, 20, 9, 0, 5), "zao", "早", 0),
                synth_event(local_ms(2026, 10, 20, 10, 30, 0), "wucan", "午餐", 0),
                synth_event(local_ms(2026, 10, 20, 10, 30, 5), "wancan", "晚餐", 0),
                synth_event(local_ms(2026, 10, 20, 11, 0, 5), "yejian", "夜间", 0)]
        events = self.write_events(rows)
        cases, _ = self.run_sample(events=events, since="2026-10-20T10:30", ratio=1, out="w1")
        self.assertEqual([c["target"] for c in cases], ["午餐", "晚餐", "夜间"])  # cut itself included
        cases, _ = self.run_sample(events=events, since="2026-10-20T10:31", ratio=1, out="w2")
        self.assertEqual([c["target"] for c in cases], ["夜间"])
        # A bare date still means local midnight: the whole day is kept, as before.
        cases, _ = self.run_sample(events=events, since="2026-10-20", ratio=1, out="w3")
        self.assertEqual([c["target"] for c in cases], ["早", "午餐", "晚餐", "夜间"])
        cases, _ = self.run_sample(events=events, since="2026-10-21", ratio=1, out="w4")
        self.assertEqual(cases, [])
        # --until takes minutes too (exclusive).
        cases, _ = self.run_sample(events=events, since="2026-10-20", until="2026-10-20T11:00",
                                   ratio=1, out="w5")
        self.assertEqual([c["target"] for c in cases], ["早", "午餐", "晚餐"])

    def test_rates_matches_hand_computed_metrics(self):
        rows = [
            # window A (2026-10-27): eleven judged picks
            *[synth_event(local_ms(2026, 10, 27, 9, 0, s), "de", "的", 0) for s in range(1, 7)],
            synth_event(local_ms(2026, 10, 27, 9, 1, 1), "mema", "么嘛", 1),
            synth_event(local_ms(2026, 10, 27, 9, 1, 2), "qianzaima", "签字嘛", 2, maxPage=0),
            synth_event(local_ms(2026, 10, 27, 9, 1, 3), "miaoheguo", "喵薄荷", 3, maxPage=2),
            synth_event(local_ms(2026, 10, 27, 9, 1, 4), "mama", "妈妈", 0, deletedAfterCommit=2),
            synth_event(local_ms(2026, 10, 27, 9, 1, 5), "jintian", "今天", 0, redoAfterCommit=True),
            # rows `sample` would never judge: abandoned outcome and non-letter pinyin
            {"outcome": "abandoned", "recordedAtMs": local_ms(2026, 10, 27, 9, 2, 0),
             "pinyin": "wangji", "selectedIndex": 0, "selectedText": "忘记"},
            synth_event(local_ms(2026, 10, 27, 9, 2, 1), "ni3hao", "你三号", 1),
            # window B (2026-10-29 from 08:33): five judged picks; the 08:10 pick stays out
            synth_event(local_ms(2026, 10, 29, 8, 10, 0), "zao", "早", 0),
            synth_event(local_ms(2026, 10, 29, 8, 40, 1), "de", "的", 0),
            synth_event(local_ms(2026, 10, 29, 8, 40, 2), "ta", "他", 0),
            synth_event(local_ms(2026, 10, 29, 8, 40, 3), "mema", "么嘛", 1),
            synth_event(local_ms(2026, 10, 29, 8, 40, 4), "shenme", "什么", 4, maxPage=1),
            synth_event(local_ms(2026, 10, 29, 8, 40, 5), "ta", "他", 1, redoAfterCommit=True),
        ]
        output = StringIO()
        with redirect_stdout(output):
            result = replay.cmd_rates(Namespace(events=self.write_events(rows),
                                                a_since="2026-10-27", a_until="2026-10-29",
                                                b_since="2026-10-29T08:33", b_until=None))
        expected_a = {"commits": 11, "first": 8, "nonfirst_first_page": 2, "nonfirst_paged": 1,
                      "deleted": 2, "redo": 1}
        for key, n in expected_a.items():
            self.assertEqual(result["a"][key], n)
            if key != "commits":
                self.assertAlmostEqual(result["a"][key + "Rate"], n / 11)
        expected_b = {"commits": 5, "first": 2, "nonfirst_first_page": 2, "nonfirst_paged": 1,
                      "deleted": 1, "redo": 1}
        for key, n in expected_b.items():
            self.assertEqual(result["b"][key], n)
            if key != "commits":
                self.assertAlmostEqual(result["b"][key + "Rate"], n / 5)
        self.assertEqual(output.getvalue().splitlines(), [
            "指标\tA\tB",
            "上屏条数\t11\t5",
            "首选上屏率\t72.7%\t40.0%",
            "非首选·第一页\t18.2%\t40.0%",
            "非首选·翻页后\t9.1%\t20.0%",
            "上屏后删除\t18.2%\t20.0%",
            "重打率\t9.1%\t20.0%",
            "B 相对 A 的变化：上屏条数 -6 条，首选上屏率 -32.7pp，非首选·第一页 +21.8pp，"
            "非首选·翻页后 +10.9pp，上屏后删除 +1.8pp，重打率 +10.9pp",
        ])

    def test_rates_with_empty_windows_reports_zero(self):
        rows = [synth_event(local_ms(2026, 10, 27, 9, 0, 1), "de", "的", 0)]
        output = StringIO()
        with redirect_stdout(output):
            result = replay.cmd_rates(Namespace(events=self.write_events(rows),
                                                a_since="2026-11-01", a_until=None,
                                                b_since="2026-11-02", b_until=None))
        for side in ("a", "b"):
            self.assertEqual(result[side]["commits"], 0)
            for key in ("first", "nonfirst_first_page", "nonfirst_paged", "deleted", "redo"):
                self.assertEqual(result[side][key], 0)
                self.assertEqual(result[side][key + "Rate"], 0.0)
        self.assertIn("上屏条数\t0\t0", output.getvalue())
        self.assertIn("首选上屏率\t0.0%\t0.0%", output.getvalue())
        self.assertIn("B 相对 A 的变化", output.getvalue())

    def test_rates_cli(self):
        path = self.write_events([synth_event(local_ms(2026, 10, 27, 9, 0, 1), "de", "的", 0)])
        result = subprocess.run([sys.executable, str(SCRIPT), "rates", "--events", path,
                                 "--a-since", "2026-10-27", "--b-since", "2026-10-27T09:00"],
                                check=True, capture_output=True, text=True)
        self.assertIn("上屏条数\t1\t1", result.stdout)
        self.assertIn("B 相对 A 的变化", result.stdout)

    def test_variance_per_bucket(self):
        cases_dir = self.root / "cases"
        cases_dir.mkdir()
        rows = ([{"id": f"s{i}", "bucket": "short", "keys": "de", "target": "的", "context": "",
                  "kind": "hit", "weight": 1} for i in range(1, 5)]
                + [{"id": f"m{i}", "bucket": "mid", "keys": "nihao", "target": "你好", "context": "",
                    "kind": "hit", "weight": 1} for i in range(1, 3)]
                + [{"id": "l1", "bucket": "long", "keys": "jintian", "target": "今天", "context": "",
                    "kind": "hit", "weight": 1}])
        (cases_dir / "cases.jsonl").write_text(
            "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows), encoding="utf-8")
        firsts = {"a": {"s1": 1, "s2": 1, "s3": 1, "s4": 0, "m1": 1, "m2": 0, "l1": 1},
                  "b": {"s1": 1, "s2": 0, "s3": 0, "s4": 0, "m1": 1, "m2": 0, "l1": 1}}
        paths = {}
        for side, flags in firsts.items():
            path = self.root / f"var-{side}.jsonl"
            with open(path, "w", encoding="utf-8") as fh:
                for case in rows:
                    fh.write(json.dumps({"id": case["id"], "bucket": case["bucket"],
                                         "first": bool(flags[case["id"]]),
                                         "page": ["的" if flags[case["id"]] else "他"]}) + "\n")
            paths[side] = str(path)
        result = call("cmd_variance", a=paths["a"], b=paths["b"], cases=str(cases_dir))
        self.assertEqual(result["short"][0], 4)
        self.assertAlmostEqual(result["short"][1], 0.75)
        self.assertAlmostEqual(result["short"][2], 0.25)
        self.assertAlmostEqual(result["mid"][1], result["mid"][2])
        self.assertAlmostEqual(result["long"][1], 1.0)
        self.assertAlmostEqual(result["long"][2], 1.0)


if __name__ == "__main__":
    unittest.main()
