"""Independent numeric checks use artificial tokens, never the product corpus."""
import hashlib
import json
import math
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
import torch
import torch.nn.functional as F
sys.path.insert(0,str(Path(__file__).resolve().parent))
from train_zhiju import encode_sequence, sequence_statistics, objective, evaluate
from model import Config,Vocab,ZhiziLM,load,save

class TrainingTests(unittest.TestCase):
    def setUp(self):
        torch.set_num_threads(1);torch.manual_seed(5)
        self.v=Vocab(itos=['<pad>','<bos>','<unk>',*'abcdef'])
        self.m=ZhiziLM(Config(len(self.v),block_size=40,n_layer=1,n_head=2,d_model=8)).eval()

    def reference(self,context,target):
        # Independent one-prefix forward for every target character, like runtime generation.
        values=[]
        for i,ch in enumerate(target):
            ids=[1]+self.v.encode(context+target[:i])
            logits=self.m(torch.tensor([ids]))[0,-1]
            values.append(F.log_softmax(logits,0)[self.v.stoi[ch]])
        return torch.stack(values).sum()

    def test_mask_and_each_prefix_reference(self):
        ids,mask=encode_sequence(self.v,'ab','cde')
        self.assertEqual(ids,[1,3,4,5,6,7]);self.assertEqual(mask,[False,False,True,True,True])
        got,counts=sequence_statistics(self.m,[(ids,mask)],'cpu')
        torch.testing.assert_close(got[0],self.reference('ab','cde'),atol=1e-6,rtol=0)
        self.assertEqual(counts.tolist(),[3])

    def test_padding_and_batching_do_not_change_score(self):
        seqs=[encode_sequence(self.v,'a','b'),encode_sequence(self.v,'abc','def')]
        batched,_=sequence_statistics(self.m,seqs,'cpu')
        for i,seq in enumerate(seqs):
            single,_=sequence_statistics(self.m,[seq],'cpu')
            torch.testing.assert_close(batched[i],single[0],atol=1e-6,rtol=0)

    def test_context_is_not_in_loss_and_score_is_sum(self):
        row={'context':'abc','target':'de'}
        loss,metrics=objective(self.m,[row],self.v,'cpu',0,1)
        score=self.reference('abc','de')
        torch.testing.assert_close(loss,-score/2)
        self.assertEqual(metrics['tokens'],2);self.assertEqual(metrics['pairs'],0)
        self.assertEqual(metrics['contrast_loss'],0)
        loss.backward();self.assertGreater(float(self.m.tok.weight.grad.abs().sum()),0)

    def test_margin_pairs_use_correct_positives_and_summed_scores(self):
        rows=[{'context':'a','target':'bc','negatives':['de','f']},{'context':'bc','target':'de','negatives':['ab']}]
        positive=[self.reference(r['context'],r['target']) for r in rows]
        differences=[self.reference(r['context'],n)-p+1.5 for r,p in zip(rows,positive) for n in r['negatives']]
        expected=-torch.stack(positive).sum()/4+.4*F.softplus(torch.stack(differences)).mean()
        actual,m=objective(self.m,rows,self.v,'cpu',.4,1.5)
        torch.testing.assert_close(actual,expected,atol=1e-6,rtol=0)
        self.assertEqual(m['pairs'],3)

    def test_validation_weighted_by_tokens_and_pairs(self):
        rows=[{'context':'a','target':'bc','negatives':['de']},{'context':'','target':'abcde'},{'context':'ab','target':'c','negatives':['f']}]
        a=evaluate(self.m,rows,self.v,'cpu',1,.25,1)
        b=evaluate(self.m,rows,self.v,'cpu',3,.25,1)
        for k in ['loss','continuation_loss','contrast_loss']:self.assertAlmostEqual(a[k],b[k],places=5)
        self.assertFalse(self.m.training)

    def test_invalid_sequence_or_negative_rejected(self):
        for context,target in [('a','?'),('',''),('a'*39,'b'),(None,'a'),('a',7)]:
            with self.subTest(context=context,target=target),self.assertRaises(ValueError):encode_sequence(self.v,context,target)
        for negative in [['de','de'],['bc'],[7],['d','e','f'],'de']:
            with self.subTest(negative=negative),self.assertRaises(ValueError):objective(self.m,[{'context':'a','target':'bc','negatives':negative}],self.v,'cpu')

    def test_empty_context_and_longest_supported_prefix(self):
        loss,_=objective(self.m,[{'context':'','target':'abc'},{'context':'a'*38,'target':'b'}],self.v,'cpu')
        self.assertTrue(math.isfinite(float(loss.detach())))

    def test_cli_changes_weights_binds_inputs_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            p=Path(directory);vocab=p/'vocab.json';vocab.write_text(json.dumps(self.v.itos))
            init=p/'init.pt';save(init,self.m,str(vocab),0)
            data=p/'data.jsonl';data.write_text(json.dumps({'context':'a','target':'bc','negatives':['de']})+'\n')
            before=hashlib.sha256(init.read_bytes()).hexdigest()
            command=[sys.executable,str(Path(__file__).with_name('train_zhiju.py')),'--init',str(init),'--train',str(data),'--valid',str(data),'--out',str(p/'out'),'--device','cpu','--steps','2','--eval-every','1','--batch','2','--hard-batch','0','--lr','.001']
            result=subprocess.run(command,capture_output=True,text=True,timeout=30)
            self.assertEqual(result.returncode,0,result.stderr)
            run=json.loads((p/'out/run.json').read_text());self.assertTrue(run['completed']);self.assertIn(run['bestStep'],[1,2])
            self.assertEqual(run['inputs']['init']['sha256'],before);self.assertEqual(hashlib.sha256(init.read_bytes()).hexdigest(),before)
            trained,_,ck=load(p/'out/best.pt');self.assertTrue(ck['zhijuTraining']);self.assertFalse(torch.equal(trained.tok.weight,self.m.tok.weight))
            again=subprocess.run(command,capture_output=True,text=True,timeout=30);self.assertNotEqual(again.returncode,0)

if __name__=='__main__':unittest.main()
