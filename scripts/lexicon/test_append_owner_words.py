"""Run with python3 -m unittest scripts.lexicon.test_append_owner_words -v.

append_owner_words runs in-process against a temporary Rime directory, with
build_general_lexicon.existing_entries stubbed to the same synthetic dictionary as
test_export_new_words.py (whichever file the discovery loads first installs the stub
that the cached export_new_words sees, so the two must stay identical). The shipped
layer, provenance and ime_context_model/ are never read.

BASELINE_DICT / BASELINE_PROVENANCE froze the pre-refactor exporter's output for the
same synthetic runs (captured 2026-10-28); extracting the owner-sourced merge rules
into shared functions must not change it by a byte.
"""

import contextlib
import hashlib
import io
import json
import sys
import tempfile
import types
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))

# identical to test_export_new_words.EXISTING: export_new_words caches whichever
# stub loads first, and both files' scenarios must see the same shipped dictionary
EXISTING = {
    "吃饭": [("chi fan", 500)],
    "天菜": [("tian cai", 300)],
    "恩那": [("en na", 350)],
    "似的": [("si de", 8000)],
}

_stub = types.ModuleType("build_general_lexicon")
_stub.existing_entries = lambda exclude_dictionaries=(): dict(EXISTING)
sys.modules["build_general_lexicon"] = _stub

import append_owner_words  # noqa: E402
import export_new_words  # noqa: E402
from export_new_words import OWNER_SOURCE  # noqa: E402

RUN_SOURCE = "deepseek-flash generation, 3-pass review"
HEADER_NOTE = append_owner_words.HEADER_NOTE

LAYER = ('# Agent Input Method new-word layer (generated, refreshed periodically; do not edit by hand)\n'
         '# Built by scripts/lexicon/gen_new_words.py + export_new_words.py; DeepSeek-listed, 3-pass reviewed.\n'
         '# Weights never exceed the existing first choice for the same reading.\n'
         '---\nname: agent_newwords\nversion: "0.1.0"\nsort: by_weight\n...\n\n'
         '恩深\ten shen\t250\n'
         '知有\tzhi you\t300\n'
         '子羽\tzi yu\t250\n')
LAYER_PROVENANCE = ('text\treading\tweight\tyear\tdomain\twiki_hits\tsynth_hits\tsource\tlicense\n'
                    '恩深\ten shen\t250\t2023\t社会\t0\t0\t' + RUN_SOURCE + '\tproject-authored\n'
                    '知有\tzhi you\t300\t2024\t文化\t2\t0\t' + RUN_SOURCE + '\tproject-authored\n'
                    '子羽\tzi yu\t250\t2023\t人\t0\t0\t' + RUN_SOURCE + '\tproject-authored\n')
# lines 9 冰霜: bng is not a pinyin syllable; line 10 冰箱柜: 2 syllables for 3 characters
OWNER = (
    "text\treading\toccurrences\treview\n"
    "知子\tzhi zi\t5\tclaude-2026-10-28\n"
    "小哲\txiao zhe\t1\tclaude-2026-10-28\n"
    "嗯呐\ten na\t8\tclaude-2026-10-28\n"
    "嗯深\ten shen\t3\tclaude-2026-10-28\n"
    "知有\tzhi you\t7\tclaude-2026-10-28\n"
    "吃饭\tchi fan\t9\tclaude-2026-10-28\n"
    "两千块\tliang qian kuai\t4\tclaude-2026-10-28\n"
    "冰霜\tbng shuang\t3\tclaude-2026-10-28\n"
    "冰箱柜\tbing xiang\t3\tclaude-2026-10-28\n"
)
EXPECTED_HEAD = ('# Agent Input Method new-word layer (generated, refreshed periodically; do not edit by hand)\n'
                 '# Built by scripts/lexicon/gen_new_words.py + export_new_words.py; DeepSeek-listed, 3-pass reviewed.\n'
                 '# Weights never exceed the existing first choice for the same reading.\n'
                 + HEADER_NOTE +
                 '---\nname: agent_newwords\nversion: "0.1.0"\nsort: by_weight\n...\n\n')
EXPECTED_DICT = EXPECTED_HEAD + (
    '嗯呐\ten na\t349\n'      # capped below the shipped 恩那 350
    '恩深\ten shen\t250\n'
    '嗯深\ten shen\t249\n'    # capped below the layer's own 恩深 250
    '小哲\txiao zhe\t250\n'   # 1 recorded occurrence
    '知有\tzhi you\t300\n'
    '知子\tzhi zi\t400\n'     # 5 recorded occurrences, after 知有 and before 子羽
    '子羽\tzi yu\t250\n')
