"""Independent output-tail and split-graph checks against the original Torch model."""
import unittest

import numpy as np
import torch
from torch.nn import functional as F

from split_tail import FP32Tail
from export_split import HiddenBody, LastBlockTail
from export_dist import PositionDistribution
from model import Config, ZhiziLM


class SplitTailTests(unittest.TestCase):
    def setUp(self):
        rng=np.random.default_rng(42)
        self.gamma=rng.normal(size=32).astype(np.float32)
        self.beta=rng.normal(size=32).astype(np.float32)
        self.weight=rng.normal(size=(97,32)).astype(np.float32)
        self.hidden=rng.normal(size=(8,32)).astype(np.float32)

    def test_numpy_tail_matches_original_layernorm_linear_and_logsoftmax(self):
        actual=FP32Tail(self.gamma,self.beta,self.weight)(self.hidden)
        expected=F.log_softmax(F.linear(F.layer_norm(torch.from_numpy(self.hidden),(32,),
            torch.from_numpy(self.gamma),torch.from_numpy(self.beta)),torch.from_numpy(self.weight)),-1).numpy()
        np.testing.assert_allclose(actual,expected,rtol=5e-6,atol=2e-5)
        np.testing.assert_allclose(np.exp(actual).sum(-1),np.ones(8),atol=2e-6)
        self.assertEqual(actual.dtype,np.float32)
        self.assertTrue(actual.flags.c_contiguous)

    def test_parameters_are_owned_and_input_is_never_modified(self):
        tail=FP32Tail(self.gamma,self.beta,self.weight);before=tail(self.hidden)
        hidden=self.hidden.copy();self.gamma.fill(99);self.beta.fill(99);self.weight.fill(99)
        np.testing.assert_array_equal(tail(self.hidden),before)
        np.testing.assert_array_equal(self.hidden,hidden)

    def test_noncontiguous_input_and_single_dimension_normalization(self):
        tail=FP32Tail(np.ones(1,np.float32),np.zeros(1,np.float32),np.ones((3,1),np.float32))
        value=np.arange(8,dtype=np.float32).reshape(4,2)[:,::2]
        np.testing.assert_allclose(tail(value),np.full((4,3),-np.log(3),np.float32),atol=1e-7)

    def test_unrepresentable_epsilon_is_rejected_at_construction(self):
        for epsilon in [1e-100,1e100]:
            with self.subTest(epsilon=epsilon),self.assertRaises(ValueError):
                FP32Tail(self.gamma,self.beta,self.weight,epsilon)

    def test_invalid_parameters_epsilon_and_hidden_rejected(self):
        for epsilon in [True,0,-1,float('nan'),float('inf')]:
            with self.subTest(epsilon=epsilon),self.assertRaises(ValueError):
                FP32Tail(self.gamma,self.beta,self.weight,epsilon)
        for gamma,beta,weight in [(self.gamma.astype(np.float16),self.beta,self.weight),
            (self.gamma,self.beta[:5],self.weight),(self.gamma,self.beta,self.weight[:2])]:
            with self.subTest(shape=weight.shape),self.assertRaises(ValueError):FP32Tail(gamma,beta,weight)
        tail=FP32Tail(self.gamma,self.beta,self.weight)
        for hidden in [self.hidden.astype(np.float16),np.zeros((9,32),np.float32),
            np.zeros((0,32),np.float32),np.full((1,32),np.nan,np.float32)]:
            with self.subTest(shape=hidden.shape),self.assertRaises(ValueError):tail(hidden)

    def test_finite_inputs_that_overflow_do_not_return_corrupt_distribution(self):
        tail=FP32Tail(self.gamma,self.beta,self.weight)
        with self.assertRaises(ValueError):tail(np.full((1,32),1e38,np.float32))

    def test_full_precision_selected_hidden_tail_matches_original_model(self):
        torch.manual_seed(7)
        lm=ZhiziLM(Config(vocab_size=23,block_size=16,n_layer=2,n_head=4,d_model=32)).eval()
        tokens=torch.tensor([[1,4,5,6],[1,7,0,0]],dtype=torch.int32)
        position=torch.tensor([[0,0,0,1],[0,1,0,0]],dtype=torch.float16)
        with torch.no_grad():
            hidden=HiddenBody(lm).eval()(tokens,position).numpy()
            reference=np.vstack([F.log_softmax(lm(tokens[i:i+1,:n].long())[0,-1],-1).numpy()
                for i,n in enumerate([4,2])])
        tail=FP32Tail(lm.ln_f.weight.detach().numpy(),lm.ln_f.bias.detach().numpy(),lm.head.weight.detach().numpy())
        np.testing.assert_allclose(tail(hidden),reference,atol=1e-6)

    def test_last_block_split_preserves_original_full_precision_graph(self):
        torch.manual_seed(9)
        lm=ZhiziLM(Config(vocab_size=23,block_size=16,n_layer=2,n_head=4,d_model=32)).eval()
        tokens=torch.tensor([[1,4,5,6],[1,7,8,9]],dtype=torch.int32)
        position=torch.tensor([[0,0,0,1],[0,1,0,0]],dtype=torch.float16)
        with torch.no_grad():
            hidden=HiddenBody(lm,last_block_in_tail=True).eval()(tokens,position)
            actual=LastBlockTail(lm).eval()(hidden,position)
            expected=PositionDistribution(lm).eval()(tokens,position)
        torch.testing.assert_close(actual,expected,rtol=1e-6,atol=1e-6)


if __name__=='__main__':unittest.main()
