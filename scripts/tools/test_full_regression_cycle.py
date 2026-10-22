#!/usr/bin/env python3
"""Self-test for scripts/tools/full_regression_cycle.sh and script/test_all.sh.

Every case executes the real bash entry points with AIM_REGRESSION_STUB_DIR
pointed at a synthetic command root: $STUB/bin shadows node/npm/cargo/swift/
python3/git/... and $STUB/script shadows every script/*.sh gate, so no real
build, test or install command ever runs.  The python `unittest` gate is
stubbed like every other tool — this is exactly what prevents the self-test
from recursing into a real regression run.  scripts/tools/full_regression_cycle.sh
execs into script/test_all.sh, so one layer of runner behavior is under test.

Run: PYTHONPATH=<repo> python3 -m unittest discover -p 'test_*.py'
(from scripts/tools; also run by script/test_all.sh's `unittest` gate).
"""
import os
import re
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
TEST_ALL = REPO / "script" / "test_all.sh"
CYCLE = REPO / "scripts" / "tools" / "full_regression_cycle.sh"

TOOL_STUBS = ["node", "npm", "cargo", "swift", "python3", "codesign", "touch", "cp"]

GENERIC_STUB = """#!/bin/sh
name=$(basename "$0")
printf '%s\\n' "$*" >> "$STUB_RECORD_DIR/$name.inv"
echo "stub $name invoked: $*"
if [ -f "$STUB_CONTROL_DIR/$name.sleep" ]; then
  exec sleep 9713
fi
if [ -f "$STUB_CONTROL_DIR/$name.exit" ]; then
  code=$(cat "$STUB_CONTROL_DIR/$name.exit")
  echo "stub $name exiting $code"
  exit "$code"
fi
exit 0
"""

GIT_STUB = """#!/bin/sh
name=$(basename "$0")
printf '%s\\n' "$*" >> "$STUB_RECORD_DIR/$name.inv"
case "$*" in
  *"rev-list --count"*) echo "${STUB_GIT_COMMIT_COUNT:-1743}"; exit 0 ;;
esac
exit 0
"""

# python3 stub for the direct context-model-gate tests.  It stands in for the
# real scripts/context_model tooling without touching model data: an argument
# ending in /ContextModel is the copy_bundle_model.py destination (create it
# with a synthetic availability.json controlled by $STUB_CONTROL_DIR), and any
# invocation mentioning availability.json is the availability probe, decided
# by reading that synthetic file exactly like the real one-line python check.
CONTEXT_PYTHON_STUB = """#!/bin/sh
name=$(basename "$0")
printf '%s\\n' "$*" >> "$STUB_RECORD_DIR/$name.inv"
echo "stub $name invoked: $*"
for a in "$@"; do
  case "$a" in
    */ContextModel)
      mkdir -p "$a"
      if [ -f "$STUB_CONTROL_DIR/model.unavailable" ]; then
        printf '{"available": false}' > "$a/availability.json"
      else
        printf '{"available": true}' > "$a/availability.json"
      fi
      echo "stub copy_bundle_model wrote $a/availability.json"
      exit 0
      ;;
  esac
done
case "$*" in
  *availability.json*)
    probe=""
    for a in "$@"; do case "$a" in *availability.json) probe="$a";; esac; done
    if [ -n "$probe" ] && grep -q '"available"[[:space:]]*:[[:space:]]*false' "$probe"; then
      echo "stub availability probe: unavailable"
      exit 1
    fi
    echo "stub availability probe: available"
    exit 0
    ;;
esac
exit 0
"""

SUMMARY_HEADER = ["id", "status", "exit", "seconds", "log", "needs", "provides", "command"]


def _run_plain(args, **kwargs):
    return subprocess.run(
        [str(a) for a in args], capture_output=True, text=True, timeout=120, **kwargs
    )


