"""Exercise the existing silent-error reader/exporter without a neural runtime."""

import contextlib
import importlib
import io
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch


class QualityMetadataCompatibilityTests(unittest.TestCase):
    def test_silent_error_export_is_identical_for_old_and_new_records(self):
        # Only the inference and pronunciation dependencies are substituted.
        # The production JSONL reader, reconstruction, homophone filter and
        # versioned review exporter execute unchanged.
        mlx = types.ModuleType("mlx")
        mlx.core = types.ModuleType("mlx.core")
        mlx_lm = types.ModuleType("mlx_lm")
        mlx_lm.load = lambda _: None
        pypinyin = types.ModuleType("pypinyin")
        pypinyin.Style = types.SimpleNamespace(NORMAL=0)
        pypinyin.pinyin = lambda word, **_: [["zai"] for _ in word]
        stubs = {"mlx": mlx, "mlx.core": mlx.core, "mlx_lm": mlx_lm, "pypinyin": pypinyin}
        with patch.dict(sys.modules, stubs):
            probe = importlib.import_module("probe_rerank")
            audit = importlib.import_module("audit_silent_errors")

        class Scorer:
            def __init__(self, _):
                pass

            def score(self, context, candidates):
                return [0, 10]

        legacy = [{"id": str(i), "recordedAtMs": i * 1000, "appHash": "a" * 64,
                   "pinyin": "zai", "candidates": ["在", "再"], "selectedIndex": 0,
                   "selectedText": "在", "precedingText": "公开合成前文"} for i in (1, 2)]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            exports = []
            for new_format in (False, True):
                events = [dict(event) for event in legacy]
                if new_format:
                    events[0]["contextModel"] = {"enabled": False, "outcome": "disabled"}
                    events[1]["contextModel"] = {"enabled": True, "outcome": "reordered",
                                                 "baseOrder": [1, 0], "groupSize": 2}
                source = root / "events.jsonl"
                source.write_text("".join(json.dumps(event) + "\n" for event in events))
                loaded = probe.load_events(source)
                self.assertEqual([e["_recon"] for e in loaded], ["", "在"])
                if new_format:
                    self.assertEqual(loaded[1]["contextModel"], events[1]["contextModel"])
                destination = root / "review.json"
                args = ["audit_silent_errors", "--model", "synthetic", "--out", str(root),
                        "--export", str(destination), "--min-flagged", "1"]
                with patch.object(audit, "EVENTS", source), patch.object(audit, "Scorer", Scorer), \
                     patch.object(sys, "argv", args), patch.object(audit.time, "time", return_value=1), \
                     contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    audit.main()
                exports.append(json.loads(destination.read_text()))
            self.assertEqual(exports[0], exports[1])
            self.assertEqual(exports[1]["schema"], 1)
            self.assertEqual(len(exports[1]["pairs"]), 1)
            self.assertNotIn("contextModel", exports[1]["pairs"][0])


if __name__ == "__main__":
    unittest.main()
