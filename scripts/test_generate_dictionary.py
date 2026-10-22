#!/usr/bin/env python3
"""Focused parser tests for the parameterized THUOCL generator."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from generate_idiom_dictionary import read_source_rows


class ReadSourceRowsTests(unittest.TestCase):
    def parse(self, payload: bytes):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source.txt"
            path.write_bytes(payload)
            return read_source_rows(path)

    def test_accepts_bom_isolated_cr_and_spacing_variants(self):
        rows = self.parse(
            b"\xef\xbb\xbfword-one \t 12\rword-two\t34\rword-three  \t  56\r"
        )
        self.assertEqual(
            rows,
            [("word-one", 12, 1), ("word-two", 34, 2), ("word-three", 56, 3)],
        )

    def test_rejects_non_numeric_df_explicitly(self):
        with self.assertRaisesRegex(RuntimeError, "invalid THUOCL DF.*125472s"):
            self.parse("丁香\t125472s\n".encode())

    def test_rejects_missing_df_explicitly(self):
        with self.assertRaisesRegex(RuntimeError, "expected word and DF"):
            self.parse("浙江省地质灾害防治管理办法\t\n".encode())


if __name__ == "__main__":
    unittest.main()
