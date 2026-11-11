import base64
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import patch

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
FIXTURES = ROOT / "src-tauri/tests/fixtures/skin-store"


def module(name):
    spec = importlib.util.spec_from_file_location(name, HERE / (name + ".py"))
    value = importlib.util.module_from_spec(spec)
    sys.modules[name] = value
    spec.loader.exec_module(value)
    return value


bundle = module("build_skin_bundle")
index = module("build_skin_index")


class SkinPublisherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="skin-publisher-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.skin = self.root / "source"
        self.skin.mkdir()
        self.skin_json = {
            "id": "teststore", "formatVersion": 1,
            "displayName": {"zh": "测试皮肤", "en": "Test Skin"},
            "author": "Tests", "description": "测试用，不上架。",
        }
        self.write_skin()
        self.output = self.root / "teststore-1.0.0.zhskin.json"
        self.preview = self.root / "preview.png"
        self.preview.write_bytes((FIXTURES / "preview.png").read_bytes())
        private = Ed25519PrivateKey.from_private_bytes(bytes(range(32)))
        self.private_path = self.root / "test-only-signing.key"
        self.private_path.write_text(bytes(range(32)).hex() + "\n", encoding="ascii")
        self.public_path = self.root / "public-key.hex"
        self.public_path.write_text(private.public_key().public_bytes(
            serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex() + "\n", encoding="ascii")
        self.env = patch.dict(os.environ, {"ZHIZI_SKIN_KEY_FILE": str(self.private_path)})
        self.env.start()
        self.addCleanup(self.env.stop)

    def write_skin(self):
        (self.skin / "skin.json").write_text(json.dumps(self.skin_json, ensure_ascii=False), encoding="utf-8")

    def build_bundle(self):
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            return bundle.main([str(self.skin), "--out", str(self.output), "--version", "1.0.0"])

    def config(self):
        self.assertEqual(self.build_bundle(), 0)
        return {
            "generatedAt": "2026-11-08T04:00:00Z", "producer": "agent-ime-skin-publisher/1.0.0",
            "skins": [{
                "id": "teststore", "name": {"zh": "测试皮肤", "en": "Test Skin"},
                "author": "Tests", "description": "测试用，不上架。", "version": "1.0.0",
                "tier": "free", "priceCents": 0, "productId": "skin.teststore", "animated": False,
                "previewUrl": "https://example.invalid/preview.png", "previewPath": "preview.png",
                "bundleUrl": "https://example.invalid/teststore-1.0.0.zhskin.json",
                "bundlePath": self.output.name, "minAppVersion": "0.3.24", "updatedAt": "2026-11-08",
                "license": "测试夹具", "changelog": "首次测试。",
            }],
        }

    def build_index(self, config, extra=()):
        config_path = self.root / "config.json"
        config_path.write_text(json.dumps(config, ensure_ascii=False), encoding="utf-8")
        self.index_dir = self.root / "published"
        args = [str(config_path), "--out", str(self.index_dir), *extra]
        stdout, stderr = io.StringIO(), io.StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            result = index.main(args)
        return result, stdout.getvalue() + stderr.getvalue()

    def test_bundle_is_deterministic_and_round_trips(self):
        self.assertEqual(self.build_bundle(), 0)
        first = self.output.read_bytes()
        self.assertEqual(self.build_bundle(), 0)
        self.assertEqual(self.output.read_bytes(), first)
        value = json.loads(first)
        self.assertEqual(value["schema"], "agent-ime-skin-bundle/1")
        self.assertEqual(value["id"], "teststore")
        self.assertEqual(value["version"], "1.0.0")
        self.assertEqual(base64.b64decode(value["files"][0]["base64"]), (self.skin / "skin.json").read_bytes())

    def test_bundle_rejects_invalid_id_and_version(self):
        for bad in ["../outside", "Uppercase", "", "x" * 65]:
            with self.subTest(id=bad):
                self.skin_json["id"] = bad
                self.write_skin()
                self.assertEqual(self.build_bundle(), 1)
        self.skin_json["id"] = "teststore"
        self.write_skin()
        with redirect_stderr(io.StringIO()):
            self.assertEqual(bundle.main([str(self.skin), "--out", str(self.output), "--version", "1.x"]), 1)

    def test_bundle_rejects_symlink_and_nested_paths(self):
        target = self.root / "outside.png"
        target.write_bytes(self.preview.read_bytes())
        (self.skin / "link.png").symlink_to(target)
        self.assertEqual(self.build_bundle(), 1)
        (self.skin / "link.png").unlink()
        (self.skin / "nested").mkdir()
        self.assertEqual(self.build_bundle(), 1)

    def test_bundle_rejects_file_count_size_and_non_png(self):
        for number in range(8):
            (self.skin / (str(number) + ".png")).write_bytes(self.preview.read_bytes())
        self.assertEqual(self.build_bundle(), 1)
        for path in self.skin.glob("*.png"):
            path.unlink()
        (self.skin / "fake.png").write_bytes(b"not PNG")
        self.assertEqual(self.build_bundle(), 1)
        (self.skin / "fake.png").unlink()
        (self.skin / "script.js").write_text("no", encoding="utf-8")
        self.assertEqual(self.build_bundle(), 1)
        (self.skin / "script.js").unlink()
        (self.skin / "skin.json").write_bytes(b" " * (256 * 1024 + 1))
        self.assertEqual(self.build_bundle(), 1)
        self.write_skin()
        (self.skin / "huge.png").write_bytes(b"\x89PNG\r\n\x1a\n" + b"x" * (1536 * 1024))
        self.assertEqual(self.build_bundle(), 1)

    def test_bundle_rejects_duplicate_json_keys(self):
        (self.skin / "skin.json").write_text('{"id":"teststore","id":"other"}', encoding="utf-8")
        self.assertEqual(self.build_bundle(), 1)

    def test_bundle_rejects_boolean_schema_version_and_embedded_version_mismatch(self):
        self.skin_json["formatVersion"] = True
        self.write_skin()
        self.assertEqual(self.build_bundle(), 1)
        self.skin_json["formatVersion"] = 1
        self.skin_json["version"] = "2.0.0"
        self.write_skin()
        self.assertEqual(self.build_bundle(), 1)
        self.skin_json["version"] = "1.0.0"
        self.write_skin()
        self.assertEqual(self.build_bundle(), 0)
        parsed = json.loads(self.output.read_bytes())
        parsed["version"] = "2.0.0"
        with self.assertRaises(ValueError):
            bundle.validate_bundle(parsed)

    def test_signed_index_and_verify_cli(self):
        result, _ = self.build_index(self.config(), ["--test-public-key-file", str(self.public_path)])
        self.assertEqual(result, 0)
        document = self.index_dir / "skins-index.json"
        value = json.loads(document.read_bytes())
        self.assertEqual(value["skins"][0]["bundleBytes"], self.output.stat().st_size)
        self.assertEqual(value["skins"][0]["bundleSha256"], hashlib.sha256(self.output.read_bytes()).hexdigest())
        self.assertNotIn("bundlePath", value["skins"][0])
        with redirect_stdout(io.StringIO()):
            self.assertEqual(index.main(["--verify", str(document), "--test-public-key-file", str(self.public_path)]), 0)
        parsed = json.loads(document.read_bytes())
        document.write_text(json.dumps(parsed, ensure_ascii=False, indent=2), encoding="utf-8")
        with redirect_stdout(io.StringIO()):
            self.assertEqual(index.main(["--verify", str(document), "--test-public-key-file", str(self.public_path)]), 0)

    def test_production_key_mismatch_writes_nothing_and_hides_seed(self):
        result, message = self.build_index(self.config())
        self.assertEqual(result, 1)
        self.assertFalse(self.index_dir.exists())
        self.assertNotIn(self.private_path.read_text().strip(), message)
        self.assertNotIn(str(self.private_path), message)

    def test_index_field_validation(self):
        base = self.config()
        cases = [("id", "../escape"), ("previewUrl", "http://example.invalid/a.png"),
                 ("previewUrl", "HTTPS://example.invalid/a.png"),
                 ("bundleUrl", "https://example.invalid/a?token=no"), ("previewUrl", "https://user@example.invalid/a"),
                 ("priceCents", -1), ("priceCents", 99901), ("priceCents", True), ("tier", "paid"),
                 ("tier", "invalid"), ("version", "1.x"), ("minAppVersion", "v1"),
                 ("productId", ""), ("animated", 1), ("updatedAt", "2026-02-30"),
                 ("name", {"zh": "", "en": "Test"})]
        for field, value in cases:
            with self.subTest(field=field, value=value):
                config = copy.deepcopy(base)
                config["skins"][0][field] = value
                self.assertEqual(self.build_index(config, ["--test-public-key-file", str(self.public_path)])[0], 1)
        for field in ["schema", "generatedAt"]:
            config = copy.deepcopy(base)
            config[field] = "invalid"
            self.assertEqual(self.build_index(config, ["--test-public-key-file", str(self.public_path)])[0], 1)
        config = copy.deepcopy(base)
        config["skins"] *= 201
        self.assertEqual(self.build_index(config, ["--test-public-key-file", str(self.public_path)])[0], 1)
        config = copy.deepcopy(base)
        self.assertEqual(self.build_index(config, ["--test-public-key-file", str(self.public_path)])[0], 0)
        signed = json.loads((self.index_dir / "skins-index.json").read_bytes())
        signed["skins"][0]["id"] = "system"
        with self.assertRaises(ValueError):
            index.validate_index(signed)

    def test_index_rejects_bundle_mismatch_and_invalid_preview(self):
        config = self.config()
        config["skins"][0]["version"] = "2.0.0"
        self.assertEqual(self.build_index(config, ["--test-public-key-file", str(self.public_path)])[0], 1)

    def test_bundle_and_preview_reject_png_magic_only_corrupt_crc_and_truncation(self):
        good = self.preview.read_bytes()
        corrupted = bytearray(good)
        corrupted[-1] ^= 1
        for malformed in [bundle.PNG_HEADER, bytes(corrupted), good[:-1], good + b"extra"]:
            with self.subTest(size=len(malformed)):
                config = self.config()
                self.preview.write_bytes(malformed)
                self.assertEqual(self.build_index(config, ["--test-public-key-file", str(self.public_path)])[0], 1)
                self.preview.write_bytes(good)
                (self.skin / "bad.png").write_bytes(malformed)
                self.assertEqual(self.build_bundle(), 1)
                (self.skin / "bad.png").unlink()
        config = self.config()
        self.preview.write_bytes(b"not PNG")
        self.assertEqual(self.build_index(config, ["--test-public-key-file", str(self.public_path)])[0], 1)

    def test_verify_rejects_tampering_wrong_key_schema_sha_and_duplicates(self):
        self.assertEqual(self.build_index(self.config(), ["--test-public-key-file", str(self.public_path)])[0], 0)
        document = self.index_dir / "skins-index.json"
        original = document.read_bytes()
        document.write_bytes(original.replace(b"Test Skin", b"Best Skin"))
        with redirect_stderr(io.StringIO()):
            self.assertEqual(index.main(["--verify", str(document), "--test-public-key-file", str(self.public_path)]), 1)
        document.write_bytes(original)
        with redirect_stderr(io.StringIO()):
            self.assertEqual(index.main(["--verify", str(document)]), 1)
        for field, value in [("schema", "wrong"), ("previewSha256", "z" * 64)]:
            parsed = json.loads(original)
            if field == "schema":
                parsed[field] = value
            else:
                parsed["skins"][0][field] = value
            data = index.canonical_bytes(parsed)
            private = Ed25519PrivateKey.from_private_bytes(bytes(range(32)))
            document.write_bytes(data)
            Path(str(document) + ".sig").write_text(private.sign(data).hex() + "\n", encoding="ascii")
            with redirect_stderr(io.StringIO()):
                self.assertEqual(index.main(["--verify", str(document), "--test-public-key-file", str(self.public_path)]), 1)
        document.write_bytes(b'{"schema":"wrong","schema":"agent-ime-skin-index/1"}')
        with redirect_stderr(io.StringIO()):
            self.assertEqual(index.main(["--verify", str(document), "--test-public-key-file", str(self.public_path)]), 1)

    def test_fixture_is_reproducible_with_public_scripts(self):
        with redirect_stdout(io.StringIO()):
            self.assertEqual(index.main(["--verify", str(FIXTURES / "skins-index.json"),
                                         "--test-public-key-file", str(FIXTURES / "public-key.hex")]), 0)
        config = FIXTURES / "publisher-config.json"
        with patch.dict(os.environ, {"ZHIZI_SKIN_KEY_FILE": str(FIXTURES / "test-only-signing.key")}):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(index.main([str(config), "--out", str(self.root / "fixture-index"),
                                             "--test-public-key-file", str(FIXTURES / "public-key.hex")]), 0)
        self.assertEqual((self.root / "fixture-index/skins-index.json").read_bytes(), (FIXTURES / "skins-index.json").read_bytes())
        self.assertEqual((self.root / "fixture-index/skins-index.json.sig").read_bytes(), (FIXTURES / "skins-index.json.sig").read_bytes())


if __name__ == "__main__":
    unittest.main()
