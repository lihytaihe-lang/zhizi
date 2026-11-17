import sys
from pathlib import Path
import unittest
import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "zhizi"))
from model import Config, ZhiziLM
from cached_dist import CachedPrefill, CachedStep


class CacheMathTests(unittest.TestCase):
    def setUp(self):
        torch.manual_seed(19)
        torch.set_num_threads(2)
        self.lm = ZhiziLM(Config(vocab_size=31, block_size=40, n_layer=2, n_head=2, d_model=16)).eval()

    def test_prefill_and_steps_match_unpadded_original_after_parent_reordering(self):
        tokens = torch.randint(3, 31, (3, 12), dtype=torch.int32)
        tokens[:, 0] = 1
        prefix = tokens[:, :4].clone()
        x = torch.zeros((3, 8), dtype=torch.int32); x[:, :4] = prefix
        position = torch.zeros((3, 8)); position[:, 3] = 1
        with torch.no_grad():
            got, keys, values = CachedPrefill(self.lm, 3, 8)(x, position)
            torch.testing.assert_close(got, torch.log_softmax(self.lm(prefix.long())[:, -1], -1), atol=1e-6, rtol=1e-6)
            for length in range(5, 13):
                parents = torch.tensor([2, 0, 0]) if length % 2 else torch.tensor([1, 2, 0])
                prefix = torch.cat((prefix[parents], tokens[:, length-1:length]), dim=1)
                keys, values = keys[:, parents], values[:, parents]
                position = torch.zeros((3, 40)); position[:, length-1] = 1
                valid = torch.zeros((3, 40)); valid[:, :length] = 1
                got, keys, values = CachedStep(self.lm, 3)(prefix[:, -1:], position, valid, keys, values)
                torch.testing.assert_close(got, torch.log_softmax(self.lm(prefix.long())[:, -1], -1), atol=1e-6, rtol=1e-6)

    def test_future_cache_and_right_padding_cannot_affect_valid_scores(self):
        x = torch.randint(3, 31, (2, 16), dtype=torch.int32); x[:, 0] = 1
        position = torch.zeros((2, 16)); position[:, 5] = 1
        other = x.clone(); other[:, 6:] = torch.randint(3, 31, (2, 10), dtype=torch.int32)
        with torch.no_grad():
            got, keys, values = CachedPrefill(self.lm, 2, 16)(x, position)
            changed, _, _ = CachedPrefill(self.lm, 2, 16)(other, position)
            torch.testing.assert_close(got, changed, atol=1e-7, rtol=0)
            keys[:, :, :, 6:] = torch.randn_like(keys[:, :, :, 6:]) * 10
            values[:, :, :, 6:] = torch.randn_like(values[:, :, :, 6:]) * 10
            position = torch.zeros((2, 40)); position[:, 6] = 1
            valid = torch.zeros((2, 40)); valid[:, :7] = 1
            got, _, _ = CachedStep(self.lm, 2)(x[:, 6:7], position, valid, keys, values)
            torch.testing.assert_close(got, torch.log_softmax(self.lm(x[:, :7].long())[:, -1], -1), atol=1e-6, rtol=1e-6)


if __name__ == "__main__":
    unittest.main()
