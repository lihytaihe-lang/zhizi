"""Check exact-real normalization representations before hardware conversion."""
import unittest
import torch
import numpy as np
from torch.nn import functional as F

from gemm_norm import gemm_layer_norm
from export_split import HiddenBody
from split_tail import FP32Tail
from model import Config,ZhiziLM


class GemmNormalizationTests(unittest.TestCase):
    def test_both_means_match_independent_float64_layernorm(self):
        torch.manual_seed(27)
        for dimension in [16,32,512]:
            x=torch.randn(2,3,dimension,dtype=torch.float64)*.2+2
            gamma=torch.randn(dimension,dtype=torch.float64);beta=torch.randn(dimension,dtype=torch.float64)
            mean_weight=torch.full((dimension,1),1/dimension,dtype=torch.float64)
            expected=F.layer_norm(x,(dimension,),gamma,beta,1e-5)
            for hierarchical in [False,True]:
                with self.subTest(dimension=dimension,hierarchical=hierarchical):
                    actual=gemm_layer_norm(x,gamma,beta,1e-5,dimension,mean_weight,hierarchical)
                    torch.testing.assert_close(actual,expected,rtol=1e-10,atol=1e-10)

    def test_constant_input_and_arguments_are_unchanged(self):
        x=torch.full((2,32),3.0);g=torch.ones(32);b=torch.arange(32,dtype=torch.float32);m=torch.full((32,1),1/32)
        copies=[v.clone() for v in [x,g,b,m]]
        for h in [False,True]:torch.testing.assert_close(gemm_layer_norm(x,g,b,1e-5,32,m,h),b.expand_as(x))
        for value,copy in zip([x,g,b,m],copies):torch.testing.assert_close(value,copy,rtol=0,atol=0)

    def test_invalid_fixed_arguments_rejected(self):
        x=torch.ones(2,32);g=torch.ones(32);b=torch.zeros(32);m=torch.full((32,1),1/32)
        for eps,dim,mean,h in [(0,32,m,False),(True,32,m,False),(float('nan'),32,m,False),
            (float('inf'),32,m,False),(1e-5,True,m,False),(1e-5,0,m,False),(1e-5,32,None,False),
            (1e-5,8,m,True),(1e-5,32,m,'yes')]:
            with self.subTest(eps=eps,dim=dim,h=h),self.assertRaises(ValueError):gemm_layer_norm(x,g,b,eps,dim,mean,h)

    def test_trace_keeps_fixed_dimensions(self):
        class Probe(torch.nn.Module):
            def __init__(self,h):
                super().__init__();self.h=h;self.register_buffer('g',torch.ones(32));self.register_buffer('b',torch.zeros(32));self.register_buffer('m',torch.full((32,1),1/32))
            def forward(self,x):return gemm_layer_norm(x,self.g,self.b,1e-5,32,self.m,self.h)
        x=torch.randn(2,3,32)
        for h in [False,True]:
            model=Probe(h);traced=torch.jit.trace(model,(x,));torch.testing.assert_close(traced(x+1),model(x+1))
            self.assertNotIn('aten::Int',str(traced.graph))

    def test_whole_fp32_split_matches_original_language_model(self):
        torch.manual_seed(29)
        lm=ZhiziLM(Config(vocab_size=23,block_size=16,n_layer=2,n_head=4,d_model=32)).eval()
        tokens=torch.tensor([[1,4,5,6],[1,7,0,0]],dtype=torch.int32);position=torch.tensor([[0,0,0,1],[0,1,0,0]],dtype=torch.float16)
        tail=FP32Tail(lm.ln_f.weight.detach().numpy(),lm.ln_f.bias.detach().numpy(),lm.head.weight.detach().numpy())
        with torch.no_grad():
            expected=np.vstack([F.log_softmax(lm(tokens[i:i+1,:n].long())[0,-1],-1).numpy() for i,n in enumerate([4,2])])
            for mode in ['gemm-stats','hierarchical-stats','aligned-gemm-stats']:
                got=tail(HiddenBody(lm,normalization=mode).eval()(tokens,position).numpy())
                np.testing.assert_allclose(got,expected,atol=1e-6)


if __name__=='__main__':unittest.main()
