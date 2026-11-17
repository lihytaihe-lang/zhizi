"""Export fixed Zhiju weights with explicit mixed-precision policies.

This changes representation only, never the trained checkpoint or search policy.
The previously frozen probability criteria and real-controller gates still apply.
"""
import argparse
import json
from pathlib import Path
import sys
import numpy as np
import torch
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'zhizi'))
from model import load
from export_dist import PositionDistribution,compile_mlpackage
from export_finetuned import sha
from check_quantized import statistics,accepts


class TailPrecision:
    """Keep ln_f and every dependent op in FP32, using graph data dependencies.

    Scope metadata identifies the actual final norm, not a generated op number.
    Unrelated ops of the same type in the transformer remain eligible for FP16.
    """
    MODES = {'tail', 'stable', 'norm-residual', 'norm'}

    def __init__(self,stable_stream=False,stream_mode=None):
        if stable_stream and stream_mode is not None:
            raise ValueError('choose stable_stream or stream_mode, not both')
        self.stream_mode = 'stable' if stable_stream else (stream_mode or 'tail')
        if self.stream_mode not in self.MODES:
            raise ValueError('unknown precision policy: '+self.stream_mode)
        self.precision_tag = {'tail':'fp16-tail32', 'stable':'fp16-stable-stream'}.get(
            self.stream_mode, 'fp16-'+self.stream_mode)
        self.outputs=set()
        self.kept=[]
        self.found_norm=False
        self.found_head=False

    def __call__(self,op):
        scopes=[v for key,values in op.scopes.items() if str(key).endswith('TORCHSCRIPT_MODULE_NAME') for v in values]
        inputs=[var for value in op.inputs.values() for var in (value if isinstance(value,(list,tuple)) else [value])]
        final_norm=op.op_type=='layer_norm' and 'ln_f' in scopes
        tail=final_norm or any(var.name in self.outputs for var in inputs)
        if tail:
            self.outputs.update(var.name for var in op.outputs)
        # Residual/embedding sums have [batch, time, hidden] outputs. Attention
        # mask sums have rank four and must not acquire FP32 just because they
        # share the add operator. Keep the two bounded attribution policies
        # separate from the previously measured stable policy.
        residual_add=(op.op_type=='add' and any(len(var.shape)==3 for var in op.outputs))
        stream_keep=(self.stream_mode=='stable' and op.op_type in {'layer_norm','add','softmax','gelu'}
            or self.stream_mode in {'norm-residual','norm'} and op.op_type=='layer_norm'
            or self.stream_mode=='norm-residual' and residual_add)
        keep=tail or stream_keep
        if keep:
            self.kept.append({'name':op.name,'type':op.op_type,'scopes':scopes,
                'reason':'output-tail' if tail else 'stable-stream' if self.stream_mode=='stable'
                    else 'normalization' if op.op_type=='layer_norm' else 'residual-sum'})
            self.found_norm |= final_norm
            self.found_head |= op.op_type=='linear' and 'head' in scopes
        return not keep


