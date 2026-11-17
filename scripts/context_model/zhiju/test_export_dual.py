"""Small synthetic models only; no training data or owner records.

Run with ime_context_model/.venv/bin/python -m unittest scripts.context_model.zhiju.test_export_dual
"""
import sys
import unittest
from pathlib import Path

import torch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "zhizi"))
from model import Config, ZhiziLM
from export_coreml_spike import StaticScorer
from export_dist import FullDistribution
from export_dual import DualScorer


class DualOutputTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        torch.set_num_threads(1)
        torch.manual_seed(42)
        cls.lm = ZhiziLM(Config(vocab_size=31, block_size=40, n_layer=2,
                               n_head=2, d_model=16)).eval()

    def test_shapes_and_logprobs_match_factory_gather(self):
        tokens = torch.randint(1, 31, (4, 8), dtype=torch.int32)
        with torch.no_grad():
            logprobs, next_logprobs = DualScorer(self.lm, 4, 8)(tokens)
            factory = StaticScorer(self.lm, 4, 8)(tokens)
        self.assertEqual(tuple(logprobs.shape), (4, 7))
        self.assertEqual(tuple(next_logprobs.shape), (4, 31))
        torch.testing.assert_close(logprobs, factory, atol=0, rtol=0)
        product = torch.randint(1, 31, (8, 40), dtype=torch.int32)
        with torch.no_grad():
            wide, dist = DualScorer(self.lm, 8, 40)(product)
        self.assertEqual(tuple(wide.shape), (8, 39))
        self.assertEqual(tuple(dist.shape), (8, 31))

    def test_next_logprobs_use_last_nonpad_including_empty_and_length_one(self):
        tokens = torch.zeros((4, 8), dtype=torch.int32)
        tokens[0, 0] = 5
        tokens[1, :5] = torch.tensor([1, 4, 6, 7, 9], dtype=torch.int32)
        tokens[2] = torch.arange(3, 11)
        # row 3 stays all padding; that row's defined position is 0
        lengths = (1, 5, 8, 1)
        with torch.no_grad():
            _, next_logprobs = DualScorer(self.lm, 4, 8)(tokens)
            for row, length in enumerate(lengths):
                expected = FullDistribution(self.lm)(tokens[row:row + 1, :length])[0]
                torch.testing.assert_close(next_logprobs[row], expected, atol=2e-6, rtol=0)
            end_of_padding = FullDistribution(self.lm)(tokens[3:4])[0]
        self.assertGreater(float((next_logprobs[3] - end_of_padding).abs().max()), 1e-3)
        self.assertGreater(float((next_logprobs[0] - next_logprobs[3]).abs().max()), 1e-3)


if __name__ == "__main__":
    unittest.main()