OWNER_ROW_FMT = '{0}\t{1}\t{2}\towner\towner-gap\t0\t0\t' + OWNER_SOURCE + '\tproject-authored\n'
EXPECTED_PROVENANCE = ('text\treading\tweight\tyear\tdomain\twiki_hits\tsynth_hits\tsource\tlicense\n'
                      + OWNER_ROW_FMT.format('嗯呐', 'en na', 349)
                      + '恩深\ten shen\t250\t2023\t社会\t0\t0\t' + RUN_SOURCE + '\tproject-authored\n'
                      + OWNER_ROW_FMT.format('嗯深', 'en shen', 249)
                      + OWNER_ROW_FMT.format('小哲', 'xiao zhe', 250)
                      + '知有\tzhi you\t300\t2024\t文化\t2\t0\t' + RUN_SOURCE + '\tproject-authored\n'
                      + OWNER_ROW_FMT.format('知子', 'zhi zi', 400)
                      + '子羽\tzi yu\t250\t2023\t人\t0\t0\t' + RUN_SOURCE + '\tproject-authored\n')

EXPORT_RUNS = ("text\treading\twiki\tsynth\tyear\tdomain\tweight\n"
               "云原生\tyun yuan sheng\t5\t0\t2024\t科技\t\n"
               "吃饭\tchi fan\t9\t9\t2020\t生活\t\n"
               "天才\ttian cai\t3\t0\t2024\t文化\t\n")
EXPORT_OWNER = ("text\treading\toccurrences\treview\n"
                "知子\tzhi zi\t5\tclaude-2026-10-28\n"
                "嗯呐\ten na\t8\tclaude-2026-10-28\n"
                "吃饭\tchi fan\t9\tclaude-2026-10-28\n")
BASELINE_DICT = ('# Agent Input Method new-word layer (generated, refreshed periodically; do not edit by hand)\n'
                 '# Built by scripts/lexicon/gen_new_words.py + export_new_words.py; DeepSeek-listed, 3-pass reviewed.\n'
                 '# Weights never exceed the existing first choice for the same reading.\n'
                 '---\nname: agent_newwords\nversion: "0.1.0"\nsort: by_weight\n...\n\n'
                 '嗯呐\ten na\t349\n'
                 '天才\ttian cai\t299\n'
                 '云原生\tyun yuan sheng\t400\n'
                 '知子\tzhi zi\t400\n')
BASELINE_PROVENANCE = ('text\treading\tweight\tyear\tdomain\twiki_hits\tsynth_hits\tsource\tlicense\n'
                       + OWNER_ROW_FMT.format('嗯呐', 'en na', 349)
                       + '天才\ttian cai\t299\t2024\t文化\t3\t0\t' + RUN_SOURCE + '\tproject-authored\n'
                       + '云原生\tyun yuan sheng\t400\t2024\t科技\t5\t0\t' + RUN_SOURCE + '\tproject-authored\n'
                       + OWNER_ROW_FMT.format('知子', 'zhi zi', 400))


class AppendOwnerWordsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.rime = self.root / "Rime"
        self.rime.mkdir()
        self.dict_path = self.rime / "agent_newwords.dict.yaml"
        self.dict_path.write_text(LAYER, encoding="utf-8")
        self.prov_path = self.root / "agent-newwords-provenance.tsv"
        self.prov_path.write_text(LAYER_PROVENANCE, encoding="utf-8")
        self.owner = self.root / "owner.tsv"
        self.owner.write_text(OWNER, encoding="utf-8")

    def run_append(self, *extra):
        saved_argv, sys.argv = sys.argv, ["append_owner_words.py",
                                          "--owner-sourced", str(self.owner),
                                          "--rime", str(self.rime),
                                          "--provenance", str(self.prov_path), *extra]
        stdout, stderr = io.StringIO(), io.StringIO()
        try:
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                append_owner_words.main()
        finally:
            sys.argv = saved_argv
        return json.loads(stdout.getvalue()), stderr.getvalue()

    def test_appends_at_sorted_position(self):
        summary, _ = self.run_append()
        self.assertEqual(summary["appended"], 4)
        self.assertEqual(self.dict_path.read_text(encoding="utf-8"), EXPECTED_DICT)
        self.assertEqual(self.prov_path.read_text(encoding="utf-8"), EXPECTED_PROVENANCE)

    def test_existing_lines_unchanged(self):
        self.run_append()
        new_lines = self.dict_path.read_text(encoding="utf-8").splitlines(True)
        rest = iter(new_lines)
        # every original line survives verbatim, in order; HEADER_NOTE is the only added non-entry line
        for line in LAYER.splitlines(True):
            self.assertIn(line, new_lines)
            while next(rest) != line:
                pass
        self.assertEqual(new_lines[-1], "子羽\tzi yu\t250\n")

    def test_capped_below_first_choice(self):
        summary, _ = self.run_append()
        body = self.dict_path.read_text(encoding="utf-8")
        self.assertIn("嗯呐\ten na\t349\n", body)   # shipped 恩那 350
        self.assertIn("嗯深\ten shen\t249\n", body)  # the layer's own 恩深 250
        self.assertEqual(summary["stats"]["capped_below_first_choice"], 2)

    def test_published_words_skipped(self):
        summary, _ = self.run_append()
        body = self.dict_path.read_text(encoding="utf-8")
        self.assertEqual(body.count("知有\tzhi you\t300\n"), 1)  # already in the layer
        self.assertNotIn("吃饭\t", body)                          # already shipped
        self.assertNotIn("两千块\t", body)                        # spelled-out numbers
        self.assertEqual(summary["stats"]["already_shipped"], 2)
        self.assertEqual(summary["stats"]["spelled_out_numbers"], 1)

    def test_invalid_readings_skipped_and_counted(self):
        summary, stderr = self.run_append()
        body = self.dict_path.read_text(encoding="utf-8")
        self.assertNotIn("冰霜\t", body)
        self.assertNotIn("冰箱柜\t", body)
        self.assertEqual(summary["stats"]["owner_bad_syllable"], 1)
        self.assertEqual(summary["stats"]["owner_syllable_count"], 1)
        self.assertIn(f"{self.owner}:9: syllable not in the pinyin inventory", stderr)
        self.assertIn(f"{self.owner}:10: syllable count differs from the Han character count", stderr)

    def test_idempotent(self):
        summary_first, _ = self.run_append()
        self.assertEqual(summary_first["appended"], 4)
        dict_after_first = self.dict_path.read_text(encoding="utf-8")
        prov_after_first = self.prov_path.read_text(encoding="utf-8")
        summary_second, _ = self.run_append()
        self.assertEqual(summary_second["appended"], 0)
        self.assertEqual(summary_second["stats"]["already_shipped"], 6)
        self.assertEqual(self.dict_path.read_text(encoding="utf-8"), dict_after_first)
        self.assertEqual(self.prov_path.read_text(encoding="utf-8"), prov_after_first)
        self.assertEqual(summary_second["newwords_sha256"], summary_first["newwords_sha256"])
        self.assertEqual(summary_second["newwords_provenance_sha256"], summary_first["newwords_provenance_sha256"])

    def test_dry_run_writes_nothing(self):
        summary, _ = self.run_append("--dry-run")
        self.assertEqual(summary["appended"], 4)
        self.assertTrue(summary["dry_run"])
        self.assertEqual(self.dict_path.read_text(encoding="utf-8"), LAYER)
        self.assertEqual(self.prov_path.read_text(encoding="utf-8"), LAYER_PROVENANCE)
        # the reported sha256 values are the would-be ones: not the untouched file's, but exactly a real run's
        self.assertNotEqual(summary["newwords_sha256"], hashlib.sha256(LAYER.encode()).hexdigest())
        summary_real, _ = self.run_append()
        self.assertEqual(summary["newwords_sha256"], summary_real["newwords_sha256"])

    def test_provenance_stays_row_aligned(self):
        self.run_append()
        entries = [l.split("\t") for l in self.dict_path.read_text(encoding="utf-8").splitlines()[10:]]
        rows = [l.split("\t") for l in self.prov_path.read_text(encoding="utf-8").splitlines()[1:]]
        self.assertEqual(len(entries), len(rows))
        for entry, row in zip(entries, rows):
            self.assertEqual(entry, row[:3])

    def test_refuses_misaligned_provenance(self):
        self.prov_path.write_text(LAYER_PROVENANCE + OWNER_ROW_FMT.format('试词', 'shi ci', 250), encoding="utf-8")
        with self.assertRaises(RuntimeError):
            self.run_append()
        self.assertEqual(self.dict_path.read_text(encoding="utf-8"), LAYER)

    def test_export_output_unchanged(self):
        out = self.root / "out"
        out.mkdir()
        (self.root / "runs.tsv").write_text(EXPORT_RUNS, encoding="utf-8")
        (self.root / "export-owner.tsv").write_text(EXPORT_OWNER, encoding="utf-8")
        saved_argv, sys.argv = sys.argv, ["export_new_words.py", "--runs", str(self.root / "runs.tsv"),
                                          "--owner-sourced", str(self.root / "export-owner.tsv"), "--out", str(out)]
        try:
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                export_new_words.main()
        finally:
            sys.argv = saved_argv
        self.assertEqual((out / "agent_newwords.dict.yaml").read_text(encoding="utf-8"), BASELINE_DICT)
        self.assertEqual((out / "agent_newwords.provenance.tsv").read_text(encoding="utf-8"), BASELINE_PROVENANCE)


if __name__ == "__main__":
    unittest.main()
