import json
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("public_export.py")


class PublicExportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.src = self.root / "repo"
        self.src.mkdir()
        self.out = self.root / "out"
        self.report = self.root / "report.json"
        self.git("init", "-q")
        self.config = self.root / "default.json"
        config = json.loads(SCRIPT.with_suffix(".json").read_text())
        config.pop("rename", None)
        config.pop("stub_dicts", None)
        self.config.write_text(json.dumps(config))

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.src), *args],
                              check=True, capture_output=True)

    def put(self, name, content="safe\n", tracked=True):
        path = self.src / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        if tracked:
            self.git("add", "--", name)
        return path

    def put_pack_dictionaries(self, rows=True):
        for name in json.loads(SCRIPT.with_suffix(".json").read_text())["stub_dicts"]:
            body = "# synthetic fixture\n---\nname: x\n...\n" + ("secret-row\tsecret row\t1\n" if rows else "")
            self.put(name, body)

    def run_cli(self, *options):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--src", str(self.src),
             "--out", str(self.out), "--report", str(self.report), "--config", str(self.config), *options],
            capture_output=True, text=True,
        )

    def test_excludes_and_skin_exceptions(self):
        names = ["src-tauri/src/test_upload.rs",
                 "native/host/src/bin/zhizi-upload.rs", "server/zhizi-ingest/index.js",
                 "src/target/a.rs", "src/node_modules/a.js", "native/build/a",
                 "scripts/a.exe", "scripts/a.dmg", "quality/golden/a",
                 "native/resources/Skins/Builtin/other/style.json",
                 "native/resources/Skins/custom/sub/a.png"]
        for name in names:
            self.put(name)
        kept = ["native/resources/Skins/Builtin/system/style.json",
                "native/resources/Skins/Builtin/xuanmo/style.json",
                "native/resources/Skins/Builtin/system/a.png",
                "native/resources/Skins/Builtin/xuanmo/sub/a.png",
                "quality/corpus/source-provenance-v1.tsv",
                "quality/corpus/base-500.json"]
        for name in kept:
            self.put(name)
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in names:
            self.assertFalse((self.out / name).exists(), name)
        for name in kept:
            self.assertTrue((self.out / name).is_file(), name)

    def test_all_free_skins_including_png_are_exported(self):
        free_skins = ["baitu", "chaiquan", "denglong", "fenghong", "juzimao",
                      "luoying", "nihong", "shuitun", "songxue", "system",
                      "xuanmo", "youzi", "yueye"]
        kept = ["native/resources/Skins/skin.schema.json"]
        for skin in free_skins:
            kept.extend(f"native/resources/Skins/Builtin/{skin}/{name}"
                        for name in ("skin.json", "sheet.png", "sub/frame.png"))
        for name in kept:
            source = self.put(name, "synthetic fixture\n")
            if name.endswith(".png"):
                source.write_bytes(b"\x89PNG\r\n\x1a\n\0synthetic fixture")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.report.read_text())["files"], sorted(kept))
        for name in kept:
            self.assertEqual((self.out / name).read_bytes(),
                             (self.src / name).read_bytes(), name)

    def test_unlisted_skins_inside_and_outside_builtin_are_excluded(self):
        excluded = [f"native/resources/Skins/{directory}/{name}"
                    for directory in ("Builtin/future-added-skin", "Paid/future-skin",
                                      "future-skin")
                    for name in ("skin.json", "sheet.png", "sub/frame.png")]
        for name in excluded:
            self.put(name, "synthetic fixture\n")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.report.read_text())["files"], [])
        for name in excluded:
            self.assertFalse((self.out / name).exists(), name)

    def test_project_license_files_are_exported(self):
        licenses = ["LICENSE", "LICENSE-LEXICON", "LICENSE-SKINS"]
        for name in licenses:
            self.put(name, f"Synthetic {name} license fixture\n")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.report.read_text())["files"], licenses)
        for name in licenses:
            self.assertEqual((self.out / name).read_bytes(),
                             (self.src / name).read_bytes(), name)

    def test_license_manifests_exported_but_internal_audits_not(self):
        kept = ["docs/current/VERSIONING.md",
                "docs/current/licenses/DEPENDENCY_REGISTRY.md",
                "docs/current/licenses/LOCKED_DEPENDENCIES.json",
                "docs/current/licenses/NATIVE_LICENSE_ASSETS.json"]
        for name in kept:
            self.put(name, "synthetic fixture\n")
        hidden = ["docs/current/licenses/LEXICON-LAYER-AUDIT-2026-10-16.md",
                  "docs/current/licenses/TRAINING-DATA-TERMS-2026-10-15.md",
                  "docs/current/licenses/NEW-INTERNAL-NOTE.md"]
        for name in hidden:
            self.put(name, "synthetic internal fixture\n")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.report.read_text())["files"], kept)
        for name in hidden:
            self.assertFalse((self.out / name).exists(), name)

    def test_private_seed_and_cleanup_excluded_even_with_broad_quality_include(self):
        config = json.loads(SCRIPT.with_suffix(".json").read_text())
        config["include"].append("quality/")
        config.pop("rename", None)
        config.pop("stub_dicts", None)
        config_path = self.root / "broad.json"
        config_path.write_text(json.dumps(config))
        private = ["quality/mined/personal-profile-v1.json",
                   "quality/mined/personal-profile-v1-seed.json",
                   "quality/corpus/learning-cleanup-evidence-v3.json",
                   "quality/corpus/learning-cleanup-evidence-future.json"]
        for name in private:
            self.put(name, "synthetic fixture")
        self.put("quality/corpus/base-500.json", "synthetic fixture")
        result = self.run_cli("--config", str(config_path))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.report.read_text())["files"],
                         ["quality/corpus/base-500.json"])
        for name in private:
            self.assertFalse((self.out / name).exists())

    def test_untracked_and_outside_include(self):
        self.put("src/untracked.py", tracked=False)
        self.put(".gitignore", "src/ignored.py\n")
        self.put("src/ignored.py", tracked=False)
        self.put("notes.txt")
        self.put("README.md")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.report.read_text())["file_count"], 1)
        self.assertFalse((self.out / "src/untracked.py").exists())
        self.assertFalse((self.out / "src/ignored.py").exists())
        self.assertFalse((self.out / "notes.txt").exists())

    def test_blockers_and_redacted_report(self):
        secrets = ["tmp/" + "zhizi-invite", "invite" + ".txt",
                   "sk-" + "A" * 16, "ghp_" + "B" * 20,
                   "hf_" + "C" * 20, "AKIA" + "D" * 12,
                   "/Users/" + "taihe", "/Volumes/" + "GF/"]
        self.put("src/example.txt", "\n".join(secrets))
        self.put("src/.env.local", "private")
        result = self.run_cli()
        self.assertEqual(result.returncode, 1, result.stderr)
        report_text = self.report.read_text()
        report = json.loads(report_text)
        self.assertEqual(len(report["blocker"]), 9)
        for secret in secrets:
            self.assertNotIn(secret, report_text)
            self.assertNotIn(secret, result.stdout + result.stderr)
        self.assertEqual([hit["line"] for hit in report["blocker"]
                          if hit["file"] == "src/example.txt"], list(range(1, 9)))
        self.assertFalse(self.out.exists())

    def test_only_exact_synthetic_tokens_are_allowed(self):
        # Split fixtures so this test does not itself add scanned token literals.
        allowed = ["sk-" + "abcdefghijklmnop", "AKIA" + "1234567890ABCD"]
        self.put("src/fixture.txt", "\n".join(allowed))
        result = self.run_cli("--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["blocker"], [])
        self.put("src/fixture.txt", "\n".join(token + "X" for token in allowed))
        result = self.run_cli("--dry-run")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual({hit["rule"] for hit in json.loads(result.stdout)["blocker"]},
                         {"openai_token", "aws_access_key"})

    def test_warnings_do_not_block(self):
        self.put("src/example.txt", "zhizi" ".ihongui" ".com\nother@example.org\n"
                 "owner@example.com\nZHIZI_TEST_INVITE\n")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(json.loads(self.report.read_text())["warning"]), 3)
        self.assertTrue((self.out / "src/example.txt").exists())

    def test_nonempty_output_refused(self):
        self.out.mkdir()
        marker = self.out / "keep"
        marker.write_text("unchanged")
        result = self.run_cli()
        self.assertEqual(result.returncode, 2)
        self.assertEqual(marker.read_text(), "unchanged")
        self.assertFalse(self.report.exists())

    def test_dry_run_writes_nothing(self):
        self.put("src/a.txt", "ZHIZI_TEST_INVITE\n")
        result = self.run_cli("--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["files"], ["src/a.txt"])
        self.assertEqual(len(report["warning"]), 1)
        self.assertFalse(self.out.exists())
        self.assertFalse(self.report.exists())

    def test_relative_paths_permissions_and_size(self):
        source = self.put("scripts/nested/run.sh", "#!/bin/sh\n")
        source.chmod(0o755)
        self.out.mkdir()
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        target = self.out / "scripts/nested/run.sh"
        self.assertEqual(target.read_bytes(), source.read_bytes())
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o755)
        self.assertEqual(json.loads(self.report.read_text())["total_size"],
                         len(source.read_bytes()))

    def test_mapping_and_public_boundaries(self):
        kept = ["quality/golden/behavior-v0.3.tsv", "quality/corpus/mixer-hr-replay-v1.json",
                "quality/corpus/typo-correction-v1.json", "quality/corpus/punctuation-context-v1.json"]
        for name in kept:
            self.put(name)
        self.put("README.md", "internal")
        self.put("docs/public/README.md", "public")
        self.put("docs/public/README.zh-CN.md", "Chinese")
        self.put("docs/public/gitignore.public", "public ignores\n")
        self.put("docs/public/PUBLIC_BUILD", "")
        self.put_pack_dictionaries()
        for name in ["docs/public/PROMOTION-POSTS.md", "quality/golden/private.tsv",
                     "scripts/context_model/zhizi/gpu/gpu.sh"]:
            self.put(name)
        result = self.run_cli("--config", str(SCRIPT.with_suffix(".json")))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.out / "README.md").read_text(), "public")
        for name in kept:
            self.assertTrue((self.out / name).exists())
        self.assertFalse((self.out / "docs/public/PROMOTION-POSTS.md").exists())
        self.assertFalse((self.out / "scripts/context_model/zhizi/gpu/gpu.sh").exists())
        self.assertFalse((self.out / "quality/golden/private.tsv").exists())

    def test_new_public_docs_and_issue_templates_are_exported(self):
        kept = ["SECURITY.md", "CODE_OF_CONDUCT.md",
                ".github/ISSUE_TEMPLATE/bug_report.md", ".github/ISSUE_TEMPLATE/config.yml"]
        for name in kept:
            self.put(name, "synthetic fixture\n")
        self.put(".github/workflows/ci.yml", "synthetic fixture\n")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.report.read_text())["files"], sorted(kept))
        self.assertFalse((self.out / ".github/workflows/ci.yml").exists())

    def test_gitignore_public_renames_to_root_gitignore(self):
        self.put("docs/public/README.md", "public\n")
        self.put("docs/public/README.zh-CN.md", "Chinese\n")
        self.put("docs/public/gitignore.public", "synthetic ignore fixture\n")
        self.put("docs/public/PUBLIC_BUILD", "")
        self.put(".gitignore", "internal ignore fixture\n")
        self.put_pack_dictionaries()
        result = self.run_cli("--config", str(SCRIPT.with_suffix(".json")))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.out / ".gitignore").read_text(), "synthetic ignore fixture\n")
        self.assertFalse((self.out / "docs/public/gitignore.public").exists())
        # the build-guard marker lands empty at the root of the export and nowhere else
        self.assertEqual((self.out / "PUBLIC_BUILD").read_bytes(), b"")
        self.assertFalse((self.out / "docs/public/PUBLIC_BUILD").exists())

    def test_pack_dictionaries_are_exported_as_header_only_shells(self):
        self.put("docs/public/README.md", "public\n")
        self.put("docs/public/README.zh-CN.md", "Chinese\n")
        self.put("docs/public/gitignore.public", "x\n")
        self.put("docs/public/PUBLIC_BUILD", "")
        self.put_pack_dictionaries()
        self.put("native/resources/Rime/agent_general.dict.yaml", "---\nname: g\n...\nkept\tkept\t1\n")
        result = self.run_cli("--config", str(SCRIPT.with_suffix(".json")))
        self.assertEqual(result.returncode, 0, result.stderr)
        shell = (self.out / "native/resources/Rime/agent_pack_poetry.dict.yaml").read_text()
        self.assertNotIn("secret", shell)
        self.assertTrue(shell.rstrip().endswith("..."))
        self.assertIn("zhizi-lexicon", shell)
        self.assertIn("kept\tkept", (self.out / "native/resources/Rime/agent_general.dict.yaml").read_text())
        # the source file is untouched
        self.assertIn("secret-row", (self.src / "native/resources/Rime/agent_pack_poetry.dict.yaml").read_text())

    def test_missing_pack_dictionary_fails_instead_of_silently_exporting_nothing(self):
        self.put("docs/public/README.md", "public\n")
        self.put("docs/public/README.zh-CN.md", "Chinese\n")
        self.put("docs/public/gitignore.public", "x\n")
        self.put("docs/public/PUBLIC_BUILD", "")
        result = self.run_cli("--config", str(SCRIPT.with_suffix(".json")))
        self.assertEqual(result.returncode, 2)

    def test_promotion_excluded_even_with_broad_include(self):
        config = json.loads(SCRIPT.with_suffix(".json").read_text())
        config.pop("rename", None)
        config.pop("stub_dicts", None)
        config["include"].append("docs/public/")
        self.config.write_text(json.dumps(config))
        self.put("docs/public/PROMOTION-future.md")
        self.put("docs/public/PROMOTION/sub/note.md")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["files"], [])

    def test_internal_license_audits_excluded_even_with_broad_include(self):
        config = json.loads(SCRIPT.with_suffix(".json").read_text())
        config.pop("rename", None)
        config.pop("stub_dicts", None)
        config["include"].append("docs/current/")
        self.config.write_text(json.dumps(config))
        self.put("docs/current/licenses/LEXICON-LAYER-AUDIT-future.md")
        self.put("docs/current/licenses/TRAINING-DATA-TERMS-future.md")
        self.put("docs/current/VERSIONING.md")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["files"],
                         ["docs/current/VERSIONING.md"])

    def test_mapping_missing_source_is_error(self):
        config = self.root / "mapping.json"
        config.write_text(json.dumps({"include": ["README.md"], "exclude": [],
            "rename": {"missing.md": "README.md"}, "scan": {"blocker": [], "warning": []}}))
        self.put("README.md")
        self.assertEqual(self.run_cli("--config", str(config)).returncode, 2)

    def test_custom_config(self):
        config = self.root / "custom.json"
        config.write_text(json.dumps({"include": ["extra/"], "exclude": ["**/*.bin"],
                                      "scan": {"blocker": [], "warning": []}}))
        self.put("extra/a.txt")
        self.put("extra/a.bin")
        self.put("src/a.txt")
        result = self.run_cli("--config", str(config))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.report.read_text())["files"], ["extra/a.txt"])

    def test_utf16_text_is_scanned_and_binary_is_preserved(self):
        text = self.put("src/wide.txt")
        text.write_bytes("ZHIZI_TEST_INVITE\n".encode("utf-16"))
        binary = self.put("src/image.bin")
        binary.write_bytes(b"\x00\xff\x01")
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.report.read_text())["warning"],
                         [{"file": "src/wide.txt", "line": 1,
                           "rule": "test_invite_variable"}])
        self.assertEqual((self.out / "src/image.bin").read_bytes(), binary.read_bytes())

    def test_output_symlink_refused(self):
        real = self.root / "real"
        real.mkdir()
        self.out.symlink_to(real, target_is_directory=True)
        result = self.run_cli()
        self.assertEqual(result.returncode, 2)
        self.assertEqual(list(real.iterdir()), [])
        self.assertFalse(self.report.exists())

    def test_symlink_cannot_read_outside_repository(self):
        outside = self.root / "private.txt"
        outside.write_text("private")
        link = self.src / "src/link.txt"
        link.parent.mkdir()
        link.symlink_to(outside)
        self.git("add", "src/link.txt")
        result = self.run_cli()
        self.assertEqual(result.returncode, 2)
        self.assertFalse(self.out.exists())
        self.assertNotIn("private", result.stderr)


if __name__ == "__main__":
    unittest.main()
