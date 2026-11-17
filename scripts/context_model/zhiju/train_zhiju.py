"""Fine-tune independent Zhiju weights; only generated target tokens carry loss.

The source Zhizi checkpoint and its product ranking assets are read-only inputs.
"""
import argparse
import hashlib
import json
import math
import random
import sys
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'zhizi'))
from model import load, save


def encode_sequence(vocab, context, target, max_tokens=40):
    if not isinstance(context, str) or not isinstance(target, str) or not target:
        raise ValueError('context must be string and target nonempty string')
    if (vocab.pad, vocab.bos, vocab.unk) != (0, 1, 2):
        raise ValueError('unexpected special token IDs')
    if len(context) + len(target) + 1 > max_tokens:
        raise ValueError('sequence exceeds runtime window; no silent truncation')
    encoded = vocab.encode(context + target)
    if any(i < 3 for i in encoded):
        raise ValueError('unknown or special token in text')
    return [vocab.bos] + encoded, [False] * len(context) + [True] * len(target)


def sequence_statistics(model, sequences, device, return_prefixes=False):
    if not sequences:
        raise ValueError('empty sequences')
    width = max(len(ids) - 1 for ids, _ in sequences)
    x = torch.zeros((len(sequences), width), dtype=torch.long, device=device)
    y = torch.zeros_like(x)
    mask = torch.zeros_like(x, dtype=torch.bool)
    for i, (ids, weights) in enumerate(sequences):
        if len(weights) != len(ids) - 1 or not any(weights):
            raise ValueError('invalid target mask')
        x[i, :len(ids)-1] = torch.tensor(ids[:-1], device=device)
        y[i, :len(ids)-1] = torch.tensor(ids[1:], device=device)
        mask[i, :len(weights)] = torch.tensor(weights, device=device)
    logits = model(x)
    logp = F.log_softmax(logits, dim=-1).gather(-1, y.unsqueeze(-1)).squeeze(-1)
    target_logp = logp * mask
    statistics = (target_logp.sum(-1), mask.sum(-1))
    return (*statistics, target_logp.cumsum(-1)) if return_prefixes else statistics


def objective(model, rows, vocab, device, contrast_weight=.25, margin=1., prefix_weight=0.):
    if not rows:
        raise ValueError('empty rows')
    for value in [contrast_weight, margin, prefix_weight]:
        if not math.isfinite(value) or value < 0:
            raise ValueError('nonfinite or negative loss argument')
    sequences = [encode_sequence(vocab, r['context'], r['target']) for r in rows]
    pairs = []
    spans = []
    for i, row in enumerate(rows):
        negatives = row.get('negatives', [])
        if (not isinstance(negatives, list) or len(negatives) > 2
            or any(not isinstance(n, str) or not n or n == row['target'] for n in negatives)
            or len(set(negatives)) != len(negatives)):
            raise ValueError('invalid negative candidates')
        for negative in negatives:
            pairs.append((i, len(sequences)))
            if prefix_weight:
                if len(negative) != len(row['target']):
                    raise ValueError('prefix competition requires equal-length legal alternatives')
                divergent = next(j for j,(positive,other) in enumerate(zip(row['target'],negative)) if positive != other)
                spans.append((i,len(sequences),len(row['context'])+divergent,len(row['context'])+len(negative)))
            sequences.append(encode_sequence(vocab, row['context'], negative))
    statistics = sequence_statistics(model, sequences, device, return_prefixes=bool(prefix_weight))
    scores, counts = statistics[:2]
    nll = -scores[:len(rows)].sum() / counts[:len(rows)].sum()
    contrast = scores.new_zeros(())
    correct = 0
    if pairs:
        positive = torch.stack([scores[p] for p, _ in pairs])
        negative = torch.stack([scores[n] for _, n in pairs])
        contrast = F.softplus(negative - positive + margin).mean()
        correct = int((positive > negative).sum().detach())
    prefix = scores.new_zeros(())
    prefix_terms = prefix_correct = 0
    if prefix_weight:
        from prefix_contrast import prefix_pairwise_loss
        prefix, prefix_terms, prefix_correct_tensor = prefix_pairwise_loss(statistics[2],spans,margin)
        prefix_correct = int(prefix_correct_tensor.detach())
    loss = nll + contrast_weight * contrast + prefix_weight * prefix
    if not torch.isfinite(loss).item():
        raise ValueError('nonfinite loss')
    return loss, {'loss':float(loss.detach()), 'continuation_loss':float(nll.detach()),
        'contrast_loss':float(contrast.detach()), 'tokens':int(counts[:len(rows)].sum().detach()),
        'pairs':len(pairs), 'contrast_correct':correct,
        'prefix_loss':float(prefix.detach()),'prefix_terms':prefix_terms,'prefix_correct':prefix_correct}


