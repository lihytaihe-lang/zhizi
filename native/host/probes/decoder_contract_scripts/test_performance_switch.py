from pathlib import Path
import os
import subprocess
import tempfile
import unittest


class PerformanceSwitchTests(unittest.TestCase):
    def test_controller_script_forwards_the_trial_switch_to_the_harness(self):
        repo = Path(__file__).resolve().parents[4]
        for setting, expected in [(None, "0"), ("0", "0"), ("true", "0"), ("1", "1")]:
            with self.subTest(setting=setting), tempfile.TemporaryDirectory(prefix="zhixu-y3-performance-") as directory:
                root = Path(directory)
                bundle = root / "Synthetic.inputmethod"
                bundle.mkdir()
                (bundle / "fixture").write_text("synthetic bundle, not a release\n")
                harness = root / "harness"
                harness.write_text(
                    '#!/bin/bash\n'
                    'printf "%s\\n" "${ZHIZI_ZHIXU_WHOLE-unset}" > "$ZHIXU_Y3_SWITCH_CAPTURE"\n'
                    'exit 23\n'
                )
                harness.chmod(0o755)
                environment = os.environ.copy()
                environment.pop("ZHIZI_ZHIXU_WHOLE", None)
                environment.pop("AIM_PERFORMANCE_REPORT_OUT", None)
                environment["ZHIXU_Y3_SWITCH_CAPTURE"] = str(root / "captured")
                if setting is not None:
                    environment["ZHIZI_ZHIXU_WHOLE"] = setting
                result = subprocess.run(
                    ["bash", str(repo / "script/test_performance_controller_path.sh"), str(bundle), str(harness)],
                    env=environment, capture_output=True, text=True, check=False,
                )
                self.assertEqual(result.returncode, 23, result.stdout + result.stderr)
                self.assertEqual((root / "captured").read_text().strip(), expected)
                self.assertIn(f"zhixu_whole_trial={expected}\n", result.stdout)
                cleanup = next(line.split("=", 1)[1] for line in result.stdout.splitlines()
                               if line.startswith("temporary_cleanup_scope="))
                self.assertFalse(Path(cleanup).exists())


if __name__ == "__main__":
    unittest.main()
