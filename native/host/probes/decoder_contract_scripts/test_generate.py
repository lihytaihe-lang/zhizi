import json
from pathlib import Path
import tempfile
import unittest

import generate


class GeneratorTests(unittest.TestCase):
    def test_typing_script_contains_only_the_complete_input(self):
        actions = generate.typing_script("ni hao shi jie")
        self.assertEqual(actions, [f"type {letter}" for letter in "nihaoshijie"])
        self.assertFalse(any(action in {"clear", "commit_raw", "left", "right"} for action in actions))

    def test_twenty_fixed_scripts_cover_extended_contract_actions(self):
        paths = sorted((Path(__file__).parent / "fixed").glob("*.keys"))
        self.assertEqual(len(paths), 20)
        contents = "\n".join(path.read_text() for path in paths)
        for action in ["next_linear", "prev_linear", "clear_after_selection",
                       "select 0 no_learn", "select 0 required", "select 999 deliberate"]:
            self.assertIn(action, contents)
        for path in paths:
            lines = [line for line in path.read_text().splitlines() if line and not line.startswith("#")]
            self.assertEqual(lines[0], "clear", path.name)
            self.assertEqual(lines[-1], "clear", path.name)

    def test_dictionary_reads_body_only_and_normalizes_umlaut(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "public.dict.yaml"
            path.write_text("---\nname: test\n...\n女\tnü\t2\n你好\tni hao\t10\n"
                            "你好\tni hao\t1\n# ignored\n错\tbad7\t1\n")
            self.assertEqual(generate.load_dictionary(path), {
                "女": {"nv"}, "你好": {"ni hao"}})

    def test_annotation_composes_and_deduplicates_paths(self):
        readings = {"你": {"ni"}, "好": {"hao"}, "你好": {"ni hao"}}
        self.assertEqual(generate.annotate("你好", readings), "ni hao")
        self.assertIsNone(generate.annotate("你", readings))
        self.assertIsNone(generate.annotate("你好吗", readings))
        self.assertIsNone(generate.annotate("你好!", readings))

    def test_annotation_rejects_ambiguity_instead_of_picking_a_reading(self):
        readings = {"重": {"zhong", "chong"}, "来": {"lai"}, "重来": {"chong lai"}}
        self.assertIsNone(generate.annotate("重来", readings))

    def test_fragments_are_public_hanzi_with_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "public.json"
            path.write_text(json.dumps({"cases": [
                {"id": "one", "allowed": ["你好世界", "abc"]},
                {"id": "two", "allowed": ["你好"]},
            ]}, ensure_ascii=False))
            rows = generate.fragments([path])
            self.assertEqual({row["fragment"] for row in rows},
                             {"你好", "好世", "世界", "你好世", "好世界", "你好世界"})
            row = next(row for row in rows if row["fragment"] == "好世界")
            self.assertEqual((row["case_id"], row["start"], row["length"]), ("one", 1, 3))
            self.assertEqual(row["sentence"], "你好世界")

    def test_selection_is_seeded_and_balanced(self):
        readings = {"你": {"ni"}, "好": {"hao"}}
        rows = [{"fragment": "你" * size, "length": size, "case_id": str(size)}
                for size in range(2, 9)]
        rows += [{"fragment": "好" * size, "length": size, "case_id": f"b{size}"}
                 for size in range(2, 9)]
        first, stats = generate.choose_fragments(rows, readings, 7, 20261029)
        again, _ = generate.choose_fragments(rows, readings, 7, 20261029)
        self.assertEqual(first, again)
        self.assertEqual(sorted(row["length"] for row in first), list(range(2, 9)))
        self.assertEqual(stats["accepted"], 14)

    def test_scripts_are_reproducible_and_preserve_typed_letters(self):
        first = generate.edit_script("ni hao shi jie", 123)
        self.assertEqual(first, generate.edit_script("ni hao shi jie", 123))
        self.assertEqual("".join(line.split()[1] for line in first if line.startswith("type ")),
                         "nihaoshijie")
        self.assertEqual(first[-1], "clear")
        self.assertIn("# resolve_prefix", first)
        allowed = {"type", "backspace", "left", "right", "pagedown", "pageup", "next",
                   "prev", "select", "commit_raw", "clear", "next_linear", "prev_linear",
                   "clear_after_selection"}
        for line in first:
            if not line.startswith("#"):
                self.assertIn(line.split()[0], allowed)

    def test_prefix_resolution_follows_the_complete_original_input(self):
        reading = "ni hao shi jie"
        expected = ["clear"] + [f"type {key}" for key in reading.replace(" ", "")]
        for seed in range(20):
            actions = generate.edit_script(reading, seed)
            self.assertEqual(actions[:actions.index("# resolve_prefix")], expected)
            tail = actions[actions.index("# resolve_prefix") + 2:]
            for action in ["backspace", "left", "right", "pagedown", "pageup"]:
                self.assertIn(action, tail)


if __name__ == "__main__":
    unittest.main()
