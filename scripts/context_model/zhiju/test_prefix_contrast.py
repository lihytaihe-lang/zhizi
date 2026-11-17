"""Test prefix competition against independent per-prefix model forwards."""
import math
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
import torch
from torch.nn import functional as F
from prefix_contrast import prefix_pairwise_loss
from train_zhiju import objective,evaluate
from model import Config,Vocab,ZhiziLM,save,load


class PrefixContrastTests(unittest.TestCase):
    def setUp(self):
        torch.set_num_threads(1);torch.manual_seed(47)

    def test_early_wrong_prefix_is_penalized_even_when_final_sentence_wins(self):
        cumulative=torch.tensor([[-1.,-5.,-6.],[-1.,-2.,-7.]],requires_grad=True)
        loss,count,correct=prefix_pairwise_loss(cumulative,[(0,1,1,3)],1.)
        expected=(math.log1p(math.exp(4))+math.log1p(math.exp(0)))/2
        self.assertAlmostEqual(float(loss.detach()),expected,places=6)
        self.assertEqual(count,2);self.assertEqual(int(correct),1)
        loss.backward()
        self.assertEqual(float(cumulative.grad[0,0]),0)
        self.assertLess(float(cumulative.grad[0,1]),0)
        self.assertGreater(float(cumulative.grad[1,1]),0)

    def test_all_spans_are_weighted_by_competing_positions(self):
        x=torch.tensor([[-1.,-2.,-3.],[-3.,-4.,-5.],[-2.,-1.,-4.]],dtype=torch.float64,requires_grad=True)
        got,count,correct=prefix_pairwise_loss(x,[(0,1,0,1),(0,2,1,3)],.5)
        expected=sum(math.log1p(math.exp(v)) for v in [-1.5,1.5,-.5])/3
        self.assertAlmostEqual(float(got.detach()),expected,places=12)
        self.assertEqual(count,3);self.assertEqual(int(correct),2)
        self.assertTrue(torch.autograd.gradcheck(lambda v:prefix_pairwise_loss(v,[(0,1,0,1),(0,2,1,3)],.5)[0],(x,)))

    def test_empty_spans_preserve_a_zero_gradient(self):
        x=torch.randn(2,3,requires_grad=True)
        loss,count,correct=prefix_pairwise_loss(x,[],1.)
        loss.backward();self.assertEqual(count,0);self.assertEqual(int(correct),0)
        torch.testing.assert_close(x.grad,torch.zeros_like(x))

    def test_invalid_margin_and_spans_are_rejected(self):
        x=torch.zeros(2,3)
        for margin in [True,-1,float('nan'),float('inf')]:
            with self.subTest(margin=margin),self.assertRaises((ValueError,TypeError)):prefix_pairwise_loss(x,[],margin)
        for spans in [[(0,2,0,2)],[(0,1,1,1)],[(0,1,-1,2)],[(0,1,0,4)],[(True,1,0,1)],[[0,1,0,1]]]:
            with self.subTest(spans=spans),self.assertRaises((ValueError,TypeError)):prefix_pairwise_loss(x,spans,1.)

    def fixture(self):
        vocab=Vocab(itos=['<pad>','<bos>','<unk>',*'abcdef'])
        model=ZhiziLM(Config(len(vocab),block_size=40,n_layer=1,n_head=2,d_model=8)).eval()
        return model,vocab

    def prefix_scores(self,model,vocab,context,target):
        total=0.;values=[]
        for i,ch in enumerate(target):
            ids=torch.tensor([[1]+vocab.encode(context+target[:i])])
            total=total+F.log_softmax(model(ids)[0,-1],0)[vocab.stoi[ch]]
            values.append(total)
        return torch.stack(values)

    def test_objective_excludes_context_and_common_prefix(self):
        model,vocab=self.fixture()
        row={'context':'ab','target':'cde','negatives':['cfe']}
        old,_=objective(model,[row],vocab,'cpu',.25,1.,0.)
        p=self.prefix_scores(model,vocab,'ab','cde');n=self.prefix_scores(model,vocab,'ab','cfe')
        expected=old+.4*F.softplus(n[1:]-p[1:]+1).mean()
        got,metrics=objective(model,[row],vocab,'cpu',.25,1.,.4)
        torch.testing.assert_close(got,expected,atol=1e-6,rtol=0)
        self.assertEqual(metrics['prefix_terms'],2)
        got.backward();self.assertTrue(torch.isfinite(model.tok.weight.grad).all())

    def test_validation_aggregation_is_independent_of_batch_partition(self):
        model,vocab=self.fixture()
        rows=[{'context':'a','target':'bcd','negatives':['bed']},{'context':'','target':'abcd'},
              {'context':'ab','target':'cde','negatives':['fda','cfe']}]
        a=evaluate(model,rows,vocab,'cpu',1,.25,1.,.25)
        b=evaluate(model,rows,vocab,'cpu',3,.25,1.,.25)
        for key in ['loss','continuation_loss','contrast_loss','prefix_loss']:self.assertAlmostEqual(a[key],b[key],places=5)
        self.assertEqual(a['prefix_terms'],b['prefix_terms']);self.assertEqual(a['prefix_correct'],b['prefix_correct'])

    def test_unequal_length_pair_remains_legacy_only(self):
        model,vocab=self.fixture();rows=[{'context':'a','target':'bc','negatives':['d']}]
        self.assertTrue(torch.isfinite(objective(model,rows,vocab,'cpu')[0]))
        with self.assertRaises(ValueError):objective(model,rows,vocab,'cpu',prefix_weight=.25)

    def test_cli_records_prefix_recipe_and_updates_a_new_checkpoint(self):
        model,vocab=self.fixture()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);vpath=root/'vocab.json';vpath.write_text(json.dumps(vocab.itos))
            initial=root/'initial.pt';save(initial,model,str(vpath),0)
            data=root/'data.jsonl';data.write_text(json.dumps({'context':'ab','target':'cde','negatives':['cfe']})+'\n')
            command=[sys.executable,str(Path(__file__).with_name('train_zhiju.py')),'--init',str(initial),
                '--train',str(data),'--valid',str(data),'--out',str(root/'out'),'--device','cpu',
                '--steps','2','--eval-every','1','--batch','2','--hard-batch','0','--lr','.001','--prefix-weight','.25']
            result=subprocess.run(command,capture_output=True,text=True,timeout=30)
            self.assertEqual(result.returncode,0,result.stderr)
            run=json.loads((root/'out/run.json').read_text());self.assertTrue(run['completed'])
            self.assertEqual(run['args']['prefix_weight'],.25)
            updated,_,checkpoint=load(root/'out/best.pt')
            self.assertEqual(checkpoint['valid']['prefix_terms'],2)
            self.assertFalse(torch.equal(updated.tok.weight,model.tok.weight))


if __name__=='__main__':unittest.main()
