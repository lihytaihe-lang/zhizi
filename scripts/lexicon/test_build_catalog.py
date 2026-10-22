"""Test catalog building with temporary dictionaries only (no real lexicon).

Run: cd scripts/lexicon && python3 -m unittest test_build_catalog -v
"""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("build_catalog.py")
SPEC = importlib.util.spec_from_file_location("build_catalog", SCRIPT)
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)


def write_dict(path, rows):
    """Write a small Rime dictionary; rows are (word, reading, weight) tuples.

    A comment line and a blank line are interleaved after the first data row
    so row counting has to skip them.
    """
    body = ["---", "name: " + path.name[:-len(".dict.yaml")], 'version: "0.0.1"',
            "sort: by_weight", "...", ""]
    for index, (word, reading, weight) in enumerate(rows):
        if index == 1:
            body.extend(["# comment inside the body", ""])
        body.append("\t".join((word, reading, str(weight))))
    body.extend(["", "# trailing comment"])
    path.write_text("\n".join(body) + "\n", encoding="utf-8")


class BuildCatalogTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.rime = self.root / "Rime"
        self.rime.mkdir()
        # Base (core). 明月 also lives in the pack layer below but must never
        # be listed as optional-only: the base dictionary is a core layer.
        self.base = self.root / "deps" / "pinyin_simp.dict.yaml"
        self.base.parent.mkdir()
        write_dict(self.base, [("明月", "ming yue", 1), ("你好", "ni hao", 1)])
        # Core: 你好, 世界.
        write_dict(self.rime / "agent_general.dict.yaml",
                   [("你好", "ni hao", 100), ("世界", "shi jie", 100)])
        # Extension: 世界, 一帆风顺 twice (exact duplicate row).
        write_dict(self.rime / "agent_idioms.dict.yaml",
                   [("世界", "shi jie", 10), ("一帆风顺", "yi fan feng shun", 240),
                    ("一帆风顺", "yi fan feng shun", 240)])
        # Pack: 一帆风顺, 床前明月光 twice with different readings, 明月 (in base).
        write_dict(self.rime / "agent_pack_poetry.dict.yaml",
                   [("一帆风顺", "yi fan feng shun", 200),
                    ("床前明月光", "chang qian ming yue guang", 200),
                    ("床前明月光", "chuang qian ming yue guang", 200),
                    ("明月", "ming yue", 200)])
        # Present in the Rime directory but deliberately absent from LAYERS.
        write_dict(self.rime / "agent_pinyin.dict.yaml", [("你好", "ni hao", 1)])
        write_dict(self.rime / "agent_custom.dict.yaml", [("自定义", "zi ding yi", 1)])

    def build(self, name="out"):
        out = self.root / name
        catalog, words = builder.build_catalog(self.rime, self.base, out)
        return catalog, out, words

    def test_membership_lists_optional_only_words(self):
        catalog, out, words = self.build()
        content = (out / "lexicon_membership.tsv").read_text(encoding="utf-8")
        self.assertEqual(content,
                         "一帆风顺\tagent_idioms,agent_pack_poetry\n"
                         "床前明月光\tagent_pack_poetry\n")
        self.assertEqual(words, 2)
        # Duplicated rows (same or different readings) list each word once.
        self.assertEqual([line.split("\t")[0] for line in content.splitlines()],
                         ["一帆风顺", "床前明月光"])
        for core_word in ("你好", "世界", "明月"):
            self.assertNotIn(core_word + "\t", content)

    def test_missing_base_fails_clearly(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--rime", str(self.rime),
             "--base", str(self.root / "nope.dict.yaml"),
             "--out", str(self.root / "o")],
            capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("base dictionary not found", result.stderr)
        self.assertFalse((self.root / "o").exists())

    def test_default_base_discovery(self):
        deps = self.root / ".build-deps" / "rime-pinyin-simp"
        with self.assertRaises(FileNotFoundError):
            builder.find_default_base(self.root)
        pinned = deps / "0c68" / "pinyin_simp.dict.yaml"
        pinned.parent.mkdir(parents=True)
        pinned.write_text("---\n...\n", encoding="utf-8")
        self.assertEqual(builder.find_default_base(self.root), pinned)
        other = deps / "aaa" / "pinyin_simp.dict.yaml"
        other.parent.mkdir()
        other.write_text("---\n...\n", encoding="utf-8")
        # Digits sort before letters, so the pinned "0c68" copy stays first.
        self.assertEqual(builder.find_default_base(self.root), pinned)

    def test_rows_count_only_data_lines(self):
        catalog, _, _ = self.build()
        self.assertEqual({layer["id"]: layer["rows"] for layer in catalog["layers"]},
                         {"pinyin_simp": 2, "agent_general": 2,
                          "agent_idioms": 3, "agent_pack_poetry": 4})

    def test_empty_optional_layer_is_treated_as_missing(self):
        # The public source tree ships the packs as header-only shells.
        write_dict(self.rime / "agent_pack_poetry.dict.yaml", [])
        write_dict(self.rime / "agent_general.dict.yaml", [])
        catalog, out, _ = self.build()
        ids = [layer["id"] for layer in catalog["layers"]]
        self.assertNotIn("agent_pack_poetry", ids)
        self.assertIn("agent_pack_poetry", catalog["missing"])
        self.assertIn("agent_general", ids, "an empty core layer stays listed")
        content = (out / "lexicon_membership.tsv").read_text(encoding="utf-8")
        self.assertNotIn("agent_pack_poetry", content)

    def test_unlisted_and_missing_layers(self):
        catalog, out, _ = self.build()
        self.assertEqual(catalog["formatVersion"], 1)
        self.assertEqual(catalog["unlisted"], ["agent_custom", "agent_pinyin"])
        self.assertEqual(catalog["missing"], [
            "agent_common_fix", "agent_daily", "agent_modern", "agent_spoken",
            "agent_newwords", "agent_thuocl_it", "agent_thuocl_caijing",
            "agent_ai_names", "agent_pack_cad", "agent_pack_industry"])
        self.assertEqual([layer["id"] for layer in catalog["layers"]],
                         ["pinyin_simp", "agent_general", "agent_idioms", "agent_pack_poetry"])
        poetry = catalog["layers"][3]
        self.assertEqual((poetry["name"], poetry["description"], poetry["group"],
                          poetry["defaultEnabled"]), ("诗词", "古诗词名句与诗人", "pack", False))
        self.assertEqual(list(catalog), ["formatVersion", "layers", "unlisted", "missing"])
        self.assertEqual([list(layer) for layer in catalog["layers"]],
                         [["id", "name", "description", "group", "defaultEnabled", "rows"]]
                         * len(catalog["layers"]))
        text = (out / "lexicon_catalog.json").read_text(encoding="utf-8")
        self.assertTrue(text.endswith("\n"))
        self.assertEqual(list(json.loads(text)), ["formatVersion", "layers", "unlisted", "missing"])
        self.assertEqual(json.loads(text), catalog)

    def test_reproducible_output(self):
        catalog, out_one, _ = self.build("one")
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--rime", str(self.rime),
             "--base", str(self.base), "--out", str(self.root / "two")],
            capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("lexicon_catalog.json", "lexicon_membership.tsv"):
            self.assertEqual((out_one / name).read_bytes(),
                             (self.root / "two" / name).read_bytes())


if __name__ == "__main__":
    unittest.main()
