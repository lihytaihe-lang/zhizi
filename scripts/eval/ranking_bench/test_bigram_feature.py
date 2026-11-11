"""Synthetic-only tests for bigram_feature.py (hand-written segmented corpus + rows.jsonl fixtures).

Run: python3 -m unittest scripts/eval/ranking_bench/test_bigram_feature.py
"""
import importlib.util
import json
import math
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("bigram_feature.py")
SPEC = importlib.util.spec_from_file_location("bigram_feature", SCRIPT)
feature = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(feature)

# 合成语料：行内相邻词对计数见 EXPECTED_PAIRS；「去 买菜」出现 3 次，「去 打扫」仅 1 次。
CORPUS = "\n".join([
    "去 吃 饭 了",
    "去 吃 饭 了",
    "去 吃 饭 了",
    "去 吃 饭",
    "去 买菜 了",
    "去 买菜",
    "你 去 吃 饭",
    "你 去 打扫",
    "同学 去 吃 饭",
    "同学 去 吃 饭",
    "去 吃 饭",
    "去 吃 饭",
    "去 吃 饭",
    "去 吃 饭",
    "同学 买菜 了",
    "同学 买菜 了",
    "同学 买菜 了",
    "同学 买菜 了",
    "去 买菜",
]) + "\n"

LAMBDAS = [0.0, 0.25, 0.5, 1.0, 2.0]


def make_row(cid, context, target, candidates, rank_all):
    return {"id": cid, "bucket": "full", "raw": "ni", "context": context,
            "target": target, "candidates": candidates, "rank_all": rank_all}


# promote：原名次 1，λ>=1 时靠（去→吃）搭配拉回首位；demote：原首位，λ=2 时被（同学→买菜）挤掉。
# empty/no-record/fallback*/bucket*：空上文、末词无搭配记录、词表外回退与分桶用例，各 λ 排序不变。
ROWS = [
    make_row("case-promote", "我去", "吃", ["买菜", "吃"], 1),
    make_row("case-demote", "好同学", "去", ["去", "买菜"], 0),
    make_row("case-empty", "", "吃", ["吃", "买菜"], 0),
    make_row("case-no-record", "你了", "吃", ["买菜", "吃"], 1),
    make_row("case-bucket2", "你去", "买菜", ["吃", "买菜"], 1),
    make_row("case-bucket3", "我去", "吃饭了", ["买菜", "吃饭了"], 1),
    make_row("case-fallback", "啊啊", "吃", ["吃", "买菜"], 0),
    make_row("case-fallback1", "嗯", "吃", ["买菜", "吃"], 1),
    make_row("case-absent", "我去", "神秘词", ["买菜", "吃"], -1),
]

EXPECTED_RANKS = {
    "case-promote": {0.0: 1, 0.25: 1, 0.5: 1, 1.0: 0, 2.0: 0},
    "case-demote": {0.0: 0, 0.25: 0, 0.5: 0, 1.0: 0, 2.0: 1},
    "case-empty": {lam: 0 for lam in LAMBDAS},
    "case-no-record": {lam: 1 for lam in LAMBDAS},
    "case-bucket2": {lam: 1 for lam in LAMBDAS},
    "case-bucket3": {lam: 1 for lam in LAMBDAS},
    "case-fallback": {lam: 0 for lam in LAMBDAS},
    "case-fallback1": {lam: 1 for lam in LAMBDAS},
}

EXPECTED_PREV = {
    "case-promote": ("去", True, True),
    "case-demote": ("同学", True, True),
    "case-empty": (None, False, False),
    "case-no-record": ("了", True, False),
    "case-bucket2": ("去", True, True),
    "case-bucket3": ("去", True, True),
    "case-fallback": ("啊啊", False, False),
    "case-fallback1": ("嗯", False, False),
}

# 默认报告只出计数与 id，不得出现上文/候选/目标文本。
HIDDEN_TEXTS = ["去", "吃", "买菜", "同学", "你了", "嗯", "啊啊", "好同学", "我去", "你去", "吃饭了", "神秘词"]


class BigramFeatureTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.corpus_path = self.root / "corpus.txt"
        self.corpus_path.write_text(CORPUS, encoding="utf-8")
        self.table_path = self.root / "table.json"
        self.rows_path = self.root / "rows.jsonl"
        self.rows_path.write_text(
            "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in ROWS), encoding="utf-8")
        data, _ = feature.build_table([self.corpus_path])
        self.table_path.write_text(json.dumps(data, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
        self.table = feature.load_table(self.table_path)

    def build(self, **kwargs):
        data, stats = feature.build_table([self.corpus_path], **kwargs)
        return data, stats

    def analyze(self, show_text=False):
        rows = feature.load_rows([self.rows_path])
        selected, absent = feature.select_cases(rows)
        cases = [feature.analyze_case(row, self.table, LAMBDAS, 1.0, show_text=show_text) for row in selected]
        return selected, absent, cases

    def test_build_counts_min_count_and_unigram(self):
        data, stats = self.build()
        self.assertEqual(data["V"], 8)
        self.assertEqual(stats["tokens"], 61)
        self.assertEqual(data["unigram"]["去"], 15)
        self.assertEqual(data["unigram"]["买菜"], 7)
        self.assertEqual(data["unigram"]["了"], 8)
        self.assertEqual(data["unigram"]["打扫"], 1)
        self.assertEqual(data["bigram"]["去"]["吃"], 11)
        self.assertEqual(data["bigram"]["同学"]["买菜"], 4)
        self.assertEqual(data["bigram"]["你"]["去"], 2)
        self.assertIsNone(data["bigram"]["去"].get("打扫"))
        self.assertNotIn("打扫", data["bigram"])
        self.assertEqual(stats["pairs_kept"], 8)

    def test_build_cap_keeps_top_pairs_deterministically(self):
        data, stats = self.build(max_entries=3)
        self.assertEqual(stats["pairs_kept"], 3)
        self.assertEqual(set(data["bigram"]["去"]), {"吃"})
        self.assertEqual(data["bigram"]["吃"], {"饭": 11})
        self.assertEqual(data["bigram"]["买菜"], {"了": 5})
        self.assertNotIn("同学", data["bigram"])

    def test_extract_prev_word_longest_match_and_fallback(self):
        extract = lambda context: feature.extract_prev_word(context, self.table.vocab, self.table.max_len)
        self.assertEqual(extract("好同学"), ("同学", True))
        self.assertEqual(extract("同学去"), ("去", True))
        self.assertEqual(extract("啊啊"), ("啊啊", False))
        self.assertEqual(extract("嗯"), ("嗯", False))
        self.assertEqual(extract(""), (None, False))

    def test_lambda_zero_preserves_original_order(self):
        self.assertEqual(feature.rerank(["甲", "乙", "丙"], "去", self.table.data, 1.0, 0.0), [0, 1, 2])
        self.assertEqual(feature.rerank(["乙", "甲", "乙"], "去", self.table.data, 1.0, 0.0), [0, 1, 2])
        self.assertEqual(feature.rerank(["买菜", "吃"], "去", self.table.data, 1.0, 0.0), [0, 1])

    def test_bigram_formula_and_rerank_scores(self):
        self.assertAlmostEqual(feature.bigram_logprob("去", "吃", self.table.data, 1.0), math.log(12 / 23))
        self.assertAlmostEqual(feature.bigram_logprob("去", "买菜", self.table.data, 1.0), math.log(4 / 23))
        self.assertAlmostEqual(feature.bigram_logprob(None, "吃", self.table.data, 1.0), math.log(1 / 8))
        self.assertEqual(feature.rerank(["买菜", "吃"], "去", self.table.data, 1.0, 1.0), [1, 0])
        self.assertEqual(feature.rerank(["买菜", "吃"], "去", self.table.data, 1.0, 0.5), [0, 1])
        self.assertEqual(feature.rerank(["去", "买菜"], "同学", self.table.data, 1.0, 2.0), [1, 0])

    def test_selection_and_case_analysis(self):
        selected, absent, cases = self.analyze()
        self.assertEqual(absent, 1)
        self.assertEqual([row["id"] for row in selected], [row["id"] for row in ROWS[:-1]])
        found = {case["id"]: case for case in cases}
        row_by_id = {row["id"]: row for row in ROWS}
        for cid, expected in EXPECTED_RANKS.items():
            self.assertEqual(found[cid]["ranks"], expected, cid)
            self.assertEqual(found[cid]["base_rank"],
                             row_by_id[cid]["candidates"].index(row_by_id[cid]["target"]), cid)
        for cid, (prev, matched, has_record) in EXPECTED_PREV.items():
            self.assertNotIn("prev", found[cid], cid)
            self.assertNotIn("candidates", found[cid], cid)
            self.assertEqual(found[cid]["matched"], matched, cid)
            self.assertEqual(found[cid]["has_record"], has_record, cid)

    def test_buckets_and_lengths(self):
        _, _, cases = self.analyze()
        found = {case["id"]: case for case in cases}
        self.assertEqual(found["case-promote"]["length_bucket"], "1字")
        self.assertEqual(found["case-bucket2"]["length_bucket"], "2字")
        self.assertEqual(found["case-bucket3"]["length_bucket"], "3字+")

    def test_cli_build_and_default_report_privacy(self):
        self.assertEqual(feature.main(["build", "--corpus", str(self.corpus_path), "--out", str(self.table_path)]), 0)
        data = json.loads(self.table_path.read_text(encoding="utf-8"))
        self.assertEqual(data["V"], 8)
        report_path = self.root / "report.md"
        self.assertEqual(
            feature.main(["score", "--rows", str(self.rows_path), "--table", str(self.table_path),
                          "--out", str(report_path)]), 0)
        report = report_path.read_text(encoding="utf-8")
        for text in HIDDEN_TEXTS:
            self.assertNotIn(text, report, text)
        for cid in ("case-promote", "case-demote"):
            self.assertIn(cid, report)
        self.assertIn("| 0 | 37.5% | 100.0% | 0.688 | 0 | 0 | 0 |", report)
        self.assertIn("| 1 | 50.0% | 100.0% | 0.750 | 0 | 1 | 1 |", report)
        self.assertIn("| 2 | 37.5% | 100.0% | 0.688 | 1 | 1 | 0 |", report)
        self.assertIn("| 0 | 1字 | 6 | 50.0% | 100.0% | 0.750 | 0 | 0 |", report)
        self.assertIn("| 0 | 2字 | 1 | 0.0% | 100.0% | 0.500 | 0 | 0 |", report)
        self.assertIn("| 空上文（无末词） | 1 | 12.5% |", report)
        self.assertIn("| 末词命中搭配记录 | 4 | 50.0% |", report)
        self.assertIn("| 末词无搭配记录（含词表外回退） | 3 | 37.5% |", report)
        self.assertIn("末词来源：最长匹配 5 例、回退末 1–2 字 3 例。", report)
        self.assertIn("- λ=1 拉回: case-promote", report)
        self.assertIn("- λ=2 被挤掉: case-demote", report)

    def test_cli_show_text_gates_candidate_and_target_but_never_context(self):
        report_path = self.root / "show.md"
        self.assertEqual(
            feature.main(["score", "--rows", str(self.rows_path), "--table", str(self.table_path),
                          "--out", str(report_path), "--show-text"]), 0)
        report = report_path.read_text(encoding="utf-8")
        for text in ("买菜", "同学", "case-promote"):
            self.assertIn(text, report)
        for context in ("我去", "好同学", "你去", "你了", "啊啊"):
            self.assertNotIn(context, report)

    def test_cli_custom_lambda_grid(self):
        report_path = self.root / "grid.md"
        self.assertEqual(
            feature.main(["score", "--rows", str(self.rows_path), "--table", str(self.table_path),
                          "--out", str(report_path), "--lambdas", "0,2", "--alpha", "2"]), 0)
        report = report_path.read_text(encoding="utf-8")
        self.assertIn("| 0 | 37.5% |", report)
        self.assertIn("| 2 | 37.5% | 100.0% |", report)
        self.assertNotIn("0.25", report)

    def test_invalid_rows_fail_loudly(self):
        bad = self.root / "bad.jsonl"
        bad.write_text('{"id":"x","bucket":"full","context":"","target":"t"}\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            feature.load_rows([bad])
        out = self.root / "never.md"
        self.assertEqual(
            feature.main(["score", "--rows", str(bad), "--table", str(self.table_path), "--out", str(out)]), 2)
        self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
