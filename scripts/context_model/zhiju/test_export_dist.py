"""Small synthetic models only; no training data or owner records.

Run with ime_context_model/.venv/bin/python -m unittest discover
    -s scripts/context_model/zhiju -p 'test_*.py'
"""
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
import torch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "zhizi"))
from model import Config, ZhiziLM
from export_onnx import Scorer
from export_dist import FullDistribution, IncrementalDistribution, PositionDistribution, export_onnx, export_position_onnx


class DistributionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        torch.set_num_threads(1)
        torch.manual_seed(42)
        cls.lm = ZhiziLM(Config(vocab_size=31, block_size=40, n_layer=2,
                               n_head=2, d_model=16)).eval()

    def test_full_matches_original_last_position(self):
        with torch.no_grad():
            for b, t in ((1, 1), (4, 16), (2, 40)):
                tokens = torch.randint(0, 31, (b, t), dtype=torch.int32)
                expected = torch.log_softmax(self.lm(tokens.long())[:, -1], -1)
                got = FullDistribution(self.lm)(tokens)
                torch.testing.assert_close(got, expected, atol=2e-6, rtol=0)
                torch.testing.assert_close(got.exp().sum(-1), torch.ones(b))

    def test_incremental_aligns_at_every_position_including_empty_cache(self):
        tokens = torch.randint(0, 31, (2, 40), dtype=torch.int32)
        keys = values = torch.empty(2, 2, 2, 0, 8)
        with torch.no_grad():
            for pos in range(40):
                got, keys, values = IncrementalDistribution(self.lm)(
                    tokens[:, pos:pos + 1], keys, values)
                expected = torch.log_softmax(self.lm(tokens[:, :pos + 1].long())[:, -1], -1)
                torch.testing.assert_close(got, expected, atol=3e-6, rtol=0)
                self.assertEqual(tuple(keys.shape), (2, 2, 2, pos + 1, 8))
                self.assertEqual(tuple(values.shape), tuple(keys.shape))

    def test_distribution_gathers_the_following_token(self):
        tokens = torch.randint(0, 31, (4, 20), dtype=torch.int32)
        with torch.no_grad():
            gathered = Scorer(self.lm)(tokens)[:, -1]
            dist = FullDistribution(self.lm)(tokens[:, :-1])
            got = dist.gather(1, tokens[:, -1:].long()).squeeze(1)
        torch.testing.assert_close(got, gathered, atol=3e-6, rtol=0)

    def test_static_shapes_preserve_full_and_incremental_math(self):
        tokens = torch.randint(0, 31, (4, 16), dtype=torch.int32)
        keys = values = torch.randn(2, 4, 2, 8, 8)
        with torch.no_grad():
            torch.testing.assert_close(FullDistribution(self.lm, shape=(4, 16))(tokens),
                                       FullDistribution(self.lm)(tokens))
            fixed = IncrementalDistribution(self.lm, shape=(4, 8))(tokens[:, :1], keys, values)
            dynamic = IncrementalDistribution(self.lm)(tokens[:, :1], keys, values)
            for got, expected in zip(fixed, dynamic):
                torch.testing.assert_close(got, expected)

    def test_position_selects_marked_step_for_short_full_and_mixed_rows(self):
        torch.manual_seed(20261111)
        model = PositionDistribution(self.lm)
        fixed = PositionDistribution(self.lm, shape=(4, 40))
        cases = (
            ("length1", (1, 1, 1, 1)),
            ("length40", (40, 40, 40, 40)),
            ("mixed", (1, 7, 18, 40)),
        )
        for name, lengths in cases:
            tokens = torch.zeros((4, 40), dtype=torch.int32)
            position = torch.zeros((4, 40), dtype=torch.float32)
            expected = []
            with torch.no_grad():
                for row, length in enumerate(lengths):
                    seq = torch.randint(1, 31, (length,), dtype=torch.int32)
                    tokens[row, :length] = seq
                    if length < 40:
                        tokens[row, length:] = torch.randint(1, 31, (40 - length,))
                    position[row, length - 1] = 1
                    expected.append(torch.log_softmax(self.lm(seq.view(1, -1).long())[:, -1], -1))
                expected = torch.cat(expected, 0)
                got = model(tokens, position)
                torch.testing.assert_close(got, expected, atol=2e-6, rtol=0, msg=name)
                torch.testing.assert_close(got.exp().sum(-1), torch.ones(4), msg=name)
                padded = tokens.clone()
                for row, length in enumerate(lengths):
                    padded[row, length:] = 0
                torch.testing.assert_close(model(padded, position), got, atol=0, rtol=0, msg=name)
                torch.testing.assert_close(fixed(tokens, position.half()), got, atol=2e-6, rtol=0, msg=name)
                if name == "length40":
                    torch.testing.assert_close(got, FullDistribution(self.lm)(tokens), atol=2e-6, rtol=0)
                for select in ("after_ln", "mul_before_ln", "mul_after_ln"):
                    alt = PositionDistribution(self.lm, select=select)
                    torch.testing.assert_close(alt(tokens, position), got, atol=2e-6, rtol=0, msg=select)

    def test_position_onnx_matches_marked_step(self):
        import onnxruntime as ort

        torch.manual_seed(7)
        lengths = (1, 40, 3, 19, 40, 2, 11, 28)
        tokens = torch.zeros((8, 40), dtype=torch.int32)
        position = torch.zeros((8, 40), dtype=torch.float16)
        expected = []
        with torch.no_grad():
            for row, length in enumerate(lengths):
                seq = torch.randint(1, 31, (length,), dtype=torch.int32)
                tokens[row, :length] = seq
                if length < 40:
                    tokens[row, length:] = torch.randint(1, 31, (40 - length,))
                position[row, length - 1] = 1
                expected.append(torch.log_softmax(self.lm(seq.view(1, -1).long())[:, -1], -1))
            expected = torch.cat(expected, 0).numpy()
        options = ort.SessionOptions()
        options.intra_op_num_threads = 1
        options.inter_op_num_threads = 1
        with tempfile.TemporaryDirectory() as folder:
            export_position_onnx(self.lm, Path(folder))
            session = ort.InferenceSession(str(Path(folder) / "position_b8_t40.onnx"), options,
                                           providers=["CPUExecutionProvider"])
            got = session.run(["logprobs"], {"tokens": tokens.numpy(), "position": position.numpy()})[0]
            zeros = tokens.clone()
            for row, length in enumerate(lengths):
                zeros[row, length:] = 0
            got_zero = session.run(["logprobs"], {"tokens": zeros.numpy(), "position": position.numpy()})[0]
        np.testing.assert_allclose(got, expected, atol=3e-6, rtol=0)
        np.testing.assert_allclose(got_zero, got, atol=0, rtol=0)

    def test_onnx_dynamic_batch_time_and_cache(self):
        import onnxruntime as ort

        options = ort.SessionOptions()
        options.intra_op_num_threads = 1
        options.inter_op_num_threads = 1
        with tempfile.TemporaryDirectory() as folder:
            export_onnx(self.lm, Path(folder))
            full = ort.InferenceSession(str(Path(folder) / "full.onnx"), options,
                                       providers=["CPUExecutionProvider"])
            step = ort.InferenceSession(str(Path(folder) / "incremental.onnx"), options,
                                       providers=["CPUExecutionProvider"])
            with torch.no_grad():
                for b, t in ((1, 1), (4, 16), (8, 40)):
                    tokens = torch.randint(0, 31, (b, t), dtype=torch.int32)
                    got = full.run(None, {"tokens": tokens.numpy()})[0]
                    expected = torch.log_softmax(self.lm(tokens.long())[:, -1], -1).numpy()
                    np.testing.assert_allclose(got, expected, atol=3e-6, rtol=0)
                    keys = values = np.empty((2, b, 2, 0, 8), np.float32)
                    for pos in range(t):
                        result = step.run(None, {"tokens": tokens[:, pos:pos + 1].numpy(),
                                                 "keys": keys, "values": values})
                        dist, keys, values = result
                    np.testing.assert_allclose(dist, expected, atol=3e-6, rtol=0)


if __name__ == "__main__":
    unittest.main()