def main():
    import coremltools as ct
    ap=argparse.ArgumentParser(description=__doc__)
    for key in ['checkpoint','initial','out','protocol']:ap.add_argument('--'+key,type=Path,required=True)
    ap.add_argument('--role',choices=['trained','general'],default='trained')
    precision=ap.add_mutually_exclusive_group()
    precision.add_argument('--stable-stream',action='store_true',help='also preserve normalization, residual sums, attention softmax and GELU in FP32; GEMMs remain FP16')
    precision.add_argument('--stream-mode',choices=sorted(TailPrecision.MODES),help='bounded attribution of normalization/residual precision; same weights and frozen checks')
    ap.add_argument('--additional-widths',type=int,nargs='*',default=[],choices=[24,32])
    ap.add_argument('--additional-probe-seed',type=int,default=20261122)
    a=ap.parse_args()
    if len(a.additional_widths)!=len(set(a.additional_widths)):raise ValueError('duplicate additional window')
    if a.out.exists() and any(a.out.iterdir()):raise ValueError('refusing to overwrite prior export')
    a.out.mkdir(parents=True,exist_ok=True);torch.set_num_threads(2)
    lm,vocab,ck=load(a.checkpoint);initial=torch.load(a.initial,map_location='cpu')
    assert initial['itos']==vocab.itos
    changed=sum(not torch.equal(v,initial['state'][k]) for k,v in ck['state'].items())
    if a.role=='trained':
        assert ck.get('zhijuTraining') is True and ck['step']>0
        assert ck['sourceCheckpointSHA256']==sha(a.initial) and changed>0
    else:
        assert sha(a.checkpoint)==sha(a.initial) and changed==0,'general role must preserve original parameters'
    protocol=json.loads(a.protocol.read_text());rng=np.random.default_rng(protocol['seed'])
    (a.out/'itos.json').write_text(json.dumps(vocab.itos,ensure_ascii=False))
    report={'checkpointSHA256':sha(a.checkpoint),'initialSHA256':sha(a.initial),'trainedStep':ck['step'],
      'sourceSHA256':sha(__file__),'protocolSHA256':sha(a.protocol),
      'representation':'FP16 transformer operations except explicitly selected FP32 stream operations and output tail',
      'precisionPolicy':'stable' if a.stable_stream else (a.stream_mode or 'tail'),
      'weightsChangedByExport':False,'stateTensorsChangedByTraining':changed,'variants':{},'passed':True}
    report['modelRole']=a.role
    runtimes={}
    jobs=[(16,'zhiju30m_v3_dist_t16'),(40,'zhiju30m_v3_dist')]
    jobs += [(width,f'zhiju30m_v3_dist_t{width}') for width in sorted(a.additional_widths)]
    report['additionalWindows']={'widths':sorted(a.additional_widths),'seed':a.additional_probe_seed}
    for width,name in jobs:
        # Additional windows do not consume the frozen 16/40 probe stream.
        window_rng=rng if width in [16,40] else np.random.default_rng(a.additional_probe_seed+width)
        tokens=torch.ones((8,width),dtype=torch.int32);position=torch.zeros((8,width),dtype=torch.float16);position[:,0]=1
        with torch.no_grad():traced=torch.jit.trace(PositionDistribution(lm,shape=(8,width)).eval(),(tokens,position))
        select=TailPrecision(stable_stream=a.stable_stream,stream_mode=a.stream_mode)
        model=ct.convert(traced,convert_to='mlprogram',compute_precision=ct.transform.FP16ComputePrecision(op_selector=select),
          minimum_deployment_target=ct.target.macOS13,skip_model_load=True,
          inputs=[ct.TensorType(name='tokens',shape=(8,width),dtype=np.int32),ct.TensorType(name='position',shape=(8,width),dtype=np.float16)],
          outputs=[ct.TensorType(name='logprobs',dtype=np.float32)])
        assert select.found_norm and select.found_head,'required FP32 tail scopes missing; refusing silent all-FP16 export'
        model.user_defined_metadata['zhijuCheckpointSHA256']=report['checkpointSHA256']
        model.user_defined_metadata['zhijuTrainedStep' if a.role=='trained' else 'zhijuSourceStep']=str(ck['step'])
        model.user_defined_metadata['zhijuModelRole']=a.role
        model.user_defined_metadata['zhijuComputePrecision']=select.precision_tag
        package=a.out/(name+'.mlpackage');model.save(str(package));compiled=compile_mlpackage(package,a.out)
        runtime=ct.models.CompiledMLModel(str(compiled),compute_units=ct.ComputeUnit.ALL);runtimes[width]=runtime
        got=[];ref=[];padding=[]
        for _ in range(protocol['rowsPerShape']//8):
            lengths=window_rng.integers(1,width+1,size=8);x=np.zeros((8,width),np.int32);p=np.zeros((8,width),np.float16)
            for row,n in enumerate(lengths):
                x[row,:n]=np.r_[1,window_rng.integers(3,len(vocab),size=n-1)];p[row,n-1]=1
                with torch.no_grad():ref.append(torch.log_softmax(lm(torch.from_numpy(x[row:row+1,:n]).long())[0,-1],-1).numpy())
            got.extend(runtime.predict({'tokens':x,'position':p})['logprobs'])
            padded=x.copy()
            # Independent padding stream must not change the frozen input probe.
            pad_rng=np.random.default_rng(1000+len(got))
            for row,n in enumerate(lengths):padded[row,n:]=pad_rng.integers(3,len(vocab),size=width-n)
            padding.extend(runtime.predict({'tokens':padded,'position':p})['logprobs'])
        metrics=statistics(np.asarray(got),np.asarray(ref));metrics['passed']=accepts(metrics,protocol)
        metrics['randomPaddingMaxAbsoluteError']=float(np.abs(np.asarray(got)-np.asarray(padding)).max())
        metrics['fp32Operations']=select.kept
        metrics['passed'] &= metrics['randomPaddingMaxAbsoluteError']<=1e-5
        report['variants'][name]=metrics;report['passed'] &= metrics['passed']
        (a.out/'PARITY.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps({name:metrics}),flush=True)
    short=[];full=[]
    for _ in range(8):
        x=np.zeros((8,40),np.int32);p=np.zeros((8,40),np.float16)
        for row,n in enumerate(rng.integers(1,17,size=8)):
            x[row,:n]=np.r_[1,rng.integers(3,len(vocab),size=n-1)];p[row,n-1]=1
        short.extend(runtimes[16].predict({'tokens':x[:,:16].copy(),'position':p[:,:16].copy()})['logprobs'])
        full.extend(runtimes[40].predict({'tokens':x,'position':p})['logprobs'])
    metrics=statistics(np.asarray(short),np.asarray(full));metrics['passed']=accepts(metrics,protocol)
    report['shortVsFull']=metrics;report['passed'] &= metrics['passed']
    (a.out/'PARITY.json').write_text(json.dumps(report,indent=2)+'\n')
    (a.out/'SHA256SUMS').write_text(''.join(f'{sha(p)}  {p.relative_to(a.out)}\n' for p in sorted(a.out.rglob('*')) if p.is_file() and ('.mlmodelc' in str(p.relative_to(a.out)) or p.name=='itos.json')))
    print(json.dumps({'passed':report['passed'],'shortVsFull':metrics}),flush=True)
    return 0 if report['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
