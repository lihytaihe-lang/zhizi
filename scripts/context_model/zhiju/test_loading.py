import importlib.util
import unittest
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("zhiju_generate", ROOT / "scripts/eval/zhizi_generate/generate.py")
gen = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gen)


class FakeSession:
    def __init__(self, output):
        self.output = output
        self.requested = None

    def run(self, names, feed):
        self.requested = names
        return [self.output]


class LoadingTests(unittest.TestCase):
    def test_v3_distribution_is_used_directly(self):
        expected = np.array([[-1.0, -2.0, -3.0]], np.float32)
        session = FakeSession(expected)
        result = gen.OnnxNext(session, distribution=True).logprobs([[1, 3]])
        np.testing.assert_array_equal(result, expected)
        self.assertEqual(session.requested, ["logprobs"])

    def test_default_still_reads_v2_logits_and_normalizes(self):
        session = FakeSession(np.array([[[8.0, -2.0, 3.0], [1.0, 3.0, 2.0]]], np.float32))
        result = gen.OnnxNext(session).logprobs([[1, 3]])
        expected = np.array([[1.0, 3.0, 2.0]])
        expected -= np.log(np.exp(expected).sum(-1, keepdims=True))
        np.testing.assert_allclose(result, expected)
        self.assertEqual(session.requested, ["/head/MatMul_output_0"])


if __name__ == "__main__":
    unittest.main()
