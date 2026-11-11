"""Synthetic checks of owner-replay accounting; never reads owner records."""
import contextlib
import io
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest

from scripts.eval import owner_replay


class PhraseLadderReplayTests(unittest.TestCase):
    def compare(self, cases, old_pages, new_pages, frequent=()):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "cases.jsonl").write_text("".join(json.dumps(row) + "\n" for row in cases))
            (root / "manifest.json").write_text(json.dumps({"topWords": [[word, 1] for word in frequent]}))
            for name, pages in [("old", old_pages), ("new", new_pages)]:
                (root / (name + ".jsonl")).write_text("".join(
                    json.dumps({"id": case["id"], "page": page}) + "\n" for case, page in zip(cases, pages)))
            with contextlib.redirect_stdout(io.StringIO()):
                stats = owner_replay.cmd_compare(SimpleNamespace(cases=str(root), old=str(root / "old.jsonl"),
                    new=str(root / "new.jsonl"), report=str(root / "report.md")))
            return stats, (root / "report.md").read_text()

    def case(self, cid, target, weight=1, kind="miss", deleted=False):
        return dict(id=cid, target=target, keys="ceshi", context="", bucket="long",
                    weight=weight, kind=kind, deleted=deleted)

    def test_weighted_first_five_and_all_first_flips_are_reported(self):
        cases = [self.case("a", "甲", 7, "hit"), self.case("b", "乙"), self.case("c", "丙")]
        stats, report = self.compare(cases, [["甲"], ["旧", "乙"], ["旧"]],
                                     [["甲"], ["新", "乙"], ["旧", "丙"]])
        self.assertEqual(stats["metrics"]["old"]["top1Weighted"], 7)
        self.assertEqual(stats["metrics"]["old"]["top5Weighted"], 8)
        self.assertEqual(stats["metrics"]["new"]["top5Weighted"], 9)
        self.assertEqual([r["id"] for r in stats["firstChanges"]], ["b"])
        self.assertIn("首位翻转", report)

    def test_deleted_hits_are_separate_from_accuracy_but_keep_flips(self):
        cases = [self.case("a", "甲", 7, "hit", True), self.case("b", "乙", deleted=True)]
        stats, _ = self.compare(cases, [["甲"], ["旧"]], [["新"], ["旧", "乙"]])
        self.assertEqual(stats["metrics"]["old"]["cases"], 1)
        self.assertEqual(stats["redoRisk"]["old"]["cases"], 2)
        self.assertEqual(stats["redoRisk"]["new"]["top5"], 1)
        self.assertEqual([r["id"] for r in stats["firstChanges"]], ["a"])

    def test_all_five_hundred_frequent_words_are_guarded_on_first_page(self):
        words = [f"词{i}" for i in range(500)]
        cases = [self.case(str(i), word) for i, word in enumerate(words)]
        pages = [["首选", word] for word in words]
        unchanged, _ = self.compare(cases, pages, pages, words)
        self.assertTrue(unchanged["pass"])
        broken = [page[:] for page in pages]
        broken[-1] = ["首选"]
        stats, _ = self.compare(cases, pages, broken, words)
        self.assertFalse(stats["pass"])
        self.assertEqual([r["target"] for r in stats["hotPageLost"]], [words[-1]])


if __name__ == "__main__":
    unittest.main()
