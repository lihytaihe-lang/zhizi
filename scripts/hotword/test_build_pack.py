"""仅用测试密钥与临时输出验证词包，不读取生产私钥。"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("build_pack.py")
ROOT = SCRIPT.parents[2]
FIXTURES = ROOT / "src-tauri/tests/fixtures"
SPEC = importlib.util.spec_from_file_location("build_pack", SCRIPT)
packer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packer)


class HotwordPackTests(unittest.TestCase):
    def run_build(self, output, source=None):
        env = dict(os.environ, ZHIZI_HOTWORD_KEY_FILE=str(FIXTURES / "hotword_test_seed.hex"))
        return subprocess.run([
            sys.executable, str(SCRIPT), "build", str(source or SCRIPT.parent / "sample/test-input.tsv"),
            "--output-dir", str(output), "--base-url", "https://example.invalid/hotword-test",
            "--generated-at", "2026-11-08T00:00:00Z", "--public-key-file", str(FIXTURES / "hotword_test_pubkey.hex"),
        ], env=env, capture_output=True, text=True)

    def test_build_and_verify_match_checked_in_cross_language_fixtures(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            result = self.run_build(output)
            self.assertEqual(result.returncode, 0, result.stderr)
            for name in ["hotword-20261108.json", "hotword-20261108.json.sig", "latest.json", "latest.json.sig"]:
                self.assertEqual((output / name).read_bytes(), (FIXTURES / name).read_bytes())
            pack = output / "hotword-20261108.json"
            latest = json.loads((output / "latest.json").read_bytes())
            self.assertEqual(latest["sha256"], hashlib.sha256(pack.read_bytes()).hexdigest())
            verified = subprocess.run([sys.executable, str(SCRIPT), "--verify", str(pack), "--public-key-file", str(FIXTURES / "hotword_test_pubkey.hex"), "--now", "2026-11-08T00:00:00Z"], capture_output=True, text=True)
            self.assertEqual(verified.returncode, 0, verified.stderr)

    def test_canonical_order_is_independent_and_has_single_newline(self):
        left = {"乙": [dict(z=1, a="词")], "a": "\b\n"}
        right = {"a": "\b\n", "乙": [dict(a="词", z=1)]}
        self.assertEqual(packer.canonical_bytes(left), packer.canonical_bytes(right))
        self.assertTrue(packer.canonical_bytes(left).endswith(b"\n"))
        self.assertFalse(packer.canonical_bytes(left).endswith(b"\n\n"))
        self.assertIn("词".encode(), packer.canonical_bytes(left))

    def test_invalid_row_produces_no_partial_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "invalid.json"
            rows = json.loads((SCRIPT.parent / "sample/test-input.json").read_text())
            rows[1]["pinyin"] = "BAD1"
            source.write_text(json.dumps(rows, ensure_ascii=False))
            output = root / "out"
            result = self.run_build(output, source)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists())

    def test_tamper_and_wrong_key_fail_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertEqual(self.run_build(root).returncode, 0)
            pack = root / "hotword-20261108.json"
            value = json.loads(pack.read_bytes())
            value["entries"][0]["definition"] += "篡改"
            pack.write_bytes(packer.canonical_bytes(value))
            for public in [FIXTURES / "hotword_test_pubkey.hex", ROOT / "src-tauri/resources/hotword_pack_pubkey.hex"]:
                result = subprocess.run([sys.executable, str(SCRIPT), "verify", str(pack), "--public-key-file", str(public)], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("签名", result.stderr)

    def test_rejects_insecure_query_or_credential_download_url(self):
        for value in ["http://example.invalid/a", "https://example.invalid/a?q=1", "https://u:p@example.invalid/a", "https://example.invalid/a#fragment"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                packer.download_url(value)

    def test_rejects_duplicate_keys_and_floats(self):
        with self.assertRaises(ValueError):
            packer.load_json(b'{"schema":"one","schema":"two"}')
        with self.assertRaises(ValueError):
            packer.canonical_bytes({"number": 0.1})

    def test_rejects_malformed_signed_structures_with_human_error(self):
        original = json.loads((FIXTURES / "hotword-20261108.json").read_bytes())
        for location in ["entry", "source"]:
            value = json.loads(json.dumps(original))
            if location == "entry":
                value["entries"][0] = None
            else:
                value["entries"][0]["sources"][0] = None
            with self.subTest(location=location), self.assertRaises(ValueError):
                packer.validate_pack(value, packer.parse_time("2026-11-08T00:00:00Z"))

    def test_rejects_float_and_boolean_numeric_rows_before_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for key in ["expiresDays", "confidenceMillis"]:
                for bad in [1.5, True]:
                    rows = json.loads((SCRIPT.parent / "sample/test-input.json").read_bytes())
                    rows[0][key] = bad
                    source = root / "bad.json"
                    source.write_text(json.dumps(rows))
                    output = root / "out"
                    result = self.run_build(output, source)
                    self.assertNotEqual(result.returncode, 0, (key, bad))
                    self.assertFalse(output.exists())

    def test_signing_key_must_match_public_key_without_output(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "out"
            result = self.run_build(output)
            self.assertEqual(result.returncode, 0)
            # 默认生产公钥必须拒绝公开测试种子。
            other = Path(directory) / "other"
            env = dict(os.environ, ZHIZI_HOTWORD_KEY_FILE=str(FIXTURES / "hotword_test_seed.hex"))
            result = subprocess.run([sys.executable, str(SCRIPT), "build", str(SCRIPT.parent / "sample/test-input.tsv"), "--output-dir", str(other), "--base-url", "https://example.invalid/test"], env=env, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("不匹配", result.stderr)
            self.assertFalse(other.exists())
            self.assertNotIn(str(FIXTURES), result.stderr)

    def test_strict_rfc3339_and_source_url(self):
        for value in ["2026-11-08 00:00:00+00:00", "2026-11-08T00:00+00:00", "2026-11-08T00:00:00", "2026-02-30T00:00:00Z"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                packer.parse_time(value)
        for value in ["https://example.invalid/\na", "https://example.invalid/a b"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                packer.source_url(value)


if __name__ == "__main__":
    unittest.main()
