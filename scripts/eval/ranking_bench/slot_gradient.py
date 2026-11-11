import json,collections,os
R=os.environ.get('ZHIZI_REPO','.').rstrip('/')+'/'
G=R+'tmp/worktrees/rank-g3-wide-model-upper-bound/tmp/delegation/rank-g3-wide-model-upper-bound/'
load=lambda p:{json.loads(l)['id']:json.loads(l) for l in open(p)}
cur=load(G+'current/current.jsonl')
pool={k:v for k,v in load(R+'tmp/rank-base/pool1/pool.jsonl').items() if k in cur}
def merge(a,b,k,size=5,first_b=0):
    out=[]
    for x in b[:first_b]:
        if x not in out: out.append(x)
    for x in a:
        if len(out)>=first_b+k: break
        if x not in out: out.append(x)
    for x in b:
        if len(out)>=size: break
        if x not in out: out.append(x)
    return out[:size]
def score(name,fn):
    t1=t2=t5=0;fix=brk=0
    for i,c in cur.items():
        p=fn(i); t=c['target']
        if p[:1]==[t]: t1+=1
        if t in p[:2]: t2+=1
        if t in p[:5]: t5+=1
        was=c['candidates'][:1]==[t]
        now=p[:1]==[t]
        fix+=now and not was; brk+=was and not now
    n=len(cur); print(f'{name:34} 首位{100*t1/n:6.2f} 前二{100*t2/n:6.2f} 前五{100*t5/n:6.2f} 修{fix:3d} 坏{brk:3d}')
score('现状(线上流程)',lambda i:cur[i]['candidates'][:5])
score('纯知序(无模型)',lambda i:pool[i]['candidates'][:5])
for N in (20,40,100):
    m=load(G+f'wide/n{N}-w0.jsonl')
    print('--- N',N)
    for k in (1,2,3,5):
        score(f'知字{k}个+现状补足',lambda i:merge(m[i]['candidates'],cur[i]['candidates'],k))
    for k in (1,2,3):
        score(f'知字{k}个+纯知序补足',lambda i:merge(m[i]['candidates'],pool[i]['candidates'],k))
    score('现状第1+知字补4',lambda i:merge(m[i]['candidates'],cur[i]['candidates'],4,first_b=1))
    # slot 1 = current, slot 2 = model, rest current
    score('现状第1+知字第2+现状补',lambda i:merge(m[i]['candidates'],cur[i]['candidates'],1,first_b=1))
# context-length effect
print('--- 上文长度')
m=load(G+'wide/n40-w0.jsonl')
b=collections.defaultdict(lambda:[0,0,0,0,0])
for i,c in cur.items():
    L=len(c['context']); k='0' if L==0 else '1-3' if L<=3 else '4-8' if L<=8 else '9-16' if L<=16 else '17+'
    r=b[k]; r[0]+=1; r[1]+=c['candidates'][:1]==[c['target']]; r[2]+=m[i]['candidates'][:1]==[c['target']]; r[3]+=c['target'] in c['candidates'][:5]; r[4]+=c['target'] in m[i]['candidates'][:5]
for k in ['0','1-3','4-8','9-16','17+']:
    r=b[k]
    if r[0]: print(k,'n',r[0],'现状首位 %.1f 知字40首位 %.1f 现状前五 %.1f 知字40前五 %.1f'%tuple(100*x/r[0] for x in r[1:]))
# outcome by bucket
print('--- 模型触发')
o=collections.defaultdict(collections.Counter)
for l in open(R+'tmp/rank-base/rep1/replay.jsonl'):
    r=json.loads(l); o[r['bucket']][r['outcome'] if r['model_ready'] or r['outcome']!='no_group' else 'no_task']+=1
for k,v in o.items(): print(k,dict(v))
