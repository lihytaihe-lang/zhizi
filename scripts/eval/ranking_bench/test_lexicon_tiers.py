"""Synthetic-only tests for lexicon_tiers.py (small hand-written dict.yaml fixtures).

Run: python3 -m unittest scripts/eval/ranking_bench/test_lexicon_tiers.py
"""
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("lexicon_tiers.py")
SPEC = importlib.util.spec_from_file_location("lexicon_tiers", SCRIPT)
tiers = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(tiers)


def write_dict(directory, name, rows, header_name=None):
    """rows: list of tab-joined data lines written after the `...` marker."""
    body = [
        "# synthetic fixture",
        "---",
        f"name: {header_name or name}",
        'version: "0.0.0"',
        "sort: by_weight",
        "...",
    ] + list(rows)
    (Path(directory) / f"{name}.dict.yaml").write_text("\n".join(body) + "\n", encoding="utf-8")


class LexiconTiersTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)

    def audit(self, catalog=None):
        return tiers.audit_dir(self.root, catalog)

    def source(self, audit, sid):
        return next(s for s in audit["sources"] if s["id"] == sid)

    def test_flat_dict_is_flagged(self):
        write_dict(self.root, "flat", [
            "甲乙\ting yi\t200",
            "甲乙丙\ting yi er\t200",
            "甲乙丙丁\ting yi er san\t200",
            "甲乙丙丁戊\ting yi er san wu\t200",
        ])
        s = self.source(self.audit(), "flat")
        self.assertEqual(s["entries"], 4)
        self.assertEqual(s["distinct_weights"], 1)
        self.assertEqual(s["max_share"], 1.0)
        self.assertEqual(s["entropy_bits"], 0.0)
        self.assertTrue(s["flat"])

    def test_tiered_dict_stats(self):
        write_dict(self.root, "tiered", [
            "你好\tni hao\t100",
            "上山\tshan shang\t100",
            "快跑\tkuai pao\t100",
            "大爆炸\tda bao zha\t200",
            "小星星\txiao xing xing\t300",
            "幼儿园\tyou er yuan\t300",
            "春去秋来\tchun qu qiu lai\t300",
            "幼儿园小班\tyou er yuan xiao ban\t300",
        ])
        s = self.source(self.audit(), "tiered")
        self.assertEqual(s["entries"], 8)
        self.assertEqual(s["distinct_weights"], 3)
        self.assertAlmostEqual(s["max_share"], 0.5)
        # -(3/8)lg(3/8) - (1/8)lg(1/8) - (4/8)lg(4/8)
        self.assertAlmostEqual(s["entropy_bits"], 1.405639, places=5)
        self.assertAlmostEqual(s["share_2_4"], 0.875)  # 7 of 8 terms are 2-4 chars
        self.assertEqual(s["lengths"], {"1": 0, "2": 3, "3": 3, "4": 1, "5+": 1})
        self.assertFalse(s["flat"])

    def test_header_name_and_data_boundary(self):
        write_dict(self.root, "file_stem", [], header_name="declared_name")
        # 数据行之前的内容（---/元数据/注释）不计入条数；名字取自元数据而非文件名。
        audit = self.audit()
        self.assertEqual(sorted(s["id"] for s in audit["sources"]), ["declared_name"])
        self.assertEqual(self.source(audit, "declared_name")["entries"], 0)
        self.assertFalse(self.source(audit, "declared_name")["flat"])

    def test_aggregate_dict_has_imports(self):
        body = [
            "# aggregate",
            "---",
            "name: aggregate",
            "sort: by_weight",
            "import_tables:",
            "  - flat",
            "...",
        ]
        (self.root / "aggregate.dict.yaml").write_text("\n".join(body) + "\n", encoding="utf-8")
        write_dict(self.root, "flat", ["甲乙\tjia yi\t200"])
        s = self.source(self.audit(), "aggregate")
        self.assertEqual(s["entries"], 0)
        self.assertEqual(s["imports"], ["flat"])

    def test_malformed_rows_skipped_and_counted(self):
        write_dict(self.root, "messy", [
            "",
            "# comment inside data section",
            "只有一列",
            "两列\tno weight",
            "三列\tsan lie\t123",
        ])
        s = self.source(self.audit(), "messy")
        self.assertEqual(s["entries"], 1)
        self.assertEqual(s["skipped_rows"], 2)  # 空行与注释不算解析失败
        self.assertEqual(s["distinct_weights"], 1)

    def test_owner_words_are_not_read(self):
        write_dict(self.root, "agent_owner_words", ["私密\tsi mi\t999"])
        write_dict(self.root, "normal", ["你好\tni hao\t100"])
        audit = self.audit()
        self.assertEqual([s["id"] for s in audit["sources"]], ["normal"])
        self.assertEqual(audit["excluded"], ["agent_owner_words"])

    def test_catalog_rows_mismatch_reported(self):
        write_dict(self.root, "small", ["你好\tni hao\t100", "再见\tzai jian\t200"])
        catalog = self.root / "catalog.json"
        catalog.write_text(json.dumps({"layers": [{"id": "small", "group": "core", "rows": 99}]}),
                           encoding="utf-8")
        audit = self.audit(catalog)
        self.assertEqual(self.source(audit, "small")["catalog_rows"], 99)
        self.assertIn("small", audit["row_mismatches"])   # 99 vs 2
        self.assertNotIn("normal", audit["row_mismatches"])

    def test_flat_list_sorted_by_entries_desc(self):
        write_dict(self.root, "big_flat", [f"词{i:02d}\tci {i}\t200" for i in range(5)])
        write_dict(self.root, "small_flat", ["词\tci\t300"])
        audit = self.audit()
        self.assertEqual([s["id"] for s in audit["flat_sources"]], ["big_flat", "small_flat"])


if __name__ == "__main__":
    unittest.main()
