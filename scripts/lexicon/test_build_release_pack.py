"""Test release packing with temporary dictionaries only.

Run: python3 -m unittest scripts.lexicon.test_build_release_pack -v
"""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("build_release_pack.py")
SPEC = importlib.util.spec_from_file_location("build_release_pack", SCRIPT)
packer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packer)


class ReleasePackTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.rime = self.root / "Rime"
        self.provenance = self.root / "corpus"
        self.rime.mkdir()
        self.provenance.mkdir()
        self.audit = self.root / "audit.md"
        self.audit.write_text(
            "# 审计\n| 层 | 条目数 | sha8 | 内容来源 | 用过大模型? | 许可状态 | 开源风险 |\n"
            "|---|---:|---|---|---|---|---|\n"
            "| `agent_test` | 2 | `1234` | 手工整理（来源原文） | 否 | 项目自有（待确认） | 低 |\n",
            encoding="utf-8")
        self.original = ("# fixture\r\n---\r\nname: agent_test\r\n...\r\n"
                         "\r\n# comment\r\n你好\tni hao\t100\r\n世界\tshi jie\t20\r\n").encode()
        (self.rime / "agent_test.dict.yaml").write_bytes(self.original)
        (self.provenance / "agent-test-provenance.tsv").write_text(
            "text\tsource\n你好\t手工\n世界\t手工\n", encoding="utf-8")

    def build(self, out="out", layers=None, **kwargs):
        return packer.build_release_pack(self.rime, self.provenance, "2026.10",
                                         self.root / out, layers, self.audit, **kwargs)

    def pack_fixture(self):
        """Add a fixture base, a core layer and two downloadable packs.

        明月 and 世界 also live in the base/core layers, so pack membership must
        drop them; 一帆风顺 is shared by both packs and must appear in each
        pack's own membership with that pack's id. agent_ai_names is absent
        from the catalog fixture on purpose (name/description/rows fall back).
        """
        base = self.root / "deps" / "pinyin_simp.dict.yaml"
        base.parent.mkdir()
        base.write_text("---\nname: pinyin_simp\n...\n"
                        "明月\tming yue\t1\n你好\tni hao\t1\n", encoding="utf-8")
        (self.rime / "agent_general.dict.yaml").write_text(
            "---\nname: agent_general\n...\n"
            "你好\tni hao\t100\n世界\tshi jie\t100\n", encoding="utf-8")
        (self.rime / "agent_pack_poetry.dict.yaml").write_text(
            "---\nname: agent_pack_poetry\n...\n"
            "一帆风顺\tyi fan feng shun\t200\n"
            "床前明月光\tchang qian ming yue guang\t200\n"
            "明月\tming yue\t200\n世界\tshi jie\t200\n", encoding="utf-8")
        (self.rime / "agent_ai_names.dict.yaml").write_text(
            "---\nname: agent_ai_names\n...\n一帆风顺\tyi fan feng shun\t50\n", encoding="utf-8")
        catalog = self.root / "catalog.json"
        catalog.write_text(json.dumps({"formatVersion": 1, "layers": [
            {"id": "agent_pack_poetry", "name": "诗词", "description": "古诗词名句与诗人",
             "group": "pack", "defaultEnabled": False, "rows": 14880}]},
            ensure_ascii=False), encoding="utf-8")
        return base, catalog

    def test_ditto_cells_resolve_to_the_row_above(self):
        self.audit.write_text(
            "| 层 | 条目数 | sha8 | 内容来源 | 用过大模型? | 许可状态 | 开源风险 |\n"
            "|---|---:|---|---|---|---|---|\n"
            "| `agent_a` | 1 | `1` | 网页核实 | 未核实 | facts-only | 低 |\n"
            "| `agent_b` | 1 | `2` | 同上（中文名） | 同上 | 同上 | 同上 |\n", encoding="utf-8")
        descriptions = packer.audit_descriptions(self.audit)
        self.assertEqual(descriptions["agent_b"], ("网页核实（中文名）", "facts-only"))

    def manifest(self, directory):
        return json.loads((directory / "MANIFEST.json").read_text(encoding="utf-8"))

    def test_contents_and_checksums(self):
        directory = self.build()
        layer = self.manifest(directory)["layers"][0]
        self.assertEqual(layer, {
            "name": "agent_test", "rows": 2,
            "sha256": hashlib.sha256(self.original).hexdigest(),
            "license": "项目自有（待确认）", "source": "手工整理（来源原文）"})
        self.assertEqual((directory / "layers/agent_test.dict.yaml").read_bytes(), self.original)
        self.assertEqual((directory / "plain/agent_test.tsv").read_text(),
                         "你好\tni hao\t100\n世界\tshi jie\t20\n")
        sums = (directory / "SHA256SUMS").read_text().splitlines()
        paths = set()
        for line in sums:
            digest, relative = line.split("  ", 1)
            paths.add(relative)
            self.assertEqual(hashlib.sha256((directory / relative).read_bytes()).hexdigest(), digest)
        self.assertEqual(paths, {p.relative_to(directory).as_posix()
                                 for p in directory.rglob("*") if p.is_file()} - {"SHA256SUMS"})
        readme = (directory / "README.md").read_text()
        for expected in ("2026.10", "agent_test", "import_tables", "尚未提供 plum 配方",
                         "项目自有（待确认）"):
            self.assertIn(expected, readme)
        with tarfile.open(directory.parent / (directory.name + ".tar.gz")) as archive:
            for member in archive:
                self.assertEqual((member.mtime, member.uid, member.gid), (0, 0, 0))
                self.assertEqual((member.uname, member.gname), ("", ""))
                self.assertEqual(member.mode, 0o755 if member.isdir() else 0o644)
            self.assertEqual({m.name for m in archive if m.isfile()},
                             {directory.name + "/" + p for p in paths | {"SHA256SUMS"}})

    def test_missing_provenance_is_todo(self):
        (self.provenance / "agent-test-provenance.tsv").unlink()
        layer = self.manifest(self.build())["layers"][0]
        self.assertEqual((layer["source"], layer["license"]), ("TODO", "TODO"))

    def test_aliased_provenance_and_empty_import_layers_are_documented(self):
        self.audit.write_text(self.audit.read_text(encoding="utf-8") +
            "| `agent_idioms` | 1 | `5678` | THUOCL 成语表（MIT） | 否 | MIT，明确允许 | 低 |\n"
            "| `agent_pinyin`（聚合导入表） | 0 | `9abc` | 项目自有 | 否 | 项目自有 | 低 |\n", encoding="utf-8")
        (self.rime / "agent_idioms.dict.yaml").write_bytes(b"---\nname: agent_idioms\n...\n\n\xe4\xb8\x80\xe5\xb8\x86\xe9\xa3\x8e\xe9\xa1\xba\tyi fan feng shun\t200\n")
        (self.rime / "agent_pinyin.dict.yaml").write_bytes(b"---\nname: agent_pinyin\nimport_tables:\n  - agent_test\n...\n")
        (self.provenance / "thuocl-idiom-provenance.tsv").write_text("text\tsource\n", encoding="utf-8")
        layers = {layer["name"]: layer for layer in self.manifest(self.build())["layers"]}
        self.assertEqual(layers["agent_idioms"]["license"], "MIT，明确允许")
        self.assertEqual(layers["agent_pinyin"]["license"], "项目自有")

    def test_missing_audit_layer_is_todo(self):
        self.audit.write_text("# 无说明\n", encoding="utf-8")
        layer = self.manifest(self.build())["layers"][0]
        self.assertEqual((layer["source"], layer["license"]), ("TODO", "TODO"))

    def test_empty_license_preserves_exact_source(self):
        self.audit.write_text(self.audit.read_text(encoding="utf-8").replace(
            "项目自有（待确认）", "").replace("手工整理（来源原文）", "**手工**（来源原文）"), encoding="utf-8")
        layer = self.manifest(self.build())["layers"][0]
        self.assertEqual(layer["license"], "TODO")
        self.assertEqual(layer["source"], "**手工**（来源原文）")

    def test_successful_cli(self):
        result = subprocess.run([sys.executable, str(SCRIPT), "--rime", str(self.rime),
                                 "--provenance", str(self.provenance), "--version", "2026.10",
                                 "--out", str(self.root / "out"), "--layers", "agent_test"],
                                cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        directory = self.root / "out/zhizi-lexicon-2026.10"
        self.assertTrue((directory / "SHA256SUMS").is_file())
        self.assertTrue(directory.with_name(directory.name + ".tar.gz").is_file())

    def test_missing_audit_file_is_todo(self):
        self.audit.unlink()
        self.assertEqual(self.manifest(self.build())["layers"][0]["license"], "TODO")

    def test_reproducible_archive_and_sorted_layers(self):
        (self.rime / "agent_aaa.dict.yaml").write_bytes(b"---\n...\n")
        first = self.build("one")
        (self.rime / "agent_test.dict.yaml").chmod(0o600)
        second = self.build("two", ["agent_test", "agent_aaa"])
        self.assertEqual(first.with_name(first.name + ".tar.gz").read_bytes(),
                         second.with_name(second.name + ".tar.gz").read_bytes())
        self.assertEqual([r["name"] for r in self.manifest(first)["layers"]],
                         ["agent_aaa", "agent_test"])

    def test_explicit_layers(self):
        (self.rime / "agent_unused.dict.yaml").write_text("invalid")
        self.assertEqual(len(self.manifest(self.build(layers=["agent_test"]))["layers"]), 1)

    def test_invalid_row_cli_fails(self):
        (self.rime / "agent_test.dict.yaml").write_text("---\n...\n坏行\tbad\n", encoding="utf-8")
        result = subprocess.run([sys.executable, str(SCRIPT), "--rime", str(self.rime),
                                 "--provenance", str(self.provenance), "--version", "2026.10",
                                 "--out", str(self.root / "out")], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("agent_test.dict.yaml:3", result.stderr)
        self.assertIn("expected 3 tab-separated columns", result.stderr)
        self.assertFalse((self.root / "out/zhizi-lexicon-2026.10").exists())

    def test_missing_separator_fails(self):
        (self.rime / "agent_test.dict.yaml").write_text("---\nname: test\n")
        with self.assertRaisesRegex(ValueError, "missing.*separator"):
            self.build()

    def test_unsafe_names_fail(self):
        for version in ("../escape", "", "a/b"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                packer.build_release_pack(self.rime, self.provenance, version,
                                          self.root / "out", audit_path=self.audit)
        with self.assertRaises(ValueError):
            self.build(layers=["../agent_test"])

    def test_existing_output_is_preserved(self):
        directory = self.build()
        before = (directory / "MANIFEST.json").read_bytes()
        with self.assertRaises(FileExistsError):
            self.build()
        self.assertEqual((directory / "MANIFEST.json").read_bytes(), before)

    def test_pack_index_fields_sorting_and_checksums(self):
        base, catalog = self.pack_fixture()
        directory = self.build(base=base, catalog_path=catalog)
        index = json.loads((directory / "packs/packs.json").read_text(encoding="utf-8"))
        self.assertEqual(list(index), ["formatVersion", "lexiconVersion", "packs"])
        self.assertEqual((index["formatVersion"], index["lexiconVersion"]), (1, "2026.10"))
        self.assertEqual([entry["id"] for entry in index["packs"]],
                         ["agent_ai_names", "agent_pack_poetry"])
        names = {entry["id"]: entry for entry in index["packs"]}
        self.assertEqual(names["agent_pack_poetry"]["name"], "诗词")
        self.assertEqual(names["agent_pack_poetry"]["description"], "古诗词名句与诗人")
        self.assertEqual(names["agent_pack_poetry"]["rows"], 14880)
        # Not in the catalog: id as name, empty description, dictionary data rows.
        self.assertEqual(names["agent_ai_names"]["name"], "agent_ai_names")
        self.assertEqual(names["agent_ai_names"]["description"], "")
        self.assertEqual(names["agent_ai_names"]["rows"], 1)
        for entry in index["packs"]:
            for key in ("dict", "membership"):
                part = entry[key]
                data = (directory / "packs" / part["file"]).read_bytes()
                self.assertEqual(part["bytes"], len(data))
                self.assertEqual(part["sha256"], hashlib.sha256(data).hexdigest())
                self.assertEqual(part["file"], entry["id"] + (".dict.yaml" if key == "dict"
                                                              else ".membership.tsv"))
        sums = (directory / "SHA256SUMS").read_text().splitlines()
        packed = {line.split("  ", 1)[1] for line in sums if line.split("  ", 1)[1].startswith("packs/")}
        self.assertEqual(packed, {"packs/packs.json", "packs/agent_ai_names.dict.yaml",
                                  "packs/agent_ai_names.membership.tsv",
                                  "packs/agent_pack_poetry.dict.yaml",
                                  "packs/agent_pack_poetry.membership.tsv"})

    def test_pack_membership_lists_pack_only_words(self):
        base, catalog = self.pack_fixture()
        directory = self.build(base=base, catalog_path=catalog)
        poetry = (directory / "packs/agent_pack_poetry.membership.tsv").read_text(encoding="utf-8")
        self.assertEqual(poetry,
                         "一帆风顺\tagent_pack_poetry\n床前明月光\tagent_pack_poetry\n")
        names = (directory / "packs/agent_ai_names.membership.tsv").read_text(encoding="utf-8")
        # Shared with the poetry pack but listed under its own pack id.
        self.assertEqual(names, "一帆风顺\tagent_ai_names\n")
        for core_word in ("你好", "世界", "明月"):
            self.assertNotIn(core_word + "\t", poetry)

    def test_pack_build_is_byte_identical_when_repeated(self):
        base, catalog = self.pack_fixture()
        first = self.build("pack-one", base=base, catalog_path=catalog)
        second = self.build("pack-two", base=base, catalog_path=catalog)
        for name in ("packs/packs.json", "packs/agent_ai_names.membership.tsv",
                     "packs/agent_pack_poetry.membership.tsv"):
            self.assertEqual((first / name).read_bytes(), (second / name).read_bytes())
        self.assertEqual(first.with_name(first.name + ".tar.gz").read_bytes(),
                         second.with_name(second.name + ".tar.gz").read_bytes())

    def test_pack_dictionaries_are_byte_copies_of_layers(self):
        base, catalog = self.pack_fixture()
        directory = self.build(base=base, catalog_path=catalog)
        for pack_id in ("agent_ai_names", "agent_pack_poetry"):
            self.assertEqual((directory / f"packs/{pack_id}.dict.yaml").read_bytes(),
                             (directory / f"layers/{pack_id}.dict.yaml").read_bytes())

if __name__ == "__main__":
    unittest.main()