def read_rows(path, vocab, rehearsal=False):
    rows = []
    with Path(path).open(encoding='utf-8') as f:
        for index, line in enumerate(f, 1):
            if not line.strip():
                continue
            row = json.loads(line)
            if not isinstance(row, dict):
                raise ValueError(f'{path}:{index} is not an object')
            encode_sequence(vocab, row.get('context'), row.get('target'))
            negatives = row.get('negatives', [])
            if (not isinstance(negatives, list) or len(negatives) > 2
                or any(not isinstance(n, str) or not n or n == row['target'] for n in negatives)
                or len(set(negatives)) != len(negatives)):
                raise ValueError(f'{path}:{index} invalid negatives')
            if rehearsal and negatives:
                raise ValueError('rehearsal must not contain contrast labels')
            for n in negatives:
                encode_sequence(vocab, row['context'], n)
            rows.append(row)
    if not rows:
        raise ValueError(f'empty dataset {path}')
    return rows


@torch.no_grad()
def evaluate(model, rows, vocab, device, batch, contrast_weight, margin, prefix_weight=0.):
    was_training = model.training
    model.eval()
    nll = contrast = tokens = pairs = correct = prefix = prefix_terms = prefix_correct = 0
    try:
        for i in range(0, len(rows), batch):
            _, m = objective(model, rows[i:i+batch], vocab, device, contrast_weight, margin, prefix_weight)
            nll += m['continuation_loss'] * m['tokens']
            contrast += m['contrast_loss'] * m['pairs']
            tokens += m['tokens']; pairs += m['pairs']; correct += m['contrast_correct']
            prefix += m['prefix_loss'] * m['prefix_terms']
            prefix_terms += m['prefix_terms']; prefix_correct += m['prefix_correct']
    finally:
        model.train(was_training)
    return {'loss':nll/tokens + contrast_weight*(contrast/pairs if pairs else 0) + prefix_weight*(prefix/prefix_terms if prefix_terms else 0),
        'continuation_loss':nll/tokens, 'contrast_loss':contrast/pairs if pairs else 0,
        'tokens':tokens, 'pairs':pairs, 'contrast_correct':correct,
        'contrast_accuracy':correct/pairs if pairs else None,
        'prefix_loss':prefix/prefix_terms if prefix_terms else 0,'prefix_terms':prefix_terms,
        'prefix_correct':prefix_correct,'prefix_accuracy':prefix_correct/prefix_terms if prefix_terms else None}


