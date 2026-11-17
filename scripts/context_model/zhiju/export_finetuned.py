"""Export trained Zhiju weights to the unchanged 8x16/8x40 interface.

FP32 verifies graph mathematics. The initial FP16 absolute-logit criterion is
retained to reproduce its failed experiment; it also rejects the installed
baseline and is not a standalone FP16 release gate. Quantized admission requires
check_quantized.py and the unchanged real-controller quality/time-budget gates.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'zhizi'))
from model import load
from export_dist import PositionDistribution, compile_mlpackage

MAX_ABSOLUTE_ERROR = .125
MIN_TOP1_AGREEMENT = .99
MAX_DISAGREEMENT_MARGIN = .05
SEED = 20261115


def sha(path):
    h=hashlib.sha256()
    with Path(path).open('rb') as f:
        for part in iter(lambda:f.read(1024*1024),b''):h.update(part)
    return h.hexdigest()


def compare(got, reference):
    errors=np.abs(got.astype(np.float64)-reference.astype(np.float64))
    actual=got.argmax(-1);expected=reference.argmax(-1);different=actual!=expected
    margins=reference[np.arange(len(reference)),expected]-reference[np.arange(len(reference)),actual]
    result={'maxAbsoluteError':float(errors.max()),'meanAbsoluteError':float(errors.mean()),
        'top1Agreement':float((~different).mean()),'rows':len(reference),
        'maxDisagreementMargin':float(margins[different].max()) if different.any() else 0.}
    result['passed']=(np.isfinite(got).all().item() and result['maxAbsoluteError']<=MAX_ABSOLUTE_ERROR
        and result['top1Agreement']>=MIN_TOP1_AGREEMENT and result['maxDisagreementMargin']<=MAX_DISAGREEMENT_MARGIN)
    return result


def main():
    import coremltools as ct
    ap=argparse.ArgumentParser(description=__doc__)
    for key in ['checkpoint','initial','out']:
        ap.add_argument('--'+key,type=Path,required=True)
    ap.add_argument('--precision',choices=['fp16','fp32'],default='fp16')
    ap.add_argument('--probability-protocol',type=Path,
        help='use the already frozen probability gate; omitted reproduces the retired absolute-logit experiment')
    args=ap.parse_args()
    if args.out.exists() and any(args.out.iterdir()):raise ValueError('refusing to overwrite prior export')
    args.out.mkdir(parents=True,exist_ok=True);torch.set_num_threads(2)
    lm,vocab,ck=load(args.checkpoint)
    initial=torch.load(args.initial,map_location='cpu')
    assert ck.get('zhijuTraining') is True and ck['step']>0
    assert ck['sourceCheckpointSHA256']==sha(args.initial)
    assert initial['itos']==vocab.itos
    changed=sum(not torch.equal(value,initial['state'][name]) for name,value in ck['state'].items())
    max_change=max(float((value-initial['state'][name]).abs().max()) for name,value in ck['state'].items())
    assert changed>0 and max_change>0,'weights must actually change'
    (args.out/'itos.json').write_text(json.dumps(vocab.itos,ensure_ascii=False))
    protocol={'seed':SEED,'maxAbsoluteError':MAX_ABSOLUTE_ERROR,'minTop1Agreement':MIN_TOP1_AGREEMENT,
        'maxDisagreementMargin':MAX_DISAGREEMENT_MARGIN,'reference':'independent original torch model, unpadded prefix, next-character log_softmax',
        'rowsPerShape':128,'shapes':[[8,16],[8,40]],'precision':args.precision,
        'randomPaddingMaxAbsoluteError':1e-5,'frozenBeforeExport':True}
    probability=json.loads(args.probability_protocol.read_text()) if args.probability_protocol else None
    if probability is not None:
        assert probability['shapes']==[[8,16],[8,40]] and probability['rowsPerShape']%8==0
        protocol.update(probability,probabilityProtocolSHA256=sha(args.probability_protocol),
            legacyAbsoluteLogitGateIsDiagnosticOnly=True)
    def active_compare(got,reference):
        if probability is None:return compare(got,reference)
        from check_quantized import statistics,accepts
        result=statistics(got,reference);result['passed']=accepts(result,probability)
        result['retiredAbsoluteLogitDiagnostic']=compare(got,reference)
        return result
    (args.out/'PARITY-PROTOCOL.json').write_text(json.dumps(protocol,indent=2)+'\n')
    report={'checkpointSHA256':sha(args.checkpoint),'initialSHA256':sha(args.initial),'trainedStep':ck['step'],
        'stateTensorsChanged':changed,'maxTensorChange':max_change,'parameters':lm.n_params(),'protocol':protocol,
        'coremltools':ct.__version__,'torch':torch.__version__,'variants':{},'passed':True}
    rng=np.random.default_rng(protocol['seed'])
    runtimes={}
    for width,name in [(16,'zhiju30m_v3_dist_t16'),(40,'zhiju30m_v3_dist')]:
        tokens=torch.ones((8,width),dtype=torch.int32)
        position=torch.zeros((8,width),dtype=torch.float16);position[:,0]=1
        with torch.no_grad():traced=torch.jit.trace(PositionDistribution(lm,shape=(8,width)).eval(),(tokens,position))
        precision=ct.precision.FLOAT16 if args.precision=='fp16' else ct.precision.FLOAT32
        model=ct.convert(traced,convert_to='mlprogram',compute_precision=precision,
            minimum_deployment_target=ct.target.macOS13,skip_model_load=True,
            inputs=[ct.TensorType(name='tokens',shape=(8,width),dtype=np.int32),ct.TensorType(name='position',shape=(8,width),dtype=np.float16)],
            outputs=[ct.TensorType(name='logprobs',dtype=np.float32)])
        model.user_defined_metadata['zhijuCheckpointSHA256']=report['checkpointSHA256']
        model.user_defined_metadata['zhijuTrainedStep']=str(ck['step'])
        model.user_defined_metadata['zhijuComputePrecision']=args.precision
        package=args.out/(name+'.mlpackage');model.save(str(package));compiled=compile_mlpackage(package,args.out)
        runtime=ct.models.CompiledMLModel(str(compiled),compute_units=ct.ComputeUnit.ALL)
        runtimes[width]=runtime
        got=[];reference=[];random_padding=[]
        for _ in range(protocol['rowsPerShape']//8):
            lengths=rng.integers(1,width+1,size=8)
            x=np.zeros((8,width),np.int32);p=np.zeros((8,width),np.float16)
            for row,n in enumerate(lengths):
                x[row,:n]=np.r_[1,rng.integers(3,len(vocab),size=n-1)];p[row,n-1]=1
                with torch.no_grad():reference.append(torch.log_softmax(lm(torch.from_numpy(x[row:row+1,:n]).long())[0,-1],-1).numpy())
            out=runtime.predict({'tokens':x,'position':p})['logprobs'];got.extend(out)
            padded=x.copy()
            pad_rng=np.random.default_rng(1000+len(got)) if probability is not None else rng
            for row,n in enumerate(lengths):padded[row,n:]=pad_rng.integers(3,len(vocab),size=width-n)
            random_padding.extend(runtime.predict({'tokens':padded,'position':p})['logprobs'])
        result=active_compare(np.asarray(got),np.asarray(reference));result['randomPadding']=compare(np.asarray(random_padding),np.asarray(got))
        result['randomPadding']['passed'] &= result['randomPadding']['maxAbsoluteError']<=1e-5
        result['passed'] &= result['randomPadding']['passed'];report['passed'] &= result['passed'];report['variants'][name]=result
        (args.out/'PARITY.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps({name:result}),flush=True)
    short=[];full=[]
    for _ in range(8):
        x=np.zeros((8,40),np.int32);p=np.zeros((8,40),np.float16)
        for row,n in enumerate(rng.integers(1,17,size=8)):
            x[row,:n]=np.r_[1,rng.integers(3,len(vocab),size=n-1)];p[row,n-1]=1
        short.extend(runtimes[16].predict({'tokens':x[:,:16].copy(),'position':p[:,:16].copy()})['logprobs'])
        full.extend(runtimes[40].predict({'tokens':x,'position':p})['logprobs'])
    report['shortVsFull']=active_compare(np.asarray(short),np.asarray(full));report['passed'] &= report['shortVsFull']['passed']
    (args.out/'PARITY.json').write_text(json.dumps(report,indent=2)+'\n')
    (args.out/'SHA256SUMS').write_text(''.join(f'{sha(p)}  {p.relative_to(args.out)}\n' for p in sorted(args.out.rglob('*')) if p.is_file() and ('.mlmodelc' in str(p.relative_to(args.out)) or p.name=='itos.json')))
    assert report['passed'],'numeric export parity failed; do not promote'


if __name__=='__main__':main()