class StubRoot:
    """Synthetic command root for AIM_REGRESSION_STUB_DIR."""

    def __init__(self, base):
        self.base = Path(base)
        self.bin = self.base / "bin"
        self.script = self.base / "script"
        self.control = self.base / "control"
        self.record = self.base / "record"
        self.runs = self.base / "runs"
        for directory in (self.bin, self.script, self.control, self.record):
            directory.mkdir(parents=True)

    def _write(self, path, body):
        path.write_text(body)
        path.chmod(0o755)

    def install(self, names, body=GENERIC_STUB):
        for name in names:
            self._write(self.bin / name, body)

    def install_git(self):
        self._write(self.bin / "git", GIT_STUB)

    def install_scripts(self, names):
        for name in names:
            self._write(self.script / name, GENERIC_STUB)

    def control_exit(self, name, code):
        (self.control / ("%s.exit" % name)).write_text(str(code))

    def control_sleep(self, name):
        (self.control / ("%s.sleep" % name)).touch()

    def invocations(self, name):
        path = self.record / ("%s.inv" % name)
        if not path.exists():
            return []
        # Each invocation prints exactly one line via printf '%s\n' "$*", so a
        # call with no arguments records a single EMPTY line — and that empty
        # line is the evidence of the call.  Filtering empty lines away made
        # no-argv commands (bootstrap_native.sh, unregister_dev_bundles.sh)
        # look like they never ran; count every line, including empty ones.
        return path.read_text().splitlines()

    def env(self, extra=None):
        env = dict(os.environ)
        env.pop("AIM_REGRESSION_RUN_DIR", None)
        env.update(
            AIM_REGRESSION_STUB_DIR=str(self.base),
            STUB_RECORD_DIR=str(self.record),
            STUB_CONTROL_DIR=str(self.control),
            STUB_GIT_COMMIT_COUNT="1743",
        )
        env.update(extra or {})
        return env

    def summaries(self):
        if not self.runs.exists():
            return []
        return sorted(p for p in self.runs.glob("*/summary.tsv"))

    def latest_rows(self):
        paths = self.summaries()
        assert paths, "no run dir under %s" % self.runs
        return self._load(paths[-1])

    def _load(self, summary_path):
        rows = {}
        with summary_path.open() as handle:
            header = handle.readline().rstrip("\n").split("\t")
            assert header == SUMMARY_HEADER, header
            for line in handle:
                fields = line.rstrip("\n").split("\t")
                row = dict(zip(header, fields))
                rows[row["id"]] = row
        return rows


def _script_names_from_gate_table():
    listing = _run_plain([TEST_ALL, "--list"])
    assert listing.returncode == 0, listing.stderr
    names = set()
    gate_ids = []
    for line in listing.stdout.splitlines()[1:]:
        fields = line.split("\t")
        gate_ids.append(fields[0])
        for match in re.finditer(r"\$REG_SCRIPT_DIR/([A-Za-z0-9_.-]+\.sh)", fields[3]):
            names.add(match.group(1))
    return names, gate_ids


