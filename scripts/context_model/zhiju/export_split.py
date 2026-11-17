"""Test fixed-weight FP16-body/FP32-tail deployment under the frozen criteria."""
import argparse
import copy
from collections import Counter
import json
from pathlib import Path
import sys
import time

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'zhizi'))
from model import load
from export_dist import PositionDistribution, log_distribution, compile_mlpackage
from export_finetuned import sha
from check_quantized import statistics, accepts
from split_tail import FP32Tail
from stable_norm import stable_layer_norm
from gemm_norm import gemm_layer_norm
from aligned_gemm_norm import aligned_gemm_layer_norm


def block_forward(z, block, mask, heads, dimension, shape=None, normalization='native', mean_weight=None):
    batch,width = z.shape[:2] if shape is None else shape
    def norm(value,layer):
        if normalization == 'aligned-gemm-stats':
            return aligned_gemm_layer_norm(value,layer.weight,layer.bias,layer.eps,dimension,mean_weight)
        if normalization in {'gemm-stats','hierarchical-stats'}:
            return gemm_layer_norm(value,layer.weight,layer.bias,layer.eps,dimension,mean_weight,
                hierarchical=normalization=='hierarchical-stats')
        return stable_layer_norm(value,layer.weight,layer.bias,layer.eps,dimension,normalization)
    q,k,v = block.qkv(norm(z,block.ln1)).split(dimension,dim=2)
    q,k,v = (a.reshape(batch,width,heads,dimension//heads).transpose(1,2) for a in (q,k,v))
    attention = torch.softmax(q@k.transpose(-1,-2)*(1.0/(dimension//heads)**.5)+mask[:width,:width],dim=-1)
    z = z+block.proj((attention@v).transpose(1,2).reshape(batch,width,dimension))
    return z+block.mlp(norm(z,block.ln2))


class HiddenBody(PositionDistribution):
    def __init__(self,lm,shape=None,last_block_in_tail=False,normalization='native'):
        super().__init__(lm,shape=shape)
        self.last_block_in_tail = last_block_in_tail
        self.normalization = normalization
        if normalization in {'gemm-stats','hierarchical-stats'}:
            self.register_buffer('normalization_mean_weight',torch.full((self.d,1),1.0/self.d))
        elif normalization == 'aligned-gemm-stats':
            self.register_buffer('normalization_mean_weight',torch.full((self.d,32),1.0/self.d))

    def forward(self,tokens,position=None):
        width = tokens.shape[1] if self.shape is None else self.shape[1]
        z = self.lm.tok(tokens.long())+self.lm.pos.weight[:width]
        blocks = self.lm.blocks[:-1] if self.last_block_in_tail else self.lm.blocks
        for block in blocks:
            z = block_forward(z,block,self.mask,self.h,self.d,self.shape,self.normalization,
                getattr(self,'normalization_mean_weight',None))
        return z if self.last_block_in_tail else self._selected(position,z)


class LastBlockTail(PositionDistribution):
    def forward(self,hidden,position):
        z = block_forward(hidden,self.lm.blocks[-1],self.mask,self.h,self.d,self.shape)
        selected = self.lm.ln_f(self._selected(position,z))
        return log_distribution(self.lm.head(selected))


def main():
    import coremltools as ct
    from coremltools.models.compute_plan import MLComputePlan
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['checkpoint','initial','out','protocol']:
        parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--structure',choices=['selected-hidden-tail','last-block-tail'],required=True)
    parser.add_argument('--body-layout',choices=['native','ane-channels-first'],default='native')
    parser.add_argument('--body-normalization-precision',choices=['fp16','fp32'],default='fp16')
    parser.add_argument('--normalization',choices=['native','centered-native','centered-explicit','gemm-stats','hierarchical-stats','aligned-gemm-stats'],default='native')
    args=parser.parse_args()
    if args.out.exists() and any(args.out.iterdir()):
        raise ValueError('refusing to overwrite prior export')
    args.out.mkdir(parents=True,exist_ok=True)
    torch.set_num_threads(2)
    lm,vocab,checkpoint=load(args.checkpoint)
    assert checkpoint.get('zhijuTraining') and checkpoint['step']>0
    assert checkpoint['sourceCheckpointSHA256']==sha(args.initial)
    protocol=json.loads(args.protocol.read_text())
    last=args.structure=='last-block-tail'
    if args.body_layout != 'native' and (last or args.normalization != 'native'):
        raise ValueError('channel-first attribution uses selected hidden and the original normalization formula')
    if args.body_normalization_precision != 'fp16' and args.body_layout != 'ane-channels-first':
        raise ValueError('FP32 channel normalization is limited to the channel-first body')
    if last and args.normalization!='native':
        raise ValueError('normalization attribution is limited to the frozen selected-hidden structure')
    gamma=lm.ln_f.weight.detach().numpy().copy()
    beta=lm.ln_f.bias.detach().numpy().copy()
    weight=lm.head.weight.detach().numpy().copy()
    tail=FP32Tail(gamma,beta,weight,epsilon=lm.ln_f.eps)
    (args.out/'itos.json').write_text(json.dumps(vocab.itos,ensure_ascii=False))
    files={}
    for name,array in [('gamma',gamma),('beta',beta),('weight',weight)]:
        path=args.out/('tail-'+name+'.f32');path.write_bytes(array.astype('<f4').tobytes())
        files[name]={'name':path.name,'sha256':sha(path),'shape':list(array.shape)}
    manifest={'format':'zhiju-split/1','structure':args.structure,'dimension':lm.config.d_model,
        'vocabularyCount':len(vocab),'epsilon':lm.ln_f.eps,'storage':'little-endian-float32-C',
        'checkpointSHA256':sha(args.checkpoint),'trainedStep':checkpoint['step'],'tailFiles':files,
        'normalizationRepresentation':args.normalization,'bodyLayout':args.body_layout,
        'bodyNormalizationPrecision':args.body_normalization_precision}
    (args.out/'split.json').write_text(json.dumps(manifest,indent=2)+'\n')
    report={'structure':args.structure,'checkpointSHA256':sha(args.checkpoint),
        'sourceSHA256':sha(__file__),'tailSourceSHA256':sha(Path(__file__).with_name('split_tail.py')),
        'protocolSHA256':sha(args.protocol),'normalizationRepresentation':args.normalization,'bodyLayout':args.body_layout,
        'bodyNormalizationPrecision':args.body_normalization_precision,
        'normalizationSourceSHA256':sha(Path(__file__).with_name('stable_norm.py')),
        'gemmNormalizationSourceSHA256':sha(Path(__file__).with_name('gemm_norm.py')),
        'alignedNormalizationSourceSHA256':sha(Path(__file__).with_name('aligned_gemm_norm.py')),
        'weightsChangedByExport':False,'variants':{},'passed':True}
    rng=np.random.default_rng(protocol['seed']);predictors={}
    if args.body_layout == 'ane-channels-first':
        from ane_body import ANEHiddenBody
        report['bodySourceSHA256']=sha(Path(__file__).with_name('ane_body.py'))
    for width,name in [(16,'zhiju30m_v3_dist_t16'),(40,'zhiju30m_v3_dist')]:
        tokens=torch.ones((8,width),dtype=torch.int32)
        position=torch.zeros((8,width),dtype=torch.float16);position[:,0]=1
        body_module=(HiddenBody(lm,shape=(8,width),last_block_in_tail=last,normalization=args.normalization)
            if args.body_layout == 'native' else ANEHiddenBody(lm,shape=(8,width)))
        with torch.no_grad():
            traced=torch.jit.trace(body_module.eval(),
                (tokens,) if last else (tokens,position))
        pipeline=copy.deepcopy(ct.PassPipeline.DEFAULT)
        if args.normalization in {'centered-explicit','gemm-stats','hierarchical-stats','aligned-gemm-stats'}:
            fusion='common::fuse_layernorm_or_instancenorm'
            assert fusion in pipeline.passes,'required normalization fusion pass missing'
            pipeline.remove_passes([fusion])
        if args.normalization in {'gemm-stats','hierarchical-stats','aligned-gemm-stats'}:
            # coremltools9 indexes bias.shape[-1] for a scalar epsilon added
            # to a one-output statistical linear op. Preserve the separate
            # add instead of changing epsilon or patching the installed SDK.
            fusion='common::fuse_linear_bias'
            assert fusion in pipeline.passes,'required linear fusion pass missing'
            pipeline.remove_passes([fusion])
        kept_norms={}
        def select_precision(op):
            if op.op_type == 'layer_norm':
                kept_norms[op.name]={'name':op.name,'type':op.op_type,'axes':op.axes.val.tolist()}
                return False
            return True
        precision=(ct.transform.FP16ComputePrecision(op_selector=select_precision)
            if args.body_normalization_precision == 'fp32' else ct.precision.FLOAT16)
        model=ct.convert(traced,convert_to='mlprogram',compute_precision=precision,
            minimum_deployment_target=ct.target.macOS13,skip_model_load=True,pass_pipeline=pipeline,
            inputs=[ct.TensorType(name='tokens',shape=(8,width),dtype=np.int32)]+([] if last else
                    [ct.TensorType(name='position',shape=(8,width),dtype=np.float16)]),
            outputs=[ct.TensorType(name='hidden',dtype=np.float32)])
        if args.body_normalization_precision == 'fp32':
            assert len(kept_norms)==2*len(lm.blocks),'required channel norms missing; refusing silent precision fallback'
        model.user_defined_metadata['zhijuCheckpointSHA256']=report['checkpointSHA256']
        model.user_defined_metadata['zhijuStructure']=args.structure
        model.user_defined_metadata['zhijuNormalizationRepresentation']=args.normalization
        package=args.out/(name+'.mlpackage');model.save(str(package));compiled=compile_mlpackage(package,args.out)
        body=ct.models.CompiledMLModel(str(compiled),compute_units=ct.ComputeUnit.ALL)
        if last:
            hidden=torch.ones((8,width,lm.config.d_model),dtype=torch.float32)
            with torch.no_grad():traced_tail=torch.jit.trace(LastBlockTail(lm,shape=(8,width)).eval(),(hidden,position))
            tail_model=ct.convert(traced_tail,convert_to='mlprogram',compute_precision=ct.precision.FLOAT32,
                minimum_deployment_target=ct.target.macOS13,skip_model_load=True,
                inputs=[ct.TensorType(name='hidden',shape=hidden.shape,dtype=np.float32),
                        ct.TensorType(name='position',shape=(8,width),dtype=np.float16)],
                outputs=[ct.TensorType(name='logprobs',dtype=np.float32)])
            tail_package=args.out/(name+'_tail.mlpackage');tail_model.save(str(tail_package))
            tail_compiled=compile_mlpackage(tail_package,args.out)
            tail_runtime=ct.models.CompiledMLModel(str(tail_compiled),compute_units=ct.ComputeUnit.ALL)
            def predict(x,p,body=body,tail_runtime=tail_runtime):
                hidden=body.predict({'tokens':x})['hidden']
                return tail_runtime.predict({'hidden':hidden,'position':p})['logprobs']
        else:
            def predict(x,p,body=body):
                return tail(body.predict({'tokens':x,'position':p})['hidden'])
        predictors[width]=predict
        got=[];reference=[];padded=[]
        for _ in range(protocol['rowsPerShape']//8):
            lengths=rng.integers(1,width+1,size=8)
            x=np.zeros((8,width),np.int32);p=np.zeros((8,width),np.float16)
            for row,length in enumerate(lengths):
                x[row,:length]=np.r_[1,rng.integers(3,len(vocab),size=length-1)]
                p[row,length-1]=1
                with torch.no_grad():reference.append(torch.log_softmax(lm(torch.from_numpy(x[row:row+1,:length]).long())[0,-1],-1).numpy())
            got.extend(predict(x,p))
            changed=x.copy();pad_rng=np.random.default_rng(1000+len(got))
            for row,length in enumerate(lengths):changed[row,length:]=pad_rng.integers(3,len(vocab),size=width-length)
            padded.extend(predict(changed,p))
        metrics=statistics(np.asarray(got),np.asarray(reference));metrics['passed']=accepts(metrics,protocol)
        metrics['randomPaddingMaxAbsoluteError']=float(np.abs(np.asarray(got)-np.asarray(padded)).max())
        metrics['passed'] &= metrics['randomPaddingMaxAbsoluteError']<=1e-5
        # Warm fixed-input calls are diagnostic only; native/controller admission is later.
        for _ in range(10):predict(x,p)
        durations=[]
        for _ in range(40):
            started=time.perf_counter_ns();predict(x,p);durations.append((time.perf_counter_ns()-started)/1e6)
        metrics['warmCallMs']={'scope':'Python body+tail call, not native40ms admission',
            'p50':float(np.percentile(durations,50)),'p95':float(np.percentile(durations,95))}
        plan=MLComputePlan.load_from_path(str(compiled),compute_units=ct.ComputeUnit.ALL);devices=Counter();operators=Counter()
        for function in plan.model_structure.program.functions.values():
            for op in function.block.operations:
                operators[op.operator_name.rsplit('.',1)[-1]]+=1
                usage=plan.get_compute_device_usage_for_mlprogram_operation(op)
                devices[type(usage.preferred_compute_device).__name__ if usage else 'unknown']+=1
        metrics['bodyEstimatedPreferredDevices']=dict(devices)
        metrics['bodyOperatorCounts']=dict(operators)
        metrics['bodyFP32Operations']=list(kept_norms.values())
        report['variants'][name]=metrics;report['passed'] &= metrics['passed']
        (args.out/'PARITY.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps({name:metrics}),flush=True)
    short=[];full=[]
    for _ in range(8):
        x=np.zeros((8,40),np.int32);p=np.zeros((8,40),np.float16)
        for row,length in enumerate(rng.integers(1,17,size=8)):
            x[row,:length]=np.r_[1,rng.integers(3,len(vocab),size=length-1)];p[row,length-1]=1
        short.extend(predictors[16](x[:,:16].copy(),p[:,:16].copy()));full.extend(predictors[40](x,p))
    metrics=statistics(np.asarray(short),np.asarray(full));metrics['passed']=accepts(metrics,protocol)
    report['shortVsFull']=metrics;report['passed'] &= metrics['passed']
    (args.out/'PARITY.json').write_text(json.dumps(report,indent=2)+'\n')
    paths=[p for p in sorted(args.out.rglob('*')) if p.is_file() and ('.mlmodelc' in str(p.relative_to(args.out)) or p.name in ['itos.json','split.json'] or p.suffix=='.f32')]
    (args.out/'SHA256SUMS').write_text(''.join(f'{sha(p)}  {p.relative_to(args.out)}\n' for p in paths))
    print(json.dumps({'passed':report['passed'],'shortVsFull':metrics}),flush=True)
    return 0 if report['passed'] else 1


if __name__=='__main__':raise SystemExit(main())
