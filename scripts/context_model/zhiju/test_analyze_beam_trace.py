"""Independent counterexamples for diagnostic misclassification and bad evidence."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

TOOL=Path(__file__).with_name('analyze_beam_trace.py')

def row():
    return {'id':'a','expected':'你好','beam':8,'diagnosticBudgetMS':5000,
      'candidates':['你好'],'searchMs':35.,'modelMs':12.,'callbacks':[
        {'step':0,'prefixes':[''],'targetPrefixAlive':True},
        {'step':1,'prefixes':['你'],'targetPrefixAlive':True}]}

class TraceTests(unittest.TestCase):
    def run_case(self,before,after,preexisting=None):
        with tempfile.TemporaryDirectory() as folder:
            d=Path(folder)
            for name,data in [('before',before),('after',after)]:
                (d/(name+'.json')).write_text(json.dumps({'rows':data},ensure_ascii=False))
            out=d/'out.json'
            if preexisting is not None:out.write_text(preexisting)
            r=subprocess.run([sys.executable,str(TOOL),'--before',str(d/'before.json'),'--after',str(d/'after.json'),'--out',str(out)],capture_output=True,text=True,timeout=5)
            return r.returncode,out.read_text() if out.exists() else None,r.stderr

    def test_known_beam_pruning_and_unicode_length(self):
        b=row();a=copy.deepcopy(b);a['candidates']=[]
        a['callbacks'][1]={'step':1,'prefixes':['他'],'targetPrefixAlive':False}
        code,raw,_=self.run_case([b],[a]);self.assertEqual(code,0)
        r=json.loads(raw);self.assertEqual(r['regressions'],['a'])
        self.assertEqual(r['rows'][0]['firstPrunedStepAfter'],1)

    def test_completed_length_disappearance_is_not_early_pruning(self):
        b=row();a=copy.deepcopy(b);a['candidates']=[]
        a['callbacks'].append({'step':2,'prefixes':['其他'],'targetPrefixAlive':False})
        code,raw,_=self.run_case([b],[a]);self.assertEqual(code,0)
        r=json.loads(raw);self.assertEqual(r['rows'][0]['classification'],'search_miss')
        self.assertIsNone(r['rows'][0]['firstPrunedStepAfter'])
        self.assertEqual(r['regressions'],['a'])

    def test_negative_times_rejected_without_output(self):
        for field in ['searchMs','modelMs']:
            with self.subTest(field=field):
                a=row();a[field]=-1
                code,raw,_=self.run_case([row()],[a]);self.assertNotEqual(code,0);self.assertIsNone(raw)

    def test_out_of_order_callbacks_rejected(self):
        a=row();a['callbacks'].reverse()
        code,raw,_=self.run_case([row()],[a]);self.assertNotEqual(code,0);self.assertIsNone(raw)

    def test_empty_active_callback_rejected(self):
        a=row();a['callbacks'][1]={'step':1,'prefixes':[],'targetPrefixAlive':False}
        code,raw,_=self.run_case([row()],[a]);self.assertNotEqual(code,0);self.assertIsNone(raw)

    def test_forged_alive_and_invalid_timings_rejected(self):
        a=row();a['callbacks'][1]['targetPrefixAlive']=False
        code,raw,_=self.run_case([row()],[a]);self.assertNotEqual(code,0);self.assertIsNone(raw)
        for value in [float('nan'),float('inf'),True,'12']:
            a=row();a['modelMs']=value
            code,raw,_=self.run_case([row()],[a]);self.assertNotEqual(code,0);self.assertIsNone(raw)

    def test_matching_identity_and_output_retention(self):
        for mutate in [lambda a:a.update(id='b'),lambda a:a.update(expected='你们')]:
            a=row();mutate(a)
            code,raw,_=self.run_case([row()],[a]);self.assertNotEqual(code,0);self.assertIsNone(raw)
        code,raw,_=self.run_case([row()],[row()],preexisting='keep')
        self.assertNotEqual(code,0);self.assertEqual(raw,'keep')

    def test_all_comparison_classes_and_input_order(self):
        before=[];after=[]
        for name in ['retained','fixed','both_miss']:
            b=row();b['id']=name;a=copy.deepcopy(b)
            if name in ['fixed','both_miss']:b['candidates']=[]
            if name=='both_miss':a['candidates']=[]
            before.append(b);after.append(a)
        code,raw,_=self.run_case(before,list(reversed(after)));self.assertEqual(code,0)
        r=json.loads(raw);self.assertEqual([x['classification'] for x in r['rows']],['retained','fixed','both_miss'])
        self.assertEqual(r['fixes'],['fixed']);self.assertIn('diagnostic',r['scope'])

if __name__=='__main__':unittest.main()
