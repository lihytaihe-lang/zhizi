"""Batched harness invocation in check_host_alignment keeps gate semantics.

A fake harness stands in for AgentIMEContextModelHarness: it derives every
report field from each case's content (so batched and unbatched runs produce
byte-identical rows) and can abort or drop rows via inherited environment
variables, mimicking the Core ML E5 IOSurface exhaustion the real binary hits
after ~16k consecutive inferences in one process.
Run: (cd scripts/context_model/zhizi && PYTHONDONTWRITEBYTECODE=1 \
 ime_context_model/.venv/bin/python -m unittest test_check_host_alignment_batching -v)
"""
import importlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

FAKE_HARNESS = f'''#!{sys.executable}
import json, os, sys
cases = json.loads(open(sys.argv[2]).read())
log = os.environ.get('FAKE_HARNESS_LOG')
if log:
    with open(log, 'a') as handle:
        handle.write(f"{{len(cases)}}\\n")
if len(cases) > int(os.environ.get('FAKE_HARNESS_CRASH_ABOVE', '1000000000')):
    sys.exit(134)
boom = os.environ.get('FAKE_HARNESS_BOOM_CONTEXT')
if boom and any(case['context'] == boom for case in cases):
    sys.exit(134)
rows = []
for case in cases:
    scores = [float(len(candidate) * 10 + sum(map(ord, case['context'])) % 7)
              for candidate in case['candidates']]
    order = sorted(range(len(scores)), key=lambda i: (-scores[i], i))
    rows.append({{'scores': scores, 'order': order,
                  'elapsedNanoseconds': 1_000_000 * (1 + len(case['context']))
                                        + 1_000 * len(case['candidates'])}})
if os.environ.get('FAKE_HARNESS_DROP_ROWS'):
    rows = rows[:-1]
json.dump({{'loadMilliseconds': 123.0, 'rows': rows}}, open(sys.argv[3], 'w'))
'''


def make_cases(count):
    groups = [['aa', 'ab', 'b'], ['x', 'xy', 'yy', 'yz'], ['q', 'qq']]
    cases = []
    for i in range(count):
        group = groups[i % len(groups)]
        shift = i % len(group)
        cases.append({'context': f'c{i % 13}', 'candidates': group[shift:] + group[:shift]})
    return cases


class FakeScorer:
    def score(self, context, candidates):
        return [float((sum(map(ord, context)) * 7 + i * 3) % 11) for i, _ in enumerate(candidates)]


def harness_top_pick(case):
    scores = [float(len(candidate) * 10 + sum(map(ord, case['context'])) % 7)
              for candidate in case['candidates']]
    return sorted(range(len(scores)), key=lambda i: (-scores[i], i))[0]


def reference_pick(case):
    scores = FakeScorer().score(case['context'], case['candidates'])
    return max(range(len(scores)), key=lambda i: scores[i] - 2 * i)


class BatchingTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        torch = types.ModuleType('torch')
        torch.set_num_threads = lambda _: None
        eval_owner = types.ModuleType('eval_owner')
        eval_owner.ZhiziScorer = object
        with patch.dict(sys.modules, {'torch': torch, 'eval_owner': eval_owner}):
            cls.cha = importlib.import_module('check_host_alignment')

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.harness = root / 'fake_harness'
        self.harness.write_text(FAKE_HARNESS)
        self.harness.chmod(self.harness.stat().st_mode | stat.S_IXUSR)
        self.log = root / 'invocations.log'
        self.scorer = FakeScorer()
        self.directory = root / 'ContextModel'
        self.directory.mkdir()

    def tearDown(self):
        self.temporary.cleanup()

    def evaluate(self, cases, batch_size=None, aim_batch=None, **environment):
        if self.log.exists():
            self.log.unlink()
        variables = {key: value for key, value in os.environ.items() if key != 'AIM_ALIGNMENT_BATCH'}
        variables['FAKE_HARNESS_LOG'] = str(self.log)
        if aim_batch is not None:
            variables['AIM_ALIGNMENT_BATCH'] = aim_batch
        variables.update(environment)
        with patch.dict(os.environ, variables, clear=True):
            return self.cha.evaluate(cases, self.scorer, str(self.harness),
                                     str(self.directory), batch_size=batch_size)

    def invocations(self):
        if not self.log.exists():
            return []
        return [int(line) for line in self.log.read_text().splitlines()]

    def test_default_batch_and_environment_override(self):
        self.assertEqual(self.evaluate(make_cases(9))['cases'], 9)
        self.assertEqual(self.invocations(), [9])
        self.evaluate(make_cases(9), batch_size=2)
        self.assertEqual(self.invocations(), [2, 2, 2, 2, 1])
        self.evaluate(make_cases(7), aim_batch='3')
        self.assertEqual(self.invocations(), [3, 3, 1])

    def test_batch_boundaries_produce_no_spurious_or_missing_batches(self):
        with self.assertRaises((ZeroDivisionError, IndexError)):
            self.evaluate([])
        self.assertEqual(self.invocations(), [])
        self.evaluate(make_cases(5), batch_size=5)
        self.assertEqual(self.invocations(), [5])
        self.evaluate(make_cases(4), batch_size=5)
        self.assertEqual(self.invocations(), [4])
        self.evaluate(make_cases(6), batch_size=5)
        self.assertEqual(self.invocations(), [5, 1])

    def test_over_limit_input_fails_unbatched_and_passes_batched(self):
        cases = make_cases(17)
        with self.assertRaises(subprocess.CalledProcessError):
            self.evaluate(cases, batch_size=1000, FAKE_HARNESS_CRASH_ABOVE='5')
        result = self.evaluate(cases, batch_size=5, FAKE_HARNESS_CRASH_ABOVE='5')
        self.assertEqual(self.invocations(), [5, 5, 5, 2])
        self.assertEqual(result['cases'], 17)
        self.assertEqual(result['agree'] / result['cases'], result['agreement'])

    def test_mid_run_batch_crash_fails_the_whole_evaluation(self):
        cases = make_cases(17)
        cases[10]['context'] = '引爆'
        with self.assertRaises(subprocess.CalledProcessError):
            self.evaluate(cases, batch_size=4, FAKE_HARNESS_BOOM_CONTEXT='引爆')
        self.assertEqual(self.invocations(), [4, 4, 4])

    def test_row_count_mismatch_fails(self):
        with self.assertRaises(AssertionError):
            self.evaluate(make_cases(9), batch_size=4, FAKE_HARNESS_DROP_ROWS='1')
        self.assertEqual(self.invocations(), [4])
        with self.assertRaises(AssertionError):
            self.evaluate(make_cases(9), batch_size=1000, FAKE_HARNESS_DROP_ROWS='1')

    def test_merged_result_matches_single_call_field_for_field(self):
        cases = make_cases(53)
        single = self.evaluate(cases, batch_size=1000)
        self.assertEqual(self.invocations(), [53])
        by_seven = self.evaluate(cases, batch_size=7)
        one_each = self.evaluate(cases, batch_size=1)
        self.assertEqual(single, by_seven)
        self.assertEqual(single, one_each)
        expected_agree = sum(1 for case in cases if reference_pick(case) == harness_top_pick(case))
        self.assertEqual(single['cases'], 53)
        self.assertEqual(single['agree'], expected_agree)
        self.assertEqual(single['agreement'], expected_agree / 53)

    def test_public_synthetic_set_stays_a_single_batch(self):
        result = self.evaluate(self.cha.synthetic_cases())
        self.assertEqual(result['cases'], 360)
        self.assertEqual(self.invocations(), [360])
        self.assertEqual(result['modelLoadAndWarmupMilliseconds'], 123.0)


if __name__ == '__main__':
    unittest.main()