class FullRegressionCycleSelfTest(unittest.TestCase):
    SCRIPT_NAMES, GATE_IDS = _script_names_from_gate_table()

    def setUp(self):
        self.stub = self.make_stub()
        # unregister_dev_bundles.sh is invoked by the runner's EXIT trap, not by
        # any gate command, so it is never part of the derived name set — but it
        # absolutely must be stubbed (the self-test never touches LaunchServices).
        self.stub.install_scripts(self.SCRIPT_NAMES | {"unregister_dev_bundles.sh"})

    def make_stub(self):
        tmp = tempfile.TemporaryDirectory(prefix="aim-regression-selftest.")
        self.addCleanup(tmp.cleanup)
        stub = StubRoot(tmp.name)
        stub.install(TOOL_STUBS)
        stub.install_git()
        return stub

    def run_entry(self, entry, stub=None, extra_env=None, timeout=120):
        return subprocess.run(
            [str(entry)],
            env=(stub or self.stub).env(extra_env),
            capture_output=True,
            text=True,
            timeout=timeout,
        )

    def assert_status(self, rows, gate_id, status, exit_code=None):
        self.assertIn(gate_id, rows, "gate missing from summary: %s" % gate_id)
        row = rows[gate_id]
        self.assertEqual(row["status"], status, "%s -> %r" % (gate_id, row))
        if exit_code is not None:
            self.assertEqual(row["exit"], str(exit_code), "%s -> %r" % (gate_id, row))
        return row

    def test_green_cycle_all_gates_pass_exit_zero(self):
        result = self.run_entry(CYCLE)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        rows = self.stub.latest_rows()
        self.assertEqual(sorted(rows), sorted(self.GATE_IDS))
        for gate_id, row in rows.items():
            self.assertEqual(row["status"], "PASS", gate_id)
            self.assertEqual(row["exit"], "0", gate_id)
            log = Path(row["log"])
            self.assertTrue(log.is_file() and log.stat().st_size > 0, row["log"])
            self.assertTrue(re.fullmatch(r"\d+", row["seconds"]), row["seconds"])
        self.assertIn("REGRESSION RESULT: PASS", result.stdout)
        self.assertIn("BUILD_NUMBER=2743", result.stdout)
        self.assertTrue(result.stdout.strip().endswith("CLEANUP unregister exit=0"), result.stdout[-300:])

    def test_gate_table_has_59_entries_including_unittest(self):
        listing = _run_plain([TEST_ALL, "--list"])
        self.assertEqual(listing.returncode, 0, listing.stderr)
        lines = listing.stdout.splitlines()
        self.assertEqual(lines[0].split("\t"), ["id", "needs", "provides", "command"])
        ids = [line.split("\t")[0] for line in lines[1:]]
        # A real 59-gate table, not an empty header: exact count, unique ids,
        # every row carries a non-empty command.
        self.assertEqual(len(ids), 59, ids)
        self.assertEqual(len(set(ids)), 59, ids)
        self.assertEqual(ids, self.GATE_IDS)
        commands = {line.split("\t")[0]: line.split("\t")[3] for line in lines[1:]}
        for gate_id, command in commands.items():
            self.assertTrue(command.strip(), "gate %s has an empty command" % gate_id)
        # The suite runs the context-model gate in strict mode: a missing
        # model must FAIL there, not SKIP-with-exit-0 into a recorded PASS.
        self.assertIn("AIM_CONTEXT_MODEL_REQUIRED=1", commands["context_model_integration"])
        for required in ("unittest", "bootstrap_native", "build_release", "fuzzy_exact_priority_replay",
                         "offcorpus_probe", "q3b1_garble_replay", "q3b2_heavy_garble_replay",
                         "partial_coverage_top1_replay", "userdb_process_recovery", "install_input_method",
                         "build_manager", "artifact_checksum_equivalence"):
            self.assertIn(required, ids)
        # The checksum equivalence gate consumes the built bundle (it exits 2
        # on a missing bundle), so it must be blocked by a failed build.
        needs = {line.split("\t")[0]: line.split("\t")[1] for line in lines[1:]}
        self.assertEqual(needs["artifact_checksum_equivalence"], "bundle")

    def test_fuzzy_gate_exit6_fails_run_but_independent_gates_still_run(self):
        self.stub.control_exit("test_fuzzy_exact_priority_replay.sh", 6)
        result = self.run_entry(CYCLE)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        rows = self.stub.latest_rows()
        # The quality gate failed and is reported exactly as it exited.
        row = rows["fuzzy_exact_priority_replay"]
        self.assertEqual((row["status"], row["exit"]), ("FAIL", "6"))
        log = Path(row["log"]).read_text()
        self.assertIn("exiting 6", log)
        self.assertIn("test_fuzzy_exact_priority_replay.sh", log)
        # Every gate the old set -e run never reached still ran and passed.
        for gate_id in (
            "offcorpus_probe",
            "q3b1_garble_replay",
            "q3b2_heavy_garble_replay",
            "partial_coverage_top1_replay",
            "userdb_process_recovery",
            "install_input_method",
            "build_manager",
        ):
            self.assert_status(rows, gate_id, "PASS", 0)
        self.assertIn("REGRESSION RESULT: FAIL", result.stdout)
        self.assertIn("BUILD_NUMBER=2743", result.stdout)

    def test_release_build_failure_blocks_bundle_gates_stale_bundle_cannot_fake_pass(self):
        # The release build fails while a perfectly "passing" offcorpus stub
        # exists: it must never run, because a stale on-disk bundle is not
        # evidence about the current tree.
        self.stub.control_exit("build_native.sh", 1)
        result = self.run_entry(TEST_ALL)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        rows = self.stub.latest_rows()
        self.assert_status(rows, "build_release", "FAIL", 1)
        for gate_id in (
            "core_network_boundary",
            "offcorpus_probe",
            "q3b2_heavy_garble_replay",
            "fuzzy_exact_priority_replay",
            "userdb_process_recovery",
            "install_input_method",
        ):
            row = self.assert_status(rows, gate_id, "BLOCKED")
            self.assertEqual(row["log"], "-")
        # The offcorpus stub was never executed even though it would exit 0.
        self.assertEqual(self.stub.invocations("test_offcorpus_probe.sh"), [])
        # Gates that do not need the bundle still collected evidence.
        for gate_id in ("frontend_tests", "unittest", "dogfood_checkpoint", "cargo_test_core",
                        "swift_tests", "context_model_integration", "userdb_word_cleanup"):
            self.assert_status(rows, gate_id, "PASS", 0)
        self.assertIn("REGRESSION RESULT: FAIL", result.stdout)

    def test_failed_core_build_blocks_swift_linkers_not_stale_core(self):
        # swift_tests and context_model_integration link the current Rust core;
        # a failed cargo_build_core_release must block them instead of letting
        # them link whatever stale library is on disk.
        self.stub.control_exit("cargo", 1)
        result = self.run_entry(TEST_ALL)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        rows = self.stub.latest_rows()
        for gate_id in ("cargo_test_src_tauri", "cargo_test_core", "cargo_build_core_release"):
            self.assert_status(rows, gate_id, "FAIL", 1)
        for gate_id in ("swift_tests", "context_model_integration", "fuzzy_exact_priority_replay"):
            row = self.assert_status(rows, gate_id, "BLOCKED")
            self.assertIn("core_lib", row["needs"].split(","))
        self.assertEqual(self.stub.invocations("swift"), [])
        # Independent gates still ran.
        for gate_id in ("frontend_tests", "unittest", "owner_error_ranking"):
            self.assert_status(rows, gate_id, "PASS", 0)

    def test_bootstrap_failure_blocks_only_toolchain_dependents(self):
        self.stub.control_exit("bootstrap_native.sh", 4)
        result = self.run_entry(TEST_ALL)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        rows = self.stub.latest_rows()
        self.assert_status(rows, "bootstrap_native", "FAIL", 4)
        for gate_id in ("cargo_test_core", "swift_tests", "build_release", "fuzzy_exact_priority_replay", "build_manager"):
            row = self.assert_status(rows, gate_id, "BLOCKED")
            self.assertIn("deps", row["needs"].split(","))
        for gate_id in ("frontend_tests", "unittest", "owner_error_ranking", "replay_gate_contract",
                       "userdb_word_cleanup"):
            self.assert_status(rows, gate_id, "PASS", 0)

    def test_any_step_nonzero_fails_whole_run(self):
        cases = [
            ("test_owner_error_ranking.sh", "owner_error_ranking", 6),
            ("npm", "frontend_tests", 1),
            ("build_manager.sh", "build_manager", 2),
            ("cargo", "cargo_test_core", 127),
            ("test_partial_coverage_top1_replay.sh", "partial_coverage_top1_replay", 6),
        ]
        for name, gate_id, code in cases:
            with self.subTest(gate=gate_id, code=code):
                # A fresh stub per case: control files from one case must not
                # block unrelated gates of the next.
                stub = self.make_stub()
                stub.install_scripts(self.SCRIPT_NAMES | {"unregister_dev_bundles.sh"})
                stub.control_exit(name, code)
                result = self.run_entry(TEST_ALL, stub=stub)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                rows = stub.latest_rows()
                self.assert_status(rows, gate_id, "FAIL", code)
                self.assertIn("REGRESSION RESULT: FAIL", result.stdout)

    def test_each_gate_script_runs_exactly_once_no_duplicate_build(self):
        result = self.run_entry(CYCLE)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for name in sorted(self.SCRIPT_NAMES):
            count = len(self.stub.invocations(name))
            self.assertEqual(count, 1, "%s ran %d times" % (name, count))
        # The old cycle built the release bundle twice (wrapper + test_all).
        self.assertEqual(len(self.stub.invocations("build_native.sh")), 1)
        self.assertIn("release", self.stub.invocations("build_native.sh")[0])
        # Exactly one runner layer: the suite's EXIT trap runs unregister once.
        self.assertEqual(len(self.stub.invocations("unregister_dev_bundles.sh")), 1)
        # The unittest gate discovered both directories and did not recurse.
        python_calls = self.stub.invocations("python3")
        self.assertEqual(len(python_calls), 2, python_calls)
        for call in python_calls:
            self.assertIn("unittest", call)
            self.assertIn("discover", call)

    def test_list_mode_lists_gates_without_running_anything(self):
        result = _run_plain([TEST_ALL, "--list"])
        self.assertEqual(result.returncode, 0, result.stderr)
        # Listing is a pure dry run: no stub was even installed for this case
        # beyond setUp, and nothing executed.
        self.assertEqual(os.listdir(self.stub.record), [])
        self.assertFalse(self.stub.runs.exists())

    def test_stub_mode_is_persisted_in_metadata_and_result_line(self):
        # A synthetic green run must be recognizable from the SAVED evidence
        # alone: metadata.tsv next to summary.tsv carries the stub/synthetic
        # flag, and the RESULT line repeats it (not only the START echo).
        result = self.run_entry(CYCLE)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertRegex(result.stdout, r"REGRESSION START suite=test_all stub=1\b")
        self.assertRegex(result.stdout, r"REGRESSION RESULT: PASS\b.*\bstub=1 synthetic=1\b")
        run_dir = self.stub.summaries()[-1].parent
        metadata = run_dir / "metadata.tsv"
        self.assertTrue(metadata.is_file(), str(metadata))
        rows = {}
        for line in metadata.read_text().splitlines()[1:]:
            key, _, value = line.partition("\t")
            rows[key] = value
        self.assertEqual(rows["stub"], "1")
        self.assertEqual(rows["synthetic"], "1")
        self.assertEqual(rows["stub_root"], str(self.stub.base))
        self.assertEqual(rows["suite"], "test_all")
        self.assertEqual(rows["root"], str(REPO))

    def test_metadata_records_real_mode_without_stub_root(self):
        # The lib itself (no AIM_REGRESSION_STUB_DIR) must persist stub=0:
        # provenance is written for every run, not only synthetic ones.  This
        # probe never executes a gate and never installs the EXIT trap, so no
        # cleanup script and no real tool can run.
        with tempfile.TemporaryDirectory(prefix="aim-regression-metadata.") as tmp:
            probe = (
                'source "%s"; regression_env_init probe; regression_open_run_dir; '
                'printf "RUN_DIR=%%s\\n" "$REG_RUN_DIR"' % (REPO / "scripts" / "tools" / "regression_gate_lib.sh")
            )
            env = dict(os.environ)
            for var in ("AIM_REGRESSION_STUB_DIR", "AIM_REGRESSION_RUN_DIR"):
                env.pop(var, None)
            env["AIM_REGRESSION_RUN_DIR"] = str(Path(tmp) / "base")
            result = subprocess.run(
                ["bash", "-c", probe], capture_output=True, text=True, timeout=30, env=env
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            run_dir = next(
                line[len("RUN_DIR="):] for line in result.stdout.splitlines()
                if line.startswith("RUN_DIR=")
            )
            rows = {}
            for line in (Path(run_dir) / "metadata.tsv").read_text().splitlines()[1:]:
                key, _, value = line.partition("\t")
                rows[key] = value
            self.assertEqual((rows["stub"], rows["synthetic"]), ("0", "0"))
            self.assertEqual(rows["stub_root"], "-")
            self.assertEqual(rows["suite"], "probe")

    def run_context_gate_direct(self, extra_env=None):
        # Runs the real script/test_context_model_integration.sh with every
        # dependency stubbed: python3 stands in for the context_model tools
        # (synthetic availability.json only — no model data is read or built),
        # AIM_CONTEXT_MODEL_PYTHON/HARNESS point at recording stubs, so no
        # swift build and no external resource is touched.  Unlike the suite
        # entries, a direct run does not go through regression_gate_lib.sh
        # (which is what prepends the stub bin to PATH), so PATH is shadowed
        # here explicitly — with the real python3 the gate would copy the
        # real model bundle and read private model data.
        self.stub.install(["python3"], CONTEXT_PYTHON_STUB)
        self.stub.install(["zhizi-python", "context-harness"])
        env = self.stub.env({
            "AIM_CONTEXT_MODEL_PYTHON": str(self.stub.bin / "zhizi-python"),
            "AIM_CONTEXT_MODEL_HARNESS": str(self.stub.bin / "context-harness"),
            **(extra_env or {}),
        })
        env["PATH"] = "%s%s%s" % (self.stub.bin, os.pathsep, env.get("PATH", ""))
        return subprocess.run(
            [str(REPO / "script" / "test_context_model_integration.sh")],
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )

    def test_context_gate_default_mode_keeps_optional_skip(self):
        # Direct invocation (no AIM_CONTEXT_MODEL_REQUIRED) keeps the original
        # optional behavior: missing model SKIPs with exit 0.
        (self.stub.control / "model.unavailable").touch()
        result = self.run_context_gate_direct()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("context_model_alignment=SKIP reason=model-files-missing-or-sha256-mismatch", result.stdout)
        self.assertIn("context_model_rerank_performance=SKIP", result.stdout)
        # The unavailable branch was really taken: the probe saw "available": false.
        self.assertIn("stub availability probe: unavailable", result.stdout)

    def test_context_gate_strict_mode_fails_when_model_unavailable(self):
        (self.stub.control / "model.unavailable").touch()
        result = self.run_context_gate_direct(extra_env={"AIM_CONTEXT_MODEL_REQUIRED": "1"})
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("context_model_alignment=FAIL reason=model-required-but-unavailable", result.stdout)
        self.assertIn("context_model_rerank_performance=FAIL", result.stdout)
        self.assertNotIn("context_model_alignment=SKIP", result.stdout)

    def test_context_gate_available_model_still_runs_alignment(self):
        # With the model available the gate must proceed to the alignment and
        # paced-rerank evidence, strict or not.
        for required in ({"AIM_CONTEXT_MODEL_REQUIRED": "1"}, {}):
            with self.subTest(strict=required):
                stub = self.make_stub()
                stub.install(["python3"], CONTEXT_PYTHON_STUB)
                stub.install(["zhizi-python", "context-harness"])
                env = stub.env({
                    "AIM_CONTEXT_MODEL_PYTHON": str(stub.bin / "zhizi-python"),
                    "AIM_CONTEXT_MODEL_HARNESS": str(stub.bin / "context-harness"),
                    **required,
                })
                env["PATH"] = "%s%s%s" % (stub.bin, os.pathsep, env.get("PATH", ""))
                result = subprocess.run(
                    [str(REPO / "script" / "test_context_model_integration.sh")],
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=60,
                )
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("stub availability probe: available", result.stdout)
                alignment = [c for c in stub.invocations("zhizi-python") if "check_host_alignment.py" in c]
                self.assertEqual(len(alignment), 1, stub.invocations("zhizi-python"))
                self.assertIn("--harness", alignment[0])
                paced = [c for c in stub.invocations("context-harness") if "--paced" in c]
                self.assertEqual(len(paced), 1, stub.invocations("context-harness"))

    def test_explicit_run_dir_is_a_parent_new_child_every_run(self):
        with tempfile.TemporaryDirectory(prefix="aim-regression-rundir.") as tmp:
            base = Path(tmp) / "explicit"
            for _ in range(2):
                result = self.run_entry(TEST_ALL, extra_env={"AIM_REGRESSION_RUN_DIR": str(base)})
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            # No summary is ever written into the base itself and no earlier
            # evidence is overwritten: each run gets a fresh child directory.
            self.assertFalse((base / "summary.tsv").exists())
            children = sorted(p.parent for p in base.glob("*/summary.tsv"))
            self.assertEqual(len(children), 2, [c.name for c in base.iterdir()])
            for child in children:
                rows = self._load_summary(child / "summary.tsv")
                self.assertEqual(len(rows), len(self.GATE_IDS))
            # Repeatedly naming the same base keeps appending, never truncates.
            self.assertIn("log_dir=%s" % children[-1], result.stdout)

    def _load_summary(self, path):
        rows = {}
        with path.open() as handle:
            header = handle.readline().rstrip("\n").split("\t")
            self.assertEqual(header, SUMMARY_HEADER)
            for line in handle:
                fields = line.rstrip("\n").split("\t")
                row = dict(zip(header, fields))
                rows[row["id"]] = row
        return rows

    def test_sigterm_interrupts_cleans_up_and_never_passes(self):
        self.stub.control_sleep("npm")
        proc = subprocess.Popen(
            [str(TEST_ALL)],
            env=self.stub.env(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            deadline = time.time() + 30
            while time.time() < deadline:
                if self.stub.invocations("npm"):
                    break
                time.sleep(0.1)
            else:
                self.fail("npm gate never started")
            proc.send_signal(signal.SIGTERM)
            stdout, stderr = proc.communicate(timeout=20)
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.communicate()
            subprocess.run(["pkill", "-f", "sleep 9713"], capture_output=True)
        self.assertEqual(proc.returncode, 143, stdout + stderr)
        # The cleanup trap still ran (stubbed unregister) and said so.
        self.assertIn("CLEANUP unregister exit=", stdout, stdout + stderr)
        self.assertGreaterEqual(len(self.stub.invocations("unregister_dev_bundles.sh")), 1)
        rows = self.stub.latest_rows()
        self.assertEqual(rows["frontend_tests"]["status"], "INTERRUPTED")
        blocked = [r for r in rows.values() if r["status"] == "BLOCKED"]
        self.assertGreater(len(blocked), 0)
        self.assertIn("REGRESSION RESULT: FAIL", stdout)

    def test_unwritable_log_dir_is_harness_error(self):
        self.stub.runs.mkdir(parents=True)
        self.stub.runs.chmod(0o555)
        self.addCleanup(lambda: self.stub.runs.chmod(0o755))
        result = self.run_entry(TEST_ALL)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("REGRESSION HARNESS ERROR", result.stderr)
        self.assertIn("cannot create log dir", result.stderr)


if __name__ == "__main__":
    unittest.main()
