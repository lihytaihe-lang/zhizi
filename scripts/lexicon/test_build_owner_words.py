"""个人词层测试：只使用临时目录里的合成事件和词典。"""
import importlib.util
import itertools
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("build_owner_words.py")
SPEC = importlib.util.spec_from_file_location("build_owner_words", SCRIPT)
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)


def write_dict(path, rows):
    path.write_text("---\nname: synthetic\nsort: by_weight\n...\n"
                    + "".join(f"{word}\t{reading}\t100\n" for word, reading in rows),
                    encoding="utf-8")


def event(word="甲乙", pinyin="jiayi", **changes):
    result = dict(outcome="committed_candidate", selectedText=word, pinyin=pinyin,
                  selectedIndex=1, recordedAtMs=1000, deletedAfterCommit=0,
                  redoAfterCommit=False)
    result.update(changes)
    return result


class BuildOwnerWordsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.dicts = self.root / "dicts"
        self.dicts.mkdir()
        # 单字读音也可以来自逐字对齐的多字词条；重复来源不算读音歧义。
        write_dict(self.dicts / "first.dict.yaml",
                   [("甲", "jia"), ("乙", "yi"), ("丙丁", "bing ding"),
                    ("的", "de"), ("地", "di"), ("得", "de"), ("甲丙", "jia bing")])
        write_dict(self.dicts / "second.dict.yaml", [("甲", "jia"), ("乙丁", "yi ding")])
        write_dict(self.dicts / "pinyin_simp.dict.yaml", [])
        self.events = self.root / "events.jsonl"
        self.out = self.root / "user"

    def select(self, events, before=None):
        known, readings = builder.read_dictionaries(self.dicts)
        return builder.select_words(events, known, readings, before)

    def cli(self, events, *args):
        self.events.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n"
                                      for row in events), encoding="utf-8")
        return subprocess.run([sys.executable, str(SCRIPT), "--events", str(self.events),
                               "--dict-dir", str(self.dicts), "--out", str(self.out), *args],
                              capture_output=True, text=True)

    def paths(self):
        return (self.out / "agent_owner_words.dict.yaml",
                self.out / "lexicon_packs/agent_owner_words.membership.tsv")

    def test_selection_conditions(self):
        self.assertEqual(self.select([event()]), [("甲乙", "jia yi", 1000)])
        for changes in [dict(outcome="committed_raw"), dict(outcome="abandoned"),
                        dict(selectedIndex=0), dict(selectedIndex=-1), dict(selectedIndex=None),
                        dict(selectedIndex=True), dict(segments=[], selectedIndex=0),
                        dict(deletedAfterCommit=1), dict(redoAfterCommit=True)]:
            with self.subTest(changes=changes):
                self.assertEqual(self.select([event(**changes)]), [])
        self.assertEqual(self.select([event(selectedIndex=0, segments=[{"text": "甲"}])]),
                         [("甲乙", "jia yi", 1000)])
        for deleted in [None, 0, -1]:
            self.assertTrue(self.select([event(deletedAfterCommit=deleted)]))
        historical = event()
        del historical["deletedAfterCommit"]
        del historical["redoAfterCommit"]
        self.assertTrue(self.select([historical]))

    def test_hanzi_length_and_existing_words_in_every_layer(self):
        for word in ["", "甲", "甲乙丙丁甲乙丙", "甲a", "甲1", "甲，", "甲 乙", "甲丙", "乙丁"]:
            with self.subTest(word=word):
                self.assertEqual(self.select([event(word)]), [])
        self.assertEqual(self.select([event("甲乙丙丁甲乙", "jiayibingdingjiayi")]),
                         [("甲乙丙丁甲乙", "jia yi bing ding jia yi", 1000)])

    def test_excludes_de_di_de_suffixes(self):
        for word, pinyin in [("甲的", "jiade"), ("甲地", "jiadi"), ("甲得", "jiade")]:
            self.assertEqual(self.select([event(word, pinyin)]), [])

    def test_only_one_exact_reading_is_accepted(self):
        self.assertEqual(self.select([event("丙乙", "bingyi")]), [("丙乙", "bing yi", 1000)])
        for pinyin in ["jy", "jiay", "jiayia", "jia yi", "jia'yi", "JIAYI", ""]:
            self.assertEqual(self.select([event(pinyin=pinyin)]), [])
        self.assertEqual(self.select([event("甲戊", "jiawu")]), [])
        # 两种逐字读音都能拼成 xian，必须跳过，不能任选一种。
        write_dict(self.dicts / "ambiguous.dict.yaml",
                   [("戊", "xi"), ("戊", "xia"), ("己", "an"), ("己", "n")])
        self.assertEqual(self.select([event("戊己", "xian")]), [])

    def test_misaligned_dictionary_rows_do_not_supply_readings(self):
        write_dict(self.dicts / "unaligned.dict.yaml", [("戊己", "wu"), ("庚辛", "geng xin extra")])
        self.assertEqual(self.select([event("甲戊", "jiawu"), event("甲庚", "jiageng")]), [])

    def test_occurrences_set_the_weight(self):
        rows = self.select([event(), event(), event(), event(redoAfterCommit=True)])
        self.assertEqual(rows, [("甲乙", "jia yi", 2000)])

    def test_weight_counts_the_word_across_unique_full_readings(self):
        write_dict(self.dicts / "polyphonic.dict.yaml", [("甲", "ga")])
        self.assertEqual(self.select([event(), event(pinyin="gayi")]),
                         [("甲乙", "ga yi", 1500), ("甲乙", "jia yi", 1500)])

    def test_before_uses_strict_local_midnight(self):
        cut = builder.before_ms("2026-10-19")
        self.assertEqual(cut, int(time.mktime((2026, 10, 19, 0, 0, 0, 0, 0, -1)) * 1000))
        rows = self.select([event(recordedAtMs=cut - 1), event(recordedAtMs=cut),
                            event(recordedAtMs=cut + 1), event(recordedAtMs=None)], cut)
        self.assertEqual(rows, [("甲乙", "jia yi", 1000)])
        with self.assertRaises(ValueError):
            builder.before_ms("2026-02-30")
        result = self.cli([event(recordedAtMs=cut - 1), event(recordedAtMs=cut)],
                          "--before", "2026-10-19")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("甲乙\tjia yi\t1000\n", self.paths()[0].read_text(encoding="utf-8"))

    def test_private_atomic_replacement_and_complete_output(self):
        self.assertEqual(self.cli([event()]).returncode, 0)
        dictionary, membership = self.paths()
        old = {path: path.read_bytes() for path in self.paths()}
        original_replace = os.replace
        replacements = []

        def replace(source, destination):
            source, destination = Path(source), Path(destination)
            self.assertNotEqual(source, destination)
            self.assertEqual(source.parent, destination.parent)
            metadata = source.lstat()
            self.assertTrue(stat.S_ISREG(metadata.st_mode))
            self.assertEqual(stat.S_IMODE(metadata.st_mode), 0o600)
            self.assertEqual(metadata.st_nlink, 1)
            self.assertEqual(metadata.st_uid, os.geteuid())
            self.assertEqual(destination.read_bytes(), old[destination])
            replacements.append(destination)
            original_replace(source, destination)

        with patch.object(builder.os, "replace", side_effect=replace):
            builder.write_outputs(self.out, [("丙乙", "bing yi", 1500)])
        self.assertEqual(set(replacements), {dictionary, membership})
        text = dictionary.read_text(encoding="utf-8")
        self.assertIn("name: agent_owner_words\n", text)
        self.assertIn("sort: by_weight\n", text)
        self.assertRegex(text, r'version: "\d{4}-\d{2}-\d{2}T')
        self.assertIn("丙乙\tbing yi\t1500\n", text)
        self.assertNotIn("甲乙", text)
        self.assertEqual(membership.read_text(encoding="utf-8"), "丙乙\tagent_owner_words\n")
        self.assertEqual(stat.S_IMODE(membership.parent.stat().st_mode), 0o700)
        self.assertEqual(sorted(path.name for path in self.out.iterdir()),
                         ["agent_owner_words.dict.yaml", "lexicon_packs"])
        self.assertEqual(list(membership.parent.iterdir()), [membership])

    def test_failed_replace_preserves_files_and_removes_temporaries(self):
        self.assertEqual(self.cli([event()]).returncode, 0)
        originals = {path: path.read_bytes() for path in self.paths()}
        with patch.object(builder.os, "replace", side_effect=OSError("synthetic failure")):
            with self.assertRaises(OSError):
                builder.write_outputs(self.out, [("丙乙", "bing yi", 1000)])
        for path, contents in originals.items():
            self.assertEqual(path.read_bytes(), contents)
        self.assertEqual(len(list(self.out.rglob("*"))), 3)

    def test_empty_selection_deletes_both_files(self):
        self.assertEqual(self.cli([event()]).returncode, 0)
        other = self.out / "unrelated"
        other.write_text("keep", encoding="utf-8")
        self.assertEqual(self.cli([]).returncode, 0)
        self.assertTrue(all(not path.exists() for path in self.paths()))
        self.assertEqual(other.read_text(encoding="utf-8"), "keep")
        builder.write_outputs(self.root / "absent", [])
        self.assertFalse((self.root / "absent").exists())

    def test_rejects_symlink_directory_and_targets(self):
        outside = self.root / "outside"
        outside.mkdir()
        self.out.mkdir()
        packs = self.out / "lexicon_packs"
        packs.symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            builder.write_outputs(self.out, [("甲乙", "jia yi", 1000)])
        self.assertEqual(list(outside.iterdir()), [])
        packs.unlink()
        packs.mkdir()
        target = outside / "target"
        target.write_text("keep", encoding="utf-8")
        self.paths()[0].symlink_to(target)
        for rows in [[], [("甲乙", "jia yi", 1000)]]:
            with self.assertRaises(ValueError):
                builder.write_outputs(self.out, rows)
        self.assertEqual(target.read_text(encoding="utf-8"), "keep")

    def test_runtime_limits_are_checked_before_writing(self):
        for limit in ["MAX_PACK_BYTES", "MAX_DICTIONARY_BYTES", "MAX_PACK_ENTRIES"]:
            with self.subTest(limit=limit), patch.object(builder, limit, 0):
                with self.assertRaises(ValueError):
                    builder.write_outputs(self.out, [("甲乙", "jia yi", 1000)])
                self.assertFalse(self.out.exists())

    def test_shared_shell_is_empty_and_imported_before_base(self):
        rime = SCRIPT.resolve().parents[2] / "native/resources/Rime"
        shell = (rime / "agent_owner_words.dict.yaml").read_text(encoding="utf-8")
        self.assertIn("name: agent_owner_words\n", shell)
        self.assertIn("sort: by_weight\n", shell)
        self.assertEqual(shell.split("...", 1)[1].strip(), "")
        aggregate = (rime / "agent_pinyin.dict.yaml").read_text(encoding="utf-8")
        self.assertIn("  - agent_owner_words\n  - pinyin_simp\n", aggregate)

    def test_existing_public_permissions_are_replaced_with_private_files(self):
        self.assertEqual(self.cli([event()]).returncode, 0)
        for path in self.paths():
            path.chmod(0o644)
        self.paths()[1].parent.chmod(0o755)
        builder.write_outputs(self.out, [("甲乙", "jia yi", 1500)])
        for path in self.paths():
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(self.paths()[1].parent.stat().st_mode), 0o700)

    def test_dry_run_only_prints_count_and_first_fifty_words(self):
        rows = [event("甲" * n + "乙" * (6 - n), "jia" * n + "yi" * (6 - n)) for n in range(1, 6)]
        result = self.cli(rows, "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("5", result.stdout.splitlines()[0])
        self.assertEqual(set(result.stdout.splitlines()[1:]), {row["selectedText"] for row in rows})
        self.assertFalse(self.out.exists())
        # 六个位置、每个两种字，共 64 个合成词。
        rows = [event("".join(chars), "".join("jia" if char == "甲" else "yi" for char in chars))
                for chars in itertools.product("甲乙", repeat=6)]
        result = self.cli(rows, "--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("64", result.stdout.splitlines()[0])
        self.assertEqual(len(result.stdout.splitlines()), 51)



class BaseTableGuardTest(unittest.TestCase):
    def test_a_directory_without_the_base_table_is_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            write_dict(Path(temp) / "agent_daily.dict.yaml", [("甲", "jia")])
            with self.assertRaisesRegex(ValueError, "pinyin_simp"):
                builder.read_dictionaries(temp)

if __name__ == "__main__":
    unittest.main()
