"""Tests for prepare_dataset (artificial Latin-only fixtures; authored, not executed here)."""

import hashlib
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from prepare_dataset import OUTPUT_FILES, main, prepare


def row(rid, target, **extra):
    base = {
        "id": rid,
        "context": "ctx " + rid,
        "target": target,
        "raw": "rawtext",
        "source": "src",
        "license": "cc0",
    }
    base.update(extra)
    return base


def all_rows(splits):
    return splits["train"] + splits["valid"] + splits["test"]


def group_count(splits):
    return len({entry["splitGroup"] for entry in all_rows(splits)})


class PrepareTests(unittest.TestCase):
    def test_component_ids_are_unambiguous_with_newlines(self):
        splits, _ = prepare([
            row("a", "aaaaaa", group="pair"),
            row("b", "zzzzzz", group="pair"),
            row("a\nb", "bbbbbb"),
        ])
        self.assertEqual(group_count(splits), 2)

    def test_exact_variants_union(self):
        splits, manifest = prepare(
            [row("a", "abcdef"), row("b", "ABCDEF"), row("c", "abc-def")]
        )
        self.assertEqual(group_count(splits), 1)
        self.assertEqual(manifest["groups"], 1)
        self.assertEqual(manifest["input_rows"], 3)

    def test_near_union_substitution(self):
        splits, _ = prepare([row("a", "abcdef"), row("b", "abcdeg")])
        self.assertEqual(group_count(splits), 1)

    def test_near_union_insertion_and_deletion(self):
        self.assertEqual(
            group_count(prepare([row("a", "abcdef"), row("b", "abcgdef")])[0]), 1
        )
        self.assertEqual(
            group_count(prepare([row("a", "abcdefg"), row("b", "abcdef")])[0]), 1
        )

    def test_distance_two_not_unioned(self):
        splits, manifest = prepare([row("a", "abcdef"), row("b", "abcdxy")])
        self.assertEqual(group_count(splits), 2)
        self.assertEqual(manifest["groups"], 2)

    def test_near_boundary_below_min_length_not_unioned(self):
        splits, _ = prepare([row("a", "abcde"), row("b", "abcdf")])
        self.assertEqual(group_count(splits), 2)
        splits, _ = prepare([row("a", "abcde"), row("b", "abcd")])
        self.assertEqual(group_count(splits), 2)

    def test_transitive_union(self):
        splits, manifest = prepare(
            [row("a", "abcdef"), row("b", "abcdeg"), row("c", "ABCDEF")]
        )
        self.assertEqual(group_count(splits), 1)
        self.assertEqual(manifest["groups"], 1)

    def test_explicit_group_union(self):
        splits, manifest = prepare(
            [
                row("a", "onealpha", group="g1"),
                row("b", "twoomega", group="g1"),
                row("c", "threegamma"),
            ]
        )
        self.assertEqual(group_count(splits), 2)
        self.assertEqual(manifest["groups"], 2)

    def test_empty_group_does_not_union(self):
        splits, _ = prepare([row("a", "onealpha", group=""), row("b", "twoomega", group="")])
        self.assertEqual(group_count(splits), 2)

    def test_input_order_invariance(self):
        rows = [
            row("a", "abcdef"),
            row("b", "abcdeg"),
            row("c", "zzqqqq", group="g1"),
            row("d", "wwrrrr", group="g1"),
            row("e", "mnopqr"),
            row("f", "mnopqs"),
            row("g", "stuvwx"),
            row("h", "stuvwy"),
        ]
        first_splits, first_manifest = prepare(rows)
        second_splits, second_manifest = prepare(list(reversed(rows)))
        self.assertEqual(
            json.dumps(first_splits, sort_keys=True), json.dumps(second_splits, sort_keys=True)
        )
        self.assertEqual(
            json.dumps(first_manifest, sort_keys=True), json.dumps(second_manifest, sort_keys=True)
        )

    def test_duplicate_ids_rejected(self):
        with self.assertRaises(ValueError):
            prepare([row("a", "abcdef"), row("b", "abcdeg"), row("a", "zzzzzz")])

    def test_invalid_fractions_rejected(self):
        rows = [row("a", "abcdef")]
        bad_values = [0, -0.1, float("nan"), float("inf"), 0.9]
        for value in bad_values:
            with self.assertRaises(ValueError):
                prepare(rows, valid_fraction=value)
            with self.assertRaises(ValueError):
                prepare(rows, test_fraction=value)
        with self.assertRaises(ValueError):
            prepare(rows, valid_fraction=0.5, test_fraction=0.5)

    def test_invalid_fields_rejected(self):
        base = row("a", "abcdef")
        bad_rows = [
            {key: value for key, value in base.items() if key != "id"},
            row("", "abcdef"),
            dict(base, target=42),
            dict(base, target="!!!"),
            dict(base, raw=""),
            dict(base, raw="RAWTEXT"),
            dict(base, raw="rawtext1"),
            dict(base, raw="raw-text"),
            dict(base, raw="''''''"),
            dict(base, context=None),
            dict(base, group=None),
            dict(base, group=7),
            dict(base, negatives="abc"),
            dict(base, negatives=[""]),
            dict(base, negatives=["x", "x"]),
            dict(base, negatives=["abcdef"]),
            dict(base, negatives=[42]),
            dict(base, splitGroup="x"),
            "not-a-dict",
        ]
        for bad in bad_rows:
            with self.assertRaises(ValueError):
                prepare([bad])
        with self.assertRaises(ValueError):
            prepare([])

    def test_preserved_fields(self):
        original = row("a", "abcdef", extra="keep", negatives=["nm"], group="g1")
        splits, _ = prepare([original])
        merged = all_rows(splits)
        self.assertEqual(len(merged), 1)
        for key, value in original.items():
            self.assertEqual(merged[0][key], value)
        self.assertEqual(merged[0]["extra"], "keep")
        self.assertNotIn("splitGroup", original)
        self.assertTrue(merged[0]["splitGroup"])

    def test_empty_context_allowed(self):
        splits, _ = prepare([row("a", "abcdef", context="")])
        self.assertEqual(len(all_rows(splits)), 1)

    def test_splits_partition_and_sorted(self):
        rows = [row("r%d" % i, "tgt%dabcdef" % i) for i in range(30)]
        splits, manifest = prepare(rows, seed="s1")
        seen = []
        for name in ("train", "valid", "test"):
            ids = [entry["id"] for entry in splits[name]]
            self.assertEqual(ids, sorted(ids))
            seen.extend(ids)
        self.assertEqual(sorted(seen), sorted(entry["id"] for entry in rows))
        total = sum(manifest["splits"][name]["rows"] for name in ("train", "valid", "test"))
        self.assertEqual(total, 30)


class CliTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = tmp.name
        self.input_path = os.path.join(self.root, "input.jsonl")
        self.out_dir = os.path.join(self.root, "out")

    def _write_input(self, rows):
        with open(self.input_path, "w", encoding="utf-8") as handle:
            for item in rows:
                handle.write(json.dumps(item, ensure_ascii=False) + "\n")

    def test_outputs_and_hash_binding(self):
        self._write_input([row("r%d" % i, "tgt%dabcdef" % i) for i in range(12)])
        main(["--input", self.input_path, "--out", self.out_dir, "--seed", "s9"])
        for name in OUTPUT_FILES:
            self.assertTrue(os.path.exists(os.path.join(self.out_dir, name)))
        with open(self.input_path, "rb") as handle:
            input_sha = hashlib.sha256(handle.read()).hexdigest()
        with open(os.path.join(self.out_dir, "manifest.json"), "rb") as handle:
            manifest = json.loads(handle.read().decode("utf-8"))
        self.assertEqual(manifest["input_sha256"], input_sha)
        self.assertEqual(
            set(manifest["output_sha256"]), {"train.jsonl", "valid.jsonl", "test.jsonl"}
        )
        for name, digest in manifest["output_sha256"].items():
            with open(os.path.join(self.out_dir, name), "rb") as handle:
                data = handle.read()
            self.assertEqual(hashlib.sha256(data).hexdigest(), digest)
            if data:
                self.assertTrue(data.endswith(b"\n"))

    def test_overwrite_refusal_and_unchanged_files(self):
        self._write_input([row("a", "abcdef"), row("b", "abcdeg")])
        main(["--input", self.input_path, "--out", self.out_dir])
        before = {}
        for name in OUTPUT_FILES:
            with open(os.path.join(self.out_dir, name), "rb") as handle:
                before[name] = handle.read()
        with self.assertRaises(ValueError):
            main(["--input", self.input_path, "--out", self.out_dir])
        for name in OUTPUT_FILES:
            with open(os.path.join(self.out_dir, name), "rb") as handle:
                self.assertEqual(handle.read(), before[name])

    def test_preexisting_output_file_refused(self):
        os.makedirs(self.out_dir)
        with open(os.path.join(self.out_dir, "manifest.json"), "w", encoding="utf-8") as handle:
            handle.write("{}\n")
        self._write_input([row("a", "abcdef")])
        with self.assertRaises(ValueError):
            main(["--input", self.input_path, "--out", self.out_dir])


if __name__ == "__main__":
    unittest.main()
