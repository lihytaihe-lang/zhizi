"""Check the channel-first graph independently of hardware conversion."""
import unittest
import numpy as np
import torch
from torch.nn import functional as F
from export_split import HiddenBody
from model import Config,ZhiziLM
from split_tail import FP32Tail
from ane_body import ANEHiddenBody


class ANEBodyTests(unittest.TestCase):
    def setUp(self):
        torch.set_num_threads(1);torch.manual_seed(43)
        self.lm=ZhiziLM(Config(vocab_size=29,block_size=16,n_layer=2,n_head=4,d_model=32)).eval()
        self.tokens=torch.tensor([[1,4,5,6,8,9,7,3],[1,7,8,6,9,4,5,3]],dtype=torch.int32)
        self.position=torch.zeros((2,8),dtype=torch.float16);self.position[0,3]=1;self.position[1,1]=1

    def test_float64_hidden_matches_original_three_dimensional_body(self):
        lm=self.lm.double()
        with torch.no_grad():
            actual=ANEHiddenBody(lm,shape=(2,8))(self.tokens,self.position)
            expected=HiddenBody(lm,shape=(2,8))(self.tokens,self.position)
        torch.testing.assert_close(actual,expected,atol=1e-12,rtol=1e-10)

    def test_untouched_parameters_and_independent_unpadded_language_model(self):
        before={k:v.clone() for k,v in self.lm.state_dict().items()}
        body=ANEHiddenBody(self.lm,shape=(2,8)).eval()
        tail=FP32Tail(self.lm.ln_f.weight.detach().numpy(),self.lm.ln_f.bias.detach().numpy(),self.lm.head.weight.detach().numpy())
        with torch.no_grad():
            got=tail(body(self.tokens,self.position).numpy())
            expected=np.vstack([F.log_softmax(self.lm(self.tokens[i:i+1,:n].long())[0,-1],-1).numpy() for i,n in enumerate([4,2])])
        np.testing.assert_allclose(got,expected,atol=1e-6)
        for k,v in self.lm.state_dict().items():torch.testing.assert_close(v,before[k],rtol=0,atol=0)
        self.assertIs(self.lm.head.weight,self.lm.tok.weight)

    def test_future_tokens_do_not_change_selected_prefix(self):
        body=ANEHiddenBody(self.lm,shape=(2,8)).eval()
        changed=self.tokens.clone();changed[0,4:]=3;changed[1,2:]=4
        with torch.no_grad():torch.testing.assert_close(body(self.tokens,self.position),body(changed,self.position),rtol=0,atol=0)

    def test_trace_is_input_and_position_dependent(self):
        body=ANEHiddenBody(self.lm,shape=(2,8)).eval()
        with torch.no_grad():
            traced=torch.jit.trace(body,(self.tokens,self.position))
            changed=self.tokens.clone();changed[0,1]=10;changed[1,1]=11
            pos=self.position.clone();pos[0].zero_();pos[0,5]=1
            torch.testing.assert_close(traced(changed,pos),HiddenBody(self.lm,shape=(2,8))(changed,pos),rtol=1e-5,atol=1e-6)


if __name__=='__main__':unittest.main()
