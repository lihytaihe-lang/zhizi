"""Check attribution isolation, tied parameters and failure safety."""
import unittest
import sys
from pathlib import Path
import torch
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'zhizi'))

from model import Config,ZhiziLM
from rounding_weights import rounded_copy


class RoundingTests(unittest.TestCase):
    def setUp(self):
        torch.manual_seed(19)
        self.model=ZhiziLM(Config(vocab_size=17,block_size=16,n_layer=1,n_head=2,d_model=8))

    def test_source_unchanged_and_copy_preserves_tied_head(self):
        before={k:v.clone() for k,v in self.model.state_dict().items()}
        copied,report=rounded_copy(self.model,'all-body')
        self.assertIs(copied.head.weight,copied.tok.weight)
        self.assertIsNot(copied.tok.weight,self.model.tok.weight)
        self.assertEqual(copied.training,self.model.training)
        for name,value in self.model.state_dict().items():torch.testing.assert_close(value,before[name],rtol=0,atol=0)
        self.assertNotIn('head.weight',report['selectedNames'])
        self.assertNotIn('ln_f.weight',report['selectedNames'])
        self.assertEqual(report['selectedTensorCount'],14)

    def test_group_boundaries_and_roundtrip_values(self):
        for group,expected in [('embeddings',2),('normalization',4),('linear',8)]:
            copied,report=rounded_copy(self.model,group)
            self.assertEqual(report['selectedTensorCount'],expected)
            original=dict(self.model.named_parameters())
            for name,param in copied.named_parameters():
                value=original[name]
                expected_value=value.half().float() if name in report['selectedNames'] else value
                torch.testing.assert_close(param,expected_value,rtol=0,atol=0)
            torch.testing.assert_close(copied.ln_f.weight,self.model.ln_f.weight,rtol=0,atol=0)

    def test_underflow_and_delta_are_measured_before_parameter_copy(self):
        with torch.no_grad():self.model.blocks[0].ln1.bias.fill_(1e-9)
        _,report=rounded_copy(self.model,'normalization')
        self.assertEqual(report['underflowToZeroCount'],8)
        self.assertGreater(report['maxAbsoluteDelta'],0)

    def test_overflow_or_nonfinite_rejected_without_modifying_source(self):
        for value in [1e6,float('nan')]:
            with torch.no_grad():self.model.blocks[0].ln1.bias.fill_(value)
            before=self.model.blocks[0].ln1.bias.clone()
            with self.subTest(value=value),self.assertRaises(ValueError):rounded_copy(self.model,'normalization')
            torch.testing.assert_close(self.model.blocks[0].ln1.bias,before,equal_nan=True,rtol=0,atol=0)

    def test_invalid_group_and_selected_dtype_rejected(self):
        with self.assertRaises(ValueError):rounded_copy(self.model,'head')
        self.model.double()
        with self.assertRaises(ValueError):rounded_copy(self.model,'linear')


if __name__=='__main__':unittest.main()
