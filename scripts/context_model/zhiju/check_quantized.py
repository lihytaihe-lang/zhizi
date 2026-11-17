"""Measure probability drift of existing FP16 graphs; never exports or trains."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import sys
import numpy as np
import torch
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'zhizi'))
from model import load


def statistics(got,reference):
    if not np.isfinite(got).all():return {'passed':False,'reason':'nonfinite output'}
    mass=np.exp(got.astype(np.float64));normalization=mass.sum(-1)
    p=mass/normalization[:,None];truth=np.exp(reference.astype(np.float64));truth/=truth.sum(-1,keepdims=True)
    tv=.5*np.abs(p-truth).sum(-1)
    a=got.argmax(-1);b=reference.argmax(-1);different=a!=b
    gap=reference[np.arange(len(a)),b]-reference[np.arange(len(a)),a]
    return {'rows':len(got),'maxTotalVariation':float(tv.max()),'meanTotalVariation':float(tv.mean()),
        'maxNormalizationError':float(np.abs(normalization-1).max()),
        'top1Agreement':float((a==b).mean()),
        'maxChangedTop1LogOddsGap':float(gap[different].max()) if different.any() else 0.,
        'maxAbsoluteLogProbabilityError':float(np.abs(got-reference).max())}


def accepts(metrics,protocol):
    return (metrics.get('reason') is None
        and metrics['maxTotalVariation']<=protocol['maxTotalVariation']
        and metrics['maxNormalizationError']<=protocol['maxNormalizationError']
        and metrics['maxChangedTop1LogOddsGap']<=protocol['maxChangedTop1LogOddsGap'])


def main():
    import coremltools as ct
    ap=argparse.ArgumentParser()
    for name in ['initial','checkpoint','initial-models','trained-models','protocol','out']:
        ap.add_argument('--'+name,type=Path,required=True)
    a=ap.parse_args();protocol=json.loads(a.protocol.read_text());torch.set_num_threads(2)
    sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
    report={'protocolSHA256':sha(a.protocol),'sourceSHA256':sha(__file__),'exportsCreated':0,'trainingStepsRun':0,
        'initialCheckpointSHA256':sha(a.initial),'trainedCheckpointSHA256':sha(a.checkpoint),'arms':{},'passed':True}
    for arm,checkpoint,models in [('initial',a.initial,a.initial_models),('trained',a.checkpoint,a.trained_models)]:
        lm,vocab,_=load(checkpoint);rng=np.random.default_rng(protocol['seed']);variants={}
        for width,name in [(16,'zhiju30m_v3_dist_t16'),(40,'zhiju30m_v3_dist')]:
            runtime=ct.models.CompiledMLModel(str(models/(name+'.mlmodelc')),compute_units=ct.ComputeUnit.ALL)
            got=[];ref=[]
            for _ in range(protocol['rowsPerShape']//8):
                lengths=rng.integers(1,width+1,size=8);x=np.zeros((8,width),np.int32);p=np.zeros((8,width),np.float16)
                for i,n in enumerate(lengths):
                    x[i,:n]=np.r_[1,rng.integers(3,len(vocab),size=n-1)];p[i,n-1]=1
                    with torch.no_grad():ref.append(torch.log_softmax(lm(torch.from_numpy(x[i:i+1,:n]).long())[0,-1],-1).numpy())
                got.extend(runtime.predict({'tokens':x,'position':p})['logprobs'])
            metrics=statistics(np.asarray(got),np.asarray(ref));metrics['passed']=accepts(metrics,protocol)
            report['passed'] &= metrics['passed'];variants[name]=metrics
        report['arms'][arm]=variants
    a.out.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report));return 0 if report['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
