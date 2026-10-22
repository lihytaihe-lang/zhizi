import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/tools/sync_version.py"
FILES = [
    "VERSION",
    "native/macos/Sources/AgentInputMethodHostKit/ReleaseVersion.swift",
    "native/macos/Resources/InputMethod-Info.plist",
    "package.json",
    "package-lock.json",
    "src-tauri/tauri.conf.json",
    "src-tauri/Cargo.toml",
    "src-tauri/Cargo.lock",
]


class SyncVersionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        for rel in FILES:
            (self.tmp / rel).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(ROOT / rel, self.tmp / rel)

    def run_sync(self, *extra):
        return subprocess.run([sys.executable, str(SCRIPT), "--root", str(self.tmp), *extra],
                              capture_output=True, text=True)

    def snapshot(self):
        return {rel: (self.tmp / rel).read_bytes() for rel in FILES}

    def set_version(self, value):
        (self.tmp / "VERSION").write_text(value, encoding="utf-8")

    def test_repository_is_in_sync(self):
        result = self.run_sync("--check")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_check_reports_drift_and_writes_nothing(self):
        self.set_version("9.8.7\n")
        before = self.snapshot()
        result = self.run_sync("--check")
        self.assertEqual(result.returncode, 1)
        for rel in FILES[1:]:
            self.assertIn(rel, result.stderr)
        self.assertEqual(self.snapshot(), before)

    def test_write_changes_only_the_version_field(self):
        self.set_version("9.8.7\n")
        before = self.snapshot()
        self.assertEqual(self.run_sync().returncode, 0)
        old = (ROOT / "VERSION").read_text().strip()
        for rel in FILES[1:]:
            old_text = before[rel].decode("utf-8")
            new_text = (self.tmp / rel).read_text(encoding="utf-8")
            expected = 2 if rel == "package-lock.json" else 1
            self.assertEqual(new_text.count("9.8.7"), expected + old_text.count("9.8.7"), rel)
            self.assertEqual(new_text.replace("9.8.7", old), old_text, rel)
        self.assertEqual(self.run_sync("--check").returncode, 0)

    def test_dependency_versions_in_lock_files_are_untouched(self):
        self.set_version("9.8.7")
        self.run_sync()
        lock = (self.tmp / "package-lock.json").read_text(encoding="utf-8")
        original = (ROOT / "package-lock.json").read_text(encoding="utf-8")
        self.assertEqual(lock.count('"version"'), original.count('"version"'))
        self.assertEqual(lock.count('"version": "9.8.7"'), 2)

    def test_blank_around_version_is_ignored_but_a_bad_value_is_rejected(self):
        self.set_version("  9.8.7 \r\n")
        self.assertEqual(self.run_sync().returncode, 0)
        self.assertIn("9.8.7", (self.tmp / "package.json").read_text())
        self.set_version("9.8\n")
        result = self.run_sync("--check")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("bad VERSION", result.stderr)


if __name__ == "__main__":
    unittest.main()