def digest(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as f:
        for block in iter(lambda:f.read(1024*1024), b''):
            h.update(block)
    return h.hexdigest()


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    for name in ['init', 'train', 'valid', 'out']:
        ap.add_argument('--'+name, type=Path, required=True)
    ap.add_argument('--rehearsal', type=Path)
    for name, value in [('steps',1200),('batch',16),('warmup',60),('eval-every',200),('seed',20261115),('hard-batch',4)]:
        ap.add_argument('--'+name, type=int, default=value)
    for name, value in [('lr',3e-5),('contrast-weight',.25),('margin',1.),('prefix-weight',0.),('rehearsal-weight',.2),('time-limit-seconds',6600.)]:
        ap.add_argument('--'+name, type=float, default=value)
    ap.add_argument('--device', choices=['auto','cpu','mps','cuda'], default='auto')
    args = ap.parse_args(argv)
    if min(args.steps,args.batch,args.eval_every) <= 0 or min(args.warmup,args.hard_batch) < 0:
        raise ValueError('invalid step or batch counts')
    if (not all(math.isfinite(v) and v >= 0 for v in [args.lr,args.contrast_weight,args.margin,args.rehearsal_weight,args.time_limit_seconds])
        or not math.isfinite(args.prefix_weight) or args.prefix_weight < 0
        or args.lr == 0 or args.time_limit_seconds == 0):
        raise ValueError('invalid training hyperparameters')
    if args.out.exists() and any(args.out.iterdir()):
        raise ValueError('refusing to overwrite nonempty output')
    args.out.mkdir(parents=True, exist_ok=True)
    torch.set_num_threads(4)
    random.seed(args.seed); np.random.seed(args.seed); torch.manual_seed(args.seed)
    device = args.device
    if device == 'auto':
        device = 'cuda' if torch.cuda.is_available() else 'mps' if torch.backends.mps.is_available() else 'cpu'
    model, vocab, checkpoint = load(args.init, device)
    train = read_rows(args.train, vocab); valid = read_rows(args.valid, vocab)
    rehearsal = read_rows(args.rehearsal, vocab, True) if args.rehearsal else []
    hard = [r for r in train if r.get('negatives')]
    inputs = {name:{'path':str(getattr(args,name).resolve()),'sha256':digest(getattr(args,name))}
        for name in ['init','train','valid','rehearsal'] if getattr(args,name)}
    run = {'args':{k:str(v) if isinstance(v,Path) else v for k,v in vars(args).items()},
        'inputs':inputs, 'torch':torch.__version__, 'device':device, 'parameters':model.n_params(),
        'trainerSHA256':digest(__file__),
        'completed':False, 'trainRows':len(train), 'validRows':len(valid), 'hardRows':len(hard),
        'rehearsalRows':len(rehearsal), 'itosSHA256':hashlib.sha256(json.dumps(vocab.itos,ensure_ascii=False).encode()).hexdigest()}
    def record_run():
        (args.out/'run.json').write_text(json.dumps(run,indent=2)+'\n')
    record_run()
    vocab_path = args.out/'vocab.json';vocab_path.write_text(json.dumps(vocab.itos,ensure_ascii=False))
    optimizer = torch.optim.AdamW(model.parameters(),lr=args.lr,betas=(.9,.95),weight_decay=.01)
    rng = random.Random(args.seed); started = time.monotonic(); best = float('inf'); best_step = None
    def save_checkpoint(name, step, metrics):
        temporary = args.out/(name+'.partial')
        save(temporary,model,str(vocab_path),step,{'zhijuTraining':True,'sourceCheckpointSHA256':inputs['init']['sha256'],
            'recipe':run['args'],'inputSHA256':{k:v['sha256'] for k,v in inputs.items()},'valid':metrics})
        temporary.replace(args.out/name)
    with (args.out/'log.jsonl').open('x') as log:
        def emit(value):
            value.update(elapsedSeconds=round(time.monotonic()-started,3))
            line=json.dumps(value);print(line,flush=True);log.write(line+'\n');log.flush()
        emit({'step':0,'kind':'baseline','valid':evaluate(model,valid,vocab,device,args.batch,args.contrast_weight,args.margin,args.prefix_weight)})
        model.train()
        for step in range(1,args.steps+1):
            if time.monotonic()-started >= args.time_limit_seconds:
                run.update(completed=False,steps=step-1,exitCode=124,elapsedSeconds=time.monotonic()-started)
                save_checkpoint('last.pt',step-1,{})
                record_run();return 124
            lr=args.lr*min(1.,step/max(1,args.warmup))*(.1+.9*.5*(1+math.cos(math.pi*(step-1)/args.steps)))
            for group in optimizer.param_groups:group['lr']=lr
            batch=rng.choices(train,k=args.batch)
            if hard and args.hard_batch:batch += rng.choices(hard,k=args.hard_batch)
            optimizer.zero_grad(set_to_none=True)
            loss,metrics=objective(model,batch,vocab,device,args.contrast_weight,args.margin,args.prefix_weight)
            if rehearsal and args.rehearsal_weight:
                extra,_=objective(model,rng.choices(rehearsal,k=args.batch),vocab,device,0.,args.margin)
                loss=loss+args.rehearsal_weight*extra
            loss.backward();torch.nn.utils.clip_grad_norm_(model.parameters(),1.);optimizer.step()
            if step%50==0:emit({'step':step,'kind':'train','lr':lr,'metrics':metrics,'totalLoss':float(loss.detach())})
            if step%args.eval_every==0 or step==args.steps:
                metrics=evaluate(model,valid,vocab,device,args.batch,args.contrast_weight,args.margin,args.prefix_weight)
                emit({'step':step,'kind':'validation','valid':metrics})
                if metrics['loss']<best:
                    best=metrics['loss'];best_step=step;save_checkpoint('best.pt',step,metrics)
        save_checkpoint('last.pt',args.steps,metrics)
    run.update(completed=True,steps=args.steps,bestStep=best_step,bestValidLoss=best,
        elapsedSeconds=time.monotonic()-started,exitCode=0,bestSHA256=digest(args.out/'best.pt'),lastSHA256=digest(args.out/'last.pt'))
    record_run();return 0


if __name__ == '__main__':
    raise SystemExit(main())
