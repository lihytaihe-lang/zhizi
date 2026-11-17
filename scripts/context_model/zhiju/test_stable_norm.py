"""Independent mathematical and traceability checks for normalization alternatives."""
import unittest

import numpy as np
import torch
from torch.nn import functional as F

from stable_norm import stable_layer_norm
from export_split import HiddenBody
from split_tail import FP32Tail
from model import Config,ZhiziLM


class NormalizationTests(unittest.TestCase):
    def test_modes_match_float64_reference_with_affine_parameters(self):
        torch.manual_seed(15)
        x=torch.randn(2,5,32,dtype=torch.float64)*.1+3
        gamma=torch.randn(32,dtype=torch.float64);beta=torch.randn(32,dtype=torch.float64)
        expected=F.layer_norm(x,(32,),gamma,beta,1e-5)
        for mode in ['native','centered-native','centered-explicit']:
            with self.subTest(mode=mode):
                actual=stable_layer_norm(x,gamma,beta,1e-5,32,mode)
                torch.testing.assert_close(actual,expected,rtol=1e-10,atol=1e-10)

    def test_constant_input_noncontiguous_layout_and_no_mutation(self):
        x=torch.full((2,3,4),2.0).transpose(0,1);before=x.clone()
        gamma=torch.ones(4);beta=torch.arange(4,dtype=torch.float32)
        for mode in ['centered-native','centered-explicit']:
            out=stable_layer_norm(x,gamma,beta,1e-5,4,mode)
            torch.testing.assert_close(out,beta.expand_as(out))
            torch.testing.assert_close(x,before,rtol=0,atol=0)

    def test_invalid_fixed_python_arguments_rejected(self):
        x=torch.ones(2,4);g=torch.ones(4);b=torch.zeros(4)
        for dim,epsilon,mode in [(0,1e-5,'native'),(True,1e-5,'native'),(4,True,'native'),
            (4,0,'native'),(4,float('nan'),'native'),(4,float('inf'),'native'),(4,1e-5,'wrong')]:
            with self.subTest(dim=dim,epsilon=epsilon,mode=mode),self.assertRaises(ValueError):
                stable_layer_norm(x,g,b,epsilon,dim,mode)

    def test_all_modes_trace_without_tensor_to_python_conversion(self):
        class Wrapper(torch.nn.Module):
            def __init__(self,mode):
                super().__init__();self.mode=mode;self.weight=torch.nn.Parameter(torch.ones(4));self.bias=torch.nn.Parameter(torch.zeros(4))
            def forward(self,x):return stable_layer_norm(x,self.weight,self.bias,1e-5,4,self.mode)
        x=torch.randn(2,3,4)
        for mode in ['centered-native','centered-explicit']:
            wrapper=Wrapper(mode).eval();traced=torch.jit.trace(wrapper,(x,))
            torch.testing.assert_close(traced(x*2),wrapper(x*2))
            self.assertNotIn('aten::Int',str(traced.graph))

    def test_full_model_split_remains_equivalent_to_original_in_float32(self):
        torch.manual_seed(14)
        lm=ZhiziLM(Config(vocab_size=23,block_size=16,n_layer=2,n_head=4,d_model=32)).eval()
        tokens=torch.tensor([[1,4,5,6],[1,7,0,0]],dtype=torch.int32)
        position=torch.tensor([[0,0,0,1],[0,1,0,0]],dtype=torch.float16)
        tail=FP32Tail(lm.ln_f.weight.detach().numpy(),lm.ln_f.bias.detach().numpy(),lm.head.weight.detach().numpy())
        with torch.no_grad():
            expected=np.vstack([F.log_softmax(lm(tokens[i:i+1,:n].long())[0,-1],-1).numpy() for i,n in enumerate([4,2])])
            for mode in ['centered-native','centered-explicit']:
                actual=tail(HiddenBody(lm,normalization=mode).eval()(tokens,position).numpy())
                np.testing.assert_allclose(actual,expected,atol=1e-6)


if __name__=='__main__':unittest.main()
