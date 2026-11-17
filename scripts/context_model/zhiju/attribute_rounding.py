"""Attribute saved-body errors with immutable weight-rounding CPU simulations."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import time

import numpy as np
import torch

from export_split import HiddenBody,FP32Tail,load,statistics
from rounding_weights import rounded_copy


def main():
    import coremltools as ct
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ['checkpoint','protocol','existing-models','known-errors','out']:
        parser.add_argument('--'+name,type=Path,required=True)
    args=parser.parse_args()
    if args.out.exists():raise ValueError('refusing to overwrite attribution output')
    torch.set_num_threads(2);started=time.monotonic()
    lm,vocab,_=load(args.checkpoint);protocol=json.loads(args.protocol.read_text())
    known=json.loads(args.known_errors.read_text())
    tail=FP32Tail(lm.ln_f.weight.detach().numpy(),lm.ln_f.bias.detach().numpy(),lm.head.weight.detach().numpy())
    models={'manual-f32':lm};rounding={}
    for group in ['embeddings','normalization','linear','all-body']:
        module,metadata=rounded_copy(lm,group);models['weights-'+group]=module.eval();rounding[group]=metadata
    models['half-cpu']=copy.deepcopy(lm).half().eval()
    report={'scope':'selected diagnostic probes only; not independent admission; error contributions are not additive',
        'checkpointSHA256':hashlib.sha256(args.checkpoint.read_bytes()).hexdigest(),
        'sourceSHA256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'protocolSHA256':hashlib.sha256(args.protocol.read_bytes()).hexdigest(),
        'roundingHelperSHA256':hashlib.sha256(Path(__file__).with_name('rounding_weights.py').read_bytes()).hexdigest(),
        'newExports':0,'newTrainingSteps':0,'roundingStats':rounding,'variants':{},
        'halfCPUScope':'different backend simulation; not an exact emulation of ANE operation rounding'}
    rng=np.random.default_rng(protocol['seed'])
    for width,name in [(16,'zhiju30m_v3_dist_t16'),(40,'zhiju30m_v3_dist')]:
        failed={row['row'] for row in known['variants'][name]['overTV']};selected=[]
        for batch in range(protocol['rowsPerShape']//8):
            lengths=rng.integers(1,width+1,size=8)
            for row,length in enumerate(lengths):
                ids=np.r_[1,rng.integers(3,len(vocab),size=length-1)].astype(np.int32)
                index=batch*8+row
                if index<32 or index in failed:selected.append({'index':index,'length':int(length),'tokens':ids})
        refs=[]
        with torch.inference_mode():
            for case in selected:
                ids=torch.from_numpy(case['tokens'][None,:]).long()
                refs.append(torch.log_softmax(lm(ids)[0,-1],-1).numpy())
        reference=np.asarray(refs);arms={}
        runtime=ct.models.CompiledMLModel(str(args.existing_models/(name+'.mlmodelc')),compute_units=ct.ComputeUnit.ALL)
        predictors={key:HiddenBody(module,shape=(8,width)).eval() for key,module in models.items()}
        predictors['half-cpu'].half()
        for label in list(predictors)+['existing-coreml']:
            got=[]
            try:
                with torch.inference_mode():
                    for start in range(0,len(selected),8):
                        cases=selected[start:start+8];x=np.zeros((8,width),np.int32);p=np.zeros((8,width),np.float16)
                        for row,case in enumerate(cases):
                            x[row,:case['length']]=case['tokens'];p[row,case['length']-1]=1
                        if label=='existing-coreml':hidden=runtime.predict({'tokens':x,'position':p})['hidden']
                        else:hidden=predictors[label](torch.from_numpy(x),torch.from_numpy(p)).float().numpy()
                        got.extend(tail(hidden)[:len(cases)])
                got=np.asarray(got);metrics=statistics(got,reference)
                rows=[]
                for i,case in enumerate(selected):
                    one=statistics(got[i:i+1],reference[i:i+1])
                    rows.append({'row':case['index'],'length':case['length'],'tv':one['maxTotalVariation']})
                metrics['knownFailedRows']=[row for row in rows if row['row'] in failed]
                arms[label]=metrics
            except RuntimeError as error:
                if label!='half-cpu':raise
                arms[label]={'unsupported':str(error)}
            print(name,label,json.dumps(arms[label]),flush=True)
        assert arms['manual-f32']['maxTotalVariation']<1e-4,'manual control differs materially from original Torch model'
        report['variants'][name]={'diagnosticRows':len(selected),'selectedIndices':[c['index'] for c in selected],
            'selection':'first32 original rows plus known failures','arms':arms}
    report['elapsedSeconds']=time.monotonic()-started
    report['checkpointUnchanged']=hashlib.sha256(args.checkpoint.read_bytes()).hexdigest()==report['checkpointSHA256']
    assert report['checkpointUnchanged']
    with args.out.open('x') as output:output.write(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':main()
