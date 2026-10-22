"""Synthetic-only tests for build_friends_set.py (no real log data, no device folders).

Run: cd scripts/eval && python3 -m unittest test_build_friends_set -v
"""
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).with_name("build_friends_set.py")
SPEC = importlib.util.spec_from_file_location("build_friends_set", SCRIPT)
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)


def event(text="你好", pinyin="ni hao", context="前面的话", redo=False, outcome="committed_candidate"):
    e = {"outcome": outcome, "committedText": text, "pinyin": pinyin, "precedingText": context}
    if redo:
        e["redoAfterCommit"] = True
    return e


class BuildFriendsSetTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)

    def write_device(self, name, days):
        folder = self.root / "data" / name
        folder.mkdir(parents=True, exist_ok=True)
        for day, events in days.items():
            (folder / day).write_text(
                "".join(json.dumps(e, ensure_ascii=False) + "\n" for e in events), encoding="utf-8")

    def run_build(self, out="out/friends.jsonl", extra=None):
        target = self.root / out
        argv = ["build_friends_set.py", "--data", str(self.root / "data"),
                "--out", str(target)] + (extra or [])
        with mock.patch.object(sys, "argv", argv):
            builder.main()
        return [json.loads(line) for line in target.read_text(encoding="utf-8").splitlines()], target

    def test_redo_ids_do_not_collide_with_normal_ids(self):
        # Same counter index in friend-1 and friend-1-redo must yield distinct ids.
        self.write_device("dev-a", {"d1.jsonl": [
            event(text="你好", pinyin="nihao", context="甲"),
            event(text="好的", pinyin="hao", context="乙", redo=True),
        ]})
        cases, _ = self.run_build()
        self.assertEqual([c["id"] for c in cases], ["friend-1-0001", "friend-1-redo-0001"])
        self.assertEqual([c["bucket"] for c in cases], ["friend-1", "friend-1-redo"])
        for c in cases:
            self.assertTrue(c["id"].startswith(c["bucket"] + "-"), c)
        self.assertEqual(len({c["id"] for c in cases}), len(cases))

    def test_ids_unique_across_buckets_and_counters(self):
        # Two devices -> friend-1 / friend-2; each also produces a redo row.
        self.write_device("dev-a", {"d1.jsonl": [
            event(text="你好", pinyin="nihao", context="甲"),
            event(text="好的", pinyin="hao", context="乙", redo=True),
        ]})
        self.write_device("dev-b", {"d1.jsonl": [
            event(text="世界", pinyin="shijie", context="丙"),
            event(text="再见", pinyin="zaijian", context="丁", redo=True),
        ]})
        cases, _ = self.run_build()
        ids = [c["id"] for c in cases]
        self.assertEqual(ids, ["friend-1-0001", "friend-1-redo-0001",
                               "friend-2-0001", "friend-2-redo-0001"])
        self.assertEqual(len(set(ids)), len(ids))

    def test_repeat_run_is_deterministic(self):
        self.write_device("dev-a", {"d1.jsonl": [
            event(), event(text="好的", pinyin="hao", context="乙", redo=True),
        ]})
        self.write_device("dev-b", {"d1.jsonl": [event(text="世界", pinyin="shijie", context="丙")]})
        _, first = self.run_build(out="out/a.jsonl")
        _, second = self.run_build(out="out/b.jsonl")
        self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_filtering_rules(self):
        self.write_device("dev-a", {"d1.jsonl": [
            {"oops": True},                                   # missing required fields -> skipped
            event(outcome="typing"),                          # wrong outcome -> skipped
            event(text="hello"),                              # non-Han target -> skipped
            event(text="你好，世界"),                          # punctuation in target -> skipped
            event(pinyin="123"),                              # no letters -> skipped
            event(context="看到[已隐藏]了吗"),                  # hidden marker -> skipped
            event(context="发到 a@b.com 吧"),                  # mail -> skipped
            event(context="看 https://example.com"),           # URL -> skipped
            event(context="单号 1234567890 到了"),              # long digit run -> skipped
            event(pinyin="a" * 20),                           # 20+ ascii run in keys -> skipped
            event(),                                          # the one good row
        ]})
        cases, _ = self.run_build()
        self.assertEqual(len(cases), 1)
        self.assertEqual((cases[0]["bucket"], cases[0]["keys"], cases[0]["target"]),
                         ("friend-1", "nihao", "你好"))
        self.assertEqual(cases[0]["id"], "friend-1-0001")

    def test_dedup_is_within_bucket_across_days(self):
        self.write_device("dev-a", {"d1.jsonl": [event()], "d2.jsonl": [event()]})
        self.write_device("dev-b", {"d1.jsonl": [event()]})
        cases, _ = self.run_build()
        self.assertEqual(len(cases), 2)  # duplicate inside friend-1 collapses; friend-2 keeps its own
        self.assertEqual([c["id"] for c in cases], ["friend-1-0001", "friend-2-0001"])

    def test_dedup_key_spans_normal_and_redo_first_seen_wins(self):
        # Identical (keys, text, context) is one test case regardless of redo flag;
        # the first occurrence decides the bucket. Deliberate: keeping both would
        # duplicate the same case, so the dedup key must not include redo.
        self.write_device("dev-a", {"d1.jsonl": [
            event(context="甲"),
            event(context="甲", redo=True),
        ]})
        cases, _ = self.run_build()
        self.assertEqual(len(cases), 1)
        self.assertEqual(cases[0]["bucket"], "friend-1")
        self.write_device("dev-a", {"d1.jsonl": [
            event(context="甲", redo=True),
            event(context="甲"),
        ]})
        cases, _ = self.run_build()
        self.assertEqual(len(cases), 1)
        self.assertEqual(cases[0]["bucket"], "friend-1-redo")
        self.assertEqual(cases[0]["id"], "friend-1-redo-0001")

    def test_exclude_device_renumbers_buckets(self):
        self.write_device("dev-a", {"d1.jsonl": [event()]})
        self.write_device("dev-b", {"d1.jsonl": [event(text="世界", pinyin="shijie")]})
        cases, _ = self.run_build(extra=["--exclude-device", "dev-a"])
        self.assertEqual([(c["id"], c["target"]) for c in cases], [("friend-1-0001", "世界")])


if __name__ == "__main__":
    unittest.main()
