"""Synthetic-only tests for homophone_anatomy.py (hand-written dict.yaml + rows.jsonl fixtures).

Run: python3 -m unittest scripts/eval/ranking_bench/test_homophone_anatomy.py
"""
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("homophone_anatomy.py")
SPEC = importlib.util.spec_from_file_location("homophone_anatomy", SCRIPT)
anatomy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(anatomy)


def write_dict(directory, name, rows, header_name=None):
    body = [
        "# synthetic fixture",
        "---",
        f"name: {header_name or name}",
        'version: "0.0.0"',
        "sort: by_weight",
        "...",
    ] + list(rows)
    (Path(directory) / f"{name}.dict.yaml").write_text("\n".join(body) + "\n", encoding="utf-8")


def write_rows(path, rows):
    Path(path).write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows), encoding="utf-8")


def miss_case(cid, top, target, rank=1, bucket="full", pool=None):
    candidates = list(pool) if pool is not None else [target, top] if rank == 0 else [top, target]
    return {"id": cid, "bucket": bucket, "raw": "shi", "context": "",
            "target": target, "candidates": candidates, "rank_all": rank}


CORE_ROWS = [
    "事实\tshi shi\t200",
    "逝世\tshi shi\t200",
    "实验\tshi yan\t200",
    "试验\tshi yan\t300",
    "事物\tshi wu\t300",
    "事务\tshi wu\t200",
    "你好\tni hao\t300",
    "再见\tzai jian\t300",
    "研究\tyan jiu\t200",
    "研究院\tyan jiu yuan\t300",
    "朝阳\tzhao yang\t200",
    "朝阳\tchao yang\t200",
    "超洋\tchao yang\t200",
    "深秘\tshen mi\t200",
]

# 分类互斥按序判：四个分类各至少 1 例，外加同音 no / 多读音 / unknown / 非数值权重 / owner 跳过。
STANDARD_CASES = [
    miss_case("case-tie", "事实", "逝世"),
    miss_case("case-inverted", "实验", "试验"),
    miss_case("case-top-higher", "事物", "事务"),
    miss_case("case-top-missing", "喵语", "事务"),
    miss_case("case-target-missing", "你好", "拟好"),
    miss_case("case-multi-reading", "超洋", "朝阳"),
    miss_case("case-homo-no", "你好", "再见"),
    miss_case("case-weight-none", "神秘", "深秘"),
    miss_case("case-prefix", "研究", "研究院"),
    miss_case("case-owner-skipped", "秘密", "事实"),
]

EXPECTED_CLASSES = {
    "case-tie": "tie_same_weight",
    "case-inverted": "weight_inverted",
    "case-top-higher": "top_weight_higher",
    "case-top-missing": "other",
    "case-target-missing": "target_missing_in_lexicon",
    "case-multi-reading": "tie_same_weight",
    "case-homo-no": "tie_same_weight",
    "case-weight-none": "other",
    "case-prefix": "weight_inverted",
    "case-owner-skipped": "other",
}

EXPECTED_HOMOPHONE = {
    "case-tie": "yes",
    "case-inverted": "yes",
    "case-top-higher": "yes",
    "case-top-missing": "unknown",
    "case-target-missing": "unknown",
    "case-multi-reading": "yes",
    "case-homo-no": "no",
    "case-weight-none": "yes",
    "case-prefix": "no",
    "case-owner-skipped": "unknown",
}


class HomophoneAnatomyTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.lexicon = self.root / "lexicon"
        self.lexicon.mkdir()
        write_dict(self.lexicon, "core", CORE_ROWS)
        write_dict(self.lexicon, "extra", ["神秘\tshen mi\t高"])
        write_dict(self.lexicon, "agent_owner_words", ["秘密\tmi mi\t999"])
        self.rows_path = self.root / "rows.jsonl"
        write_rows(self.rows_path, STANDARD_CASES)

    def by_id(self, cases):
        return {case["id"]: case for case in cases}

    def analyze(self, show_text=False):
        lexicon = anatomy.load_lexicon(self.lexicon)
        rows = anatomy.load_rows([self.rows_path])
        selected, skipped = anatomy.select_cases(rows)
        cases = [anatomy.analyze_case(row, lexicon, show_text=show_text) for row in selected]
        return lexicon, rows, selected, skipped, cases

    def test_selection_filters_bucket_rank_and_absent(self):
        rows = [
            miss_case("ok-1", "事实", "逝世"),
            miss_case("abbrev", "事实", "逝世", bucket="abbrev"),
            miss_case("first", "逝世", "事实", rank=0),
            miss_case("pool-out", "事实", "逝世", rank=-1, pool=["事实", "其他"]),
            miss_case("absent", "事实", "逝世", rank=70, pool=["事实", "其他"]),
        ]
        selected, skipped = anatomy.select_cases(rows)
        self.assertEqual([row["id"] for row in selected], ["ok-1"])
        self.assertEqual(skipped, {"bucket": 1, "rank": 2, "absent": 1})

    def test_classes_and_homophone_matches_matrix(self):
        _, _, selected, _, cases = self.analyze()
        self.assertEqual(len(selected), len(STANDARD_CASES))
        found = self.by_id(cases)
        for cid, expected in EXPECTED_CLASSES.items():
            self.assertEqual(found[cid]["class"], expected, cid)
        for cid, expected in EXPECTED_HOMOPHONE.items():
            self.assertEqual(found[cid]["homophone"], expected, cid)

    def test_sources_and_max_weights(self):
        _, _, _, _, cases = self.analyze()
        found = self.by_id(cases)
        self.assertEqual(found["case-tie"]["top_sources"], ["core"])
        self.assertEqual(found["case-tie"]["target_max_weight"], 200)
        self.assertEqual(found["case-weight-none"]["top_sources"], ["extra"])
        self.assertIsNone(found["case-weight-none"]["top_max_weight"])
        self.assertEqual(found["case-weight-none"]["target_max_weight"], 200)
        self.assertEqual(found["case-owner-skipped"]["top_sources"], [])
        self.assertEqual(found["case-target-missing"]["target_sources"], [])

    def test_multi_reading_term_shares_secondary_reading(self):
        _, _, _, _, cases = self.analyze(show_text=True)
        found = self.by_id(cases)
        self.assertEqual(sorted(found["case-multi-reading"]["shared_readings"]), ["chao yang"])

    def test_length_relation_and_prefix(self):
        _, _, _, _, cases = self.analyze()
        found = self.by_id(cases)
        self.assertEqual(found["case-tie"]["length"], "same")
        self.assertFalse(found["case-tie"]["prefix"])
        self.assertEqual(found["case-prefix"]["length"], "top_shorter")
        self.assertTrue(found["case-prefix"]["prefix"])

    def test_text_gated_by_show_text_flag(self):
        default_out = self.root / "default.md"
        self.assertEqual(
            anatomy.main(["--rows", str(self.rows_path), "--lexicon-dir", str(self.lexicon),
                          "--out", str(default_out)]), 0)
        default_md = default_out.read_text(encoding="utf-8")
        for text in ("事实", "逝世", "试验", "chao yang"):
            self.assertNotIn(text, default_md)
        for cid in EXPECTED_CLASSES:
            self.assertIn(cid, default_md)
        self.assertIn("分析例数：10", default_md)

        show_out = self.root / "show.md"
        json_out = self.root / "cases.json"
        self.assertEqual(
            anatomy.main(["--rows", str(self.rows_path), "--lexicon-dir", str(self.lexicon),
                          "--out", str(show_out), "--json", str(json_out), "--show-text"]), 0)
        show_md = show_out.read_text(encoding="utf-8")
        for text in ("事实", "逝世", "试验", "chao yang"):
            self.assertIn(text, show_md)
        cases = json.loads(json_out.read_text(encoding="utf-8"))
        found = self.by_id(cases)
        self.assertEqual(found["case-tie"]["top"], "事实")
        self.assertEqual(found["case-tie"]["target"], "逝世")

    def test_context_never_written_even_with_show_text(self):
        row = miss_case("case-ctx", "事实", "逝世")
        row["context"] = "见面先说"
        row["raw"] = "zzqqxx"
        hidden = self.root / "hidden.jsonl"
        write_rows(hidden, [row])
        out = self.root / "ctx.md"
        json_out = self.root / "ctx.json"
        self.assertEqual(
            anatomy.main(["--rows", str(hidden), "--lexicon-dir", str(self.lexicon),
                          "--out", str(out), "--json", str(json_out), "--show-text"]), 0)
        text = out.read_text(encoding="utf-8")
        self.assertNotIn("见面先说", text)
        self.assertNotIn("zzqqxx", text)
        case = json.loads(json_out.read_text(encoding="utf-8"))[0]
        self.assertNotIn("raw", case)
        self.assertNotIn("context", case)

    def test_multiple_rows_files_are_combined(self):
        first = self.root / "a.jsonl"
        second = self.root / "b.jsonl"
        write_rows(first, [STANDARD_CASES[0]])
        write_rows(second, [STANDARD_CASES[1], STANDARD_CASES[2]])
        out = self.root / "multi.md"
        self.assertEqual(
            anatomy.main(["--rows", str(first), str(second), "--lexicon-dir", str(self.lexicon),
                          "--out", str(out)]), 0)
        self.assertIn("分析例数：3", out.read_text(encoding="utf-8"))

    def test_invalid_line_fails_loudly(self):
        bad = self.root / "bad.jsonl"
        bad.write_text('{"id":"x","bucket":"full"}\n', encoding="utf-8")
        out = self.root / "never.md"
        with self.assertRaises(ValueError):
            anatomy.load_rows([bad])
        self.assertEqual(
            anatomy.main(["--rows", str(bad), "--lexicon-dir", str(self.lexicon),
                          "--out", str(out)]), 2)
        self.assertFalse(out.exists())

    def test_empty_match_set_writes_zero_report(self):
        only_first = [miss_case("first", "逝世", "事实", rank=0)]
        rows_path = self.root / "first-only.jsonl"
        write_rows(rows_path, only_first)
        out = self.root / "empty.md"
        self.assertEqual(
            anatomy.main(["--rows", str(rows_path), "--lexicon-dir", str(self.lexicon),
                          "--out", str(out)]), 0)
        text = out.read_text(encoding="utf-8")
        self.assertIn("分析例数：0", text)
        self.assertIn("（无用例）", text)


if __name__ == "__main__":
    unittest.main()
