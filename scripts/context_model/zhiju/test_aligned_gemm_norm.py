"""Independent mathematical and trace checks for the fixed32 layout."""
import unittest
import torch
from torch.nn import functional as F
from aligned_gemm_norm import aligned_gemm_layer_norm


class AlignedGemmTests(unittest.TestCase):
    def test_matches_float64_layernorm_at_multiple_group_counts(self):
        torch.manual_seed(41)
        for dimension in [32,64,512]:
            with self.subTest(dimension=dimension):
                x=torch.randn(2,3,dimension,dtype=torch.float64)*.2+2
                gamma=torch.randn(dimension,dtype=torch.float64)
                beta=torch.randn(dimension,dtype=torch.float64)
                mean=torch.full((dimension,32),1/dimension,dtype=torch.float64)
                actual=aligned_gemm_layer_norm(x,gamma,beta,1e-5,dimension,mean)
                torch.testing.assert_close(actual,F.layer_norm(x,(dimension,),gamma,beta,1e-5),rtol=1e-10,atol=1e-10)

    def test_constant_input_preserves_affine_and_inputs(self):
        x=torch.full((2,3,64),3.0);g=torch.ones(64);b=torch.arange(64,dtype=torch.float32);m=torch.full((64,32),1/64)
        before=[value.clone() for value in [x,g,b,m]]
        torch.testing.assert_close(aligned_gemm_layer_norm(x,g,b,1e-5,64,m),b.expand_as(x))
        for value,copy in zip([x,g,b,m],before):torch.testing.assert_close(value,copy,rtol=0,atol=0)

    def test_invalid_fixed_arguments_are_rejected(self):
        x=torch.ones(2,3,64);g=torch.ones(64);b=torch.zeros(64);m=torch.full((64,32),1/64)
        for dim,eps in [(0,1e-5),(True,1e-5),(48,1e-5),(64,0),(64,-1),(64,float('nan')),(64,float('inf')),(64,True)]:
            with self.subTest(dimension=dim,epsilon=eps),self.assertRaises((ValueError,TypeError)):
                aligned_gemm_layer_norm(x,g,b,eps,dim,m)

    def test_trace_remains_input_dependent(self):
        class Probe(torch.nn.Module):
            def __init__(self):
                super().__init__();self.register_buffer('g',torch.ones(64));self.register_buffer('b',torch.zeros(64));self.register_buffer('m',torch.full((64,32),1/64))
            def forward(self,x):return aligned_gemm_layer_norm(x,self.g,self.b,1e-5,64,self.m)
        x=torch.randn(2,3,64);model=Probe();traced=torch.jit.trace(model,(x,))
        torch.testing.assert_close(traced(x+1),F.layer_norm(x+1,(64,)),rtol=1e-5,atol=1e-6)
        self.assertNotIn('aten::Int',str(traced.graph))


if __name__=='__main__':unittest.main()
