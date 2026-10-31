"""Run with python3 -m unittest scripts.lexicon.test_export_new_words -v.

The exporter runs in-process with build_general_lexicon.existing_entries stubbed to a
small synthetic dictionary, so tests never read the shipped dictionaries or
ime_context_model/. BASELINE_* froze the pre-owner-sourced output for the same
synthetic runs (captured 2026-10-28 from the unmodified script); without
--owner-sourced the exporter must reproduce it byte for byte.
"""

import contextlib
import io
import json
import sys
import tempfile
import types
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))

EXISTING = {
    "吃饭": [("chi fan", 500)],
    "天菜": [("tian cai", 300)],
    "恩那": [("en na", 350)],
    "似的": [("si de", 8000)],
}

_stub = types.ModuleType("build_general_lexicon")
_stub.existing_entries = lambda exclude_dictionaries=(): dict(EXISTING)
sys.modules["build_general_lexicon"] = _stub

import export_new_words  # noqa: E402

RUN_SOURCE = "deepseek-flash generation, 3-pass review"
OWNER_SOURCE = "owner input records (owner-approved for public use), DeepSeek-reviewed, human-checked"

RUNS1 = (
    "text\treading\twiki\tsynth\tyear\tdomain\tweight\n"
    "云原生\tyun yuan sheng\t5\t0\t2024\t科技\t\n"
    "空灵\tkong ling\t0\t0\t2023\t文化\t\n"
    "麒麟九千零一\tqi lin jiu ling ling yi\t2\t0\t2024\t科技\t\n"
    "吃饭\tchi fan\t9\t9\t2020\t生活\t\n"
    "天才\ttian cai\t3\t0\t2024\t文化\t\n"
    "私域\tsi yu\t0\t0\t2024\t商业\t500\n"
    "丹参\tdan shen\t2\t0\t2023\t植物\t\n"
    "行吧\txing ba\t0\t0\t2024\t口语\t\n"
)
RUNS2 = (
    "text\treading\twiki\tsynth\tyear\tdomain\tweight\n"
    "云原生\tyun yuan sheng\t1\t1\t2024\t科技\t\n"
)
# line 9 冰霜: bng is not a pinyin syllable; line 10 冰箱柜: 2 syllables for 3 characters
OWNER = (
    "text\treading\toccurrences\treview\n"
    "知子\tzhi zi\t5\tclaude-2026-10-28\n"
    "小哲\txiao zhe\t1\tclaude-2026-10-28\n"
    "嘎嘎\tga ga\t3\tclaude-2026-10-28\n"
    "云原生\tyun yuan sheng\t9\tclaude-2026-10-28\n"
    "恩那\ten na\t6\tclaude-2026-10-28\n"
    "两千块\tliang qian kuai\t4\tclaude-2026-10-28\n"
    "嗯呐\ten na\t8\tclaude-2026-10-28\n"
    "冰霜\tbng shuang\t3\tclaude-2026-10-28\n"
    "冰箱柜\tbing xiang\t3\tclaude-2026-10-28\n"
)

BASELINE_DICT_YAML = ('# Agent Input Method new-word layer (generated, refreshed periodically; do not edit by hand)\n'
                      '# Built by scripts/lexicon/gen_new_words.py + export_new_words.py; DeepSeek-listed, 3-pass reviewed.\n'
                      '# Weights never exceed the existing first choice for the same reading.\n'
                      '---\nname: agent_newwords\nversion: "0.1.0"\nsort: by_weight\n...\n\n'
                      '丹参\tdan shen\t400\n'
                      '空灵\tkong ling\t250\n'
                      '私域\tsi yu\t500\n'
                      '天才\ttian cai\t299\n'
                      '行吧\txing ba\t6000\n'
                      '云原生\tyun yuan sheng\t400\n')
