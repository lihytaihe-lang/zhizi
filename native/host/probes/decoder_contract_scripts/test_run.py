from pathlib import Path
import tempfile
import unittest

import run


class RunnerTests(unittest.TestCase):
    def test_session_comparison_selects_session_probe_and_preserves_y0_default(self):
        args = run.arguments(["--rime-root", "/public/rime", "--dictionary", "/public/pinyin_simp.dict.yaml",
                              "--session", "--tested", "zhixu"])
        self.assertTrue(args.session)
        self.assertEqual(run.probe_name(args), "session_contract_compare")
        ordinary = run.arguments(["--rime-root", "/public/rime", "--dictionary", "/public/pinyin_simp.dict.yaml"])
        self.assertFalse(ordinary.session)
        self.assertEqual(run.probe_name(ordinary), "decoder_contract_compare")

    def test_typing_suite_has_300_inputs_by_default_and_accepts_zhixu(self):
        args = run.arguments(["--rime-root", "/public/rime", "--dictionary", "/public/pinyin_simp.dict.yaml",
                              "--tested", "zhixu", "--typing-only"])
        self.assertEqual((args.tested, args.typing_only, args.count), ("zhixu", True, 300))
        ordinary = run.arguments(["--rime-root", "/public/rime", "--dictionary", "/public/pinyin_simp.dict.yaml"])
        self.assertEqual(ordinary.count, 70)

    def test_existing_scripts_cannot_silently_replace_requested_generation(self):
        with self.assertRaises(SystemExit):
            run.arguments(["--rime-root", "/public/rime", "--dictionary", "/public/pinyin_simp.dict.yaml",
                           "--scripts", "/public/scripts", "--typing-only"])

    def fixture(self, directory):
        root = Path(directory)
        rime = root / "librime"
        (rime / "include").mkdir(parents=True)
        (rime / "lib").mkdir()
        (rime / "include/rime_api.h").write_text("/* public API */\n")
        (rime / "lib/librime.dylib").write_bytes(b"test placeholder")
        dictionary = root / "pinyin_simp.dict.yaml"
        dictionary.write_text("---\nname: pinyin_simp\n...\n你\tni\t1\n")
        return root, rime, dictionary

    def test_shared_contains_only_the_two_public_configs_and_dictionary(self):
        with tempfile.TemporaryDirectory() as directory:
            root, _, dictionary = self.fixture(directory)
            resources = root / "repo/native/resources/Rime"
            resources.mkdir(parents=True)
            (resources / "default.yaml").write_text("menu:\n  page_size: 5\n")
            (resources / "agent_pinyin.schema.yaml").write_text("schema:\n  schema_id: agent_pinyin\n")
            # An unrelated file must neither be copied nor opened by preparation.
            (resources / "unrelated.dict.yaml").write_text("not part of this public fixture")
            destination = root / "shared"
            result = run.prepare_shared(root / "repo", dictionary, destination)
            self.assertEqual(result, destination)
            self.assertEqual({path.name for path in destination.iterdir()}, {
                "default.yaml", "agent_pinyin.schema.yaml", "agent_pinyin.dict.yaml",
                "pinyin_simp.dict.yaml"})
            self.assertEqual((destination / dictionary.name).read_bytes(), dictionary.read_bytes())
            aggregate = (destination / "agent_pinyin.dict.yaml").read_text()
            self.assertIn("import_tables:\n  - pinyin_simp\n", aggregate)
            self.assertNotIn("unrelated", aggregate)

    def test_validate_requires_existing_library_dictionary_and_scripts(self):
        with tempfile.TemporaryDirectory() as directory:
            root, rime, dictionary = self.fixture(directory)
            script = root / "one.keys"
            script.write_text("clear\n")
            self.assertEqual(run.validate_inputs(rime, dictionary, script),
                             (rime.resolve(), dictionary.resolve(), script.resolve()))
            for library, table, keys in [
                (root / "missing", dictionary, script),
                (rime, root / "missing.dict.yaml", script),
                (rime, dictionary, root / "missing.keys"),
                (rime, dictionary, root / "empty"),
            ]:
                (root / "empty").mkdir(exist_ok=True)
                with self.assertRaises(ValueError):
                    run.validate_inputs(library, table, keys)

    def test_validate_rejects_wrong_dictionary_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            _, rime, dictionary = self.fixture(directory)
            dictionary.write_text("---\nname: some_other_table\n...\n你\tni\t1\n")
            with self.assertRaises(ValueError):
                run.validate_inputs(rime, dictionary)

    def test_shim_is_taken_only_from_host_build_script_message(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            (out / "libaim_rime_shim.a").write_bytes(b"test archive")
            message = {"reason": "build-script-executed",
                       "package_id": "path+file:///public/host#zhizi-host@0.1.0",
                       "out_dir": str(out)}
            self.assertEqual(run.shim_from_message(message), out)
            self.assertIsNone(run.shim_from_message({**message, "package_id": "other@0.1.0"}))
            self.assertIsNone(run.shim_from_message({**message, "reason": "compiler-artifact"}))


if __name__ == "__main__":
    unittest.main()
