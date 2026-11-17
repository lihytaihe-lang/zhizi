"""Independent checks of GLM's estimator, including dtype aliases and failure atomicity."""
import contextlib
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest

import summarize_compute_plan as tool


def op(name='a', cost=.2, kind='linear', source=('fp16',), target=('fp16',)):
    return dict(name=name, operator=kind, device='GPU', estimatedCost=cost,
        inputDtypes=list(source), outputDtypes=list(target))


class ComputePlanTests(unittest.TestCase):
    def run_tool(self, payload, existing=None):
        with tempfile.TemporaryDirectory() as directory:
            source, dest = Path(directory)/'in.json', Path(directory)/'out.json'
            source.write_text(json.dumps(payload))
            if existing is not None:
                dest.write_text(existing)
            with contextlib.redirect_stderr(io.StringIO()):
                code = tool.main(['--input', str(source), '--out', str(dest)])
            return code, dest.read_text() if dest.exists() else None

    def test_float_dtype_aliases_count_each_cast_direction(self):
        payload = {'variants':[{'name':'short','operations':[
            op('up', kind='cast', source=['float16'], target=['float32']),
            op('down', kind='cast', source=['float32'], target=['float16'])]}]}
        code, text = self.run_tool(payload)
        self.assertEqual(code, 0)
        result = json.loads(text)['variants'][0]
        self.assertEqual(result['fp16ToFp32CastCount'], 1)
        self.assertEqual(result['fp32ToFp16CastCount'], 1)

    def test_mixed_aliases_and_non_cast_operator(self):
        payload = {'variants':[{'name':'short','operations':[
            op('up', kind='cast', source=['int32', 'fp16'], target=['float32']),
            op('linear', source=['fp16'], target=['fp32'])]}]}
        code, text = self.run_tool(payload)
        self.assertEqual(code, 0)
        result = json.loads(text)['variants'][0]
        self.assertEqual(result['fp16ToFp32CastCount'], 1)
        self.assertEqual(result['fp32ToFp16CastCount'], 0)

    def test_missing_costs_not_renormalized_and_ties_preserve_order(self):
        payload = {'variants':[{'name':'short','operations':[
            op('first', .3), op('unknown', None), op('second', .3), op('third', .1)]}]}
        original = copy.deepcopy(payload)
        code, text = self.run_tool(payload)
        self.assertEqual(code, 0)
        result = json.loads(text)['variants'][0]
        self.assertEqual(result['missingCostCount'], 1)
        self.assertAlmostEqual(result['costByDevice']['GPU'], .7)
        self.assertEqual([x['name'] for x in result['topOperations']], ['first','second','third'])
        self.assertEqual(payload, original)

    def test_empty_operation_variant_valid(self):
        code, text = self.run_tool({'variants':[{'name':'empty','operations':[]}]})
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(text)['variants'][0]['operationCount'], 0)

    def test_bad_cost_does_not_create_report(self):
        for cost in [-.1, 1.1, True, float('nan'), float('inf')]:
            with self.subTest(cost=cost):
                code, text = self.run_tool({'variants':[{'name':'bad','operations':[op(cost=cost)]}]})
                self.assertNotEqual(code, 0)
                self.assertIsNone(text)

    def test_duplicate_names_and_malformed_dtype_rejected(self):
        for payload in [
            {'variants':[{'name':'x','operations':[op(),op()]}]},
            {'variants':[{'name':'x','operations':[]},{'name':'x','operations':[]}]},
            {'variants':[{'name':'x','operations':[op(target=[''])]}]},
        ]:
            with self.subTest(payload=payload):
                code, text = self.run_tool(payload)
                self.assertNotEqual(code, 0)
                self.assertIsNone(text)

    def test_existing_output_preserved(self):
        code, text = self.run_tool({'variants':[{'name':'x','operations':[]}]}, 'preserve-me')
        self.assertNotEqual(code, 0)
        self.assertEqual(text, 'preserve-me')


if __name__ == '__main__':
    unittest.main()