BASELINE_PROVENANCE = ('text\treading\tweight\tyear\tdomain\twiki_hits\tsynth_hits\tsource\tlicense\n'
                       '丹参\tdan shen\t400\t2023\t植物\t2\t0\tdeepseek-flash generation, 3-pass review\tproject-authored\n'
                       '空灵\tkong ling\t250\t2023\t文化\t0\t0\tdeepseek-flash generation, 3-pass review\tproject-authored\n'
                       '私域\tsi yu\t500\t2024\t商业\t0\t0\tdeepseek-flash generation, 3-pass review\tproject-authored\n'
                       '天才\ttian cai\t299\t2024\t文化\t3\t0\tdeepseek-flash generation, 3-pass review\tproject-authored\n'
                       '行吧\txing ba\t6000\t2024\t口语\t0\t0\tdeepseek-flash generation, 3-pass review\tproject-authored\n'
                       '云原生\tyun yuan sheng\t400\t2024\t科技\t5\t0\tdeepseek-flash generation, 3-pass review\tproject-authored\n')
BASELINE_EXPORT_JSON = ('{\n "rows": 6,\n "sha256": "18aaef7099ac48035d036d8d8f727c6d29890d8fa79ee038312f2b61eb0fda2e",\n'
                        ' "stats": {\n  "spelled_out_numbers": 1,\n  "already_shipped": 1,\n  "capped_below_first_choice": 1,\n'
                        '  "duplicate_across_runs": 1\n },\n "by_year": {\n  "2023": 2,\n  "2024": 4\n },\n "with_evidence": 4\n}')


class ExportNewWordsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.runs = []
        for name, text in (("runs1.tsv", RUNS1), ("runs2.tsv", RUNS2)):
            path = self.root / name
            path.write_text(text, encoding="utf-8")
            self.runs.append(path)
        self.owner = self.root / "owner.tsv"
        self.owner.write_text(OWNER, encoding="utf-8")

    def run_export(self, owner=()):
        out = self.root / "out"
        out.mkdir(exist_ok=True)
        saved_argv, sys.argv = sys.argv, ["export_new_words.py",
                                          "--runs", *[str(p) for p in self.runs], "--out", str(out)]
        for path in owner:
            sys.argv += ["--owner-sourced", str(path)]
        stdout, stderr = io.StringIO(), io.StringIO()
        try:
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                export_new_words.main()
        finally:
            sys.argv = saved_argv
        summary = json.loads((out / "export.json").read_text(encoding="utf-8"))
        return out, summary, stdout.getvalue(), stderr.getvalue()

    def read(self, out, name):
        return (out / name).read_text(encoding="utf-8")

    def test_without_owner_sourced_matches_baseline(self):
        out, summary, _, _ = self.run_export()
        self.assertEqual(self.read(out, "agent_newwords.dict.yaml"), BASELINE_DICT_YAML)
        self.assertEqual(self.read(out, "agent_newwords.provenance.tsv"), BASELINE_PROVENANCE)
        self.assertEqual(summary.pop("owner_sourced"), 0)
        self.assertEqual(json.dumps(summary, ensure_ascii=False, indent=1), BASELINE_EXPORT_JSON)

    def test_header_only_owner_sourced_matches_baseline(self):
        empty = self.root / "empty.tsv"
        empty.write_text("text\treading\toccurrences\treview\n", encoding="utf-8")
        out, summary, _, _ = self.run_export(owner=[empty])
        self.assertEqual(self.read(out, "agent_newwords.dict.yaml"), BASELINE_DICT_YAML)
        self.assertEqual(self.read(out, "agent_newwords.provenance.tsv"), BASELINE_PROVENANCE)
        self.assertEqual(summary.pop("owner_sourced"), 0)
        self.assertEqual(json.dumps(summary, ensure_ascii=False, indent=1), BASELINE_EXPORT_JSON)

    def test_owner_rows_merge_with_weights(self):
        out, summary, _, _ = self.run_export(owner=[self.owner])
        body = self.read(out, "agent_newwords.dict.yaml")
        self.assertIn("知子\tzhi zi\t400\n", body)   # occurrences >= 3
        self.assertIn("嘎嘎\tga ga\t400\n", body)    # exactly 3 occurrences
        self.assertIn("小哲\txiao zhe\t250\n", body)  # occurrences < 3
        self.assertEqual(summary["rows"], 10)
        self.assertEqual(summary["owner_sourced"], 4)
        self.assertEqual(summary["with_evidence"], 6)  # 丹参 私域 行吧 云原生 (runs) + 知子 嘎嘎 (owner)
        self.assertEqual(summary["by_year"]["owner"], 4)

    def test_owner_capped_below_first_choice(self):
        out, summary, _, _ = self.run_export(owner=[self.owner])
        self.assertIn("嗯呐\ten na\t349\n", self.read(out, "agent_newwords.dict.yaml"))  # below 恩那 350
        self.assertEqual(summary["stats"]["capped_below_first_choice"], 2)  # 天才 (runs) + 嗯呐 (owner)

    def test_owner_merge_rules(self):
        out, summary, _, _ = self.run_export(owner=[self.owner])
        body = self.read(out, "agent_newwords.dict.yaml")
        self.assertNotIn("恩那\t", body)     # already shipped
        self.assertNotIn("两千块\t", body)   # spelled-out numbers
        self.assertNotIn("冰霜\t", body)
        self.assertNotIn("冰箱柜\t", body)
        self.assertEqual(summary["stats"]["already_shipped"], 2)  # 吃饭 (runs) + 恩那 (owner)
        self.assertEqual(summary["stats"]["spelled_out_numbers"], 2)  # 麒麟九千零一 + 两千块
        self.assertEqual(summary["stats"]["duplicate_across_runs"], 2)  # runs2 云原生 + owner 云原生

    def test_owner_provenance(self):
        out, _, _, _ = self.run_export(owner=[self.owner])
        lines = self.read(out, "agent_newwords.provenance.tsv").splitlines()
        owner_lines = [l for l in lines if "\towner\towner-gap\t" in l]
        self.assertEqual(len(owner_lines), 4)
        for line in owner_lines:
            self.assertTrue(line.endswith(f"\t0\t0\t{OWNER_SOURCE}\tproject-authored"), line)
        self.assertIn(f"知子\tzhi zi\t400\towner\towner-gap\t0\t0\t{OWNER_SOURCE}\tproject-authored", lines)
        # run rows keep the DeepSeek provenance
        self.assertIn(f"私域\tsi yu\t500\t2024\t商业\t0\t0\t{RUN_SOURCE}\tproject-authored", lines)

    def test_owner_reading_validation(self):
        out, summary, _, stderr = self.run_export(owner=[self.owner])
        self.assertEqual(summary["stats"]["owner_bad_syllable"], 1)
        self.assertEqual(summary["stats"]["owner_syllable_count"], 1)
        self.assertIn(f"{self.owner}:9:", stderr)
        self.assertIn(f"{self.owner}:10:", stderr)
        for secret in ("冰霜", "冰箱柜", "bng"):
            self.assertNotIn(secret, stderr)  # reasons carry line numbers only

    def test_multiple_owner_files(self):
        a = self.root / "a.tsv"
        b = self.root / "b.tsv"
        a.write_text("text\treading\toccurrences\treview\n知子\tzhi zi\t5\tclaude-2026-10-28\n", encoding="utf-8")
        b.write_text("text\treading\toccurrences\treview\n知子\tzhi zi\t9\tclaude-2026-10-28\n", encoding="utf-8")
        out, summary, _, _ = self.run_export(owner=[a, b])
        self.assertEqual(summary["owner_sourced"], 1)
        self.assertEqual(summary["stats"]["duplicate_across_runs"], 2)  # 云原生 (runs2) + 知子 (owner b.tsv)


if __name__ == "__main__":
    unittest.main()
