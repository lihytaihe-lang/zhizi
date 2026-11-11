import json,numpy as np,collections
R=[json.loads(l) for l in open('replay_gc.jsonl')]
print('now ok',np.mean([r['now']==r['chosen'] for r in R]).round(4))
G=[r for r in R if r['gtexts'] and r['chosen'] in r['gtexts']]
A=[r for r in R if r['gtexts']]
print('cases with group',len(A),'chosen in group',len(G))
def first_ok(r): return r['gtexts'][0]==r['chosen']
def model_ok(r): return r['gtexts'][int(np.argmax(r['gscores']))]==r['chosen']
def cur_ok(r): return r['now']==r['chosen']
for name,f in [('decoder first',first_ok),('model argmax (raw)',model_ok),('current pipeline',cur_ok)]:
    print(f'  {name:22s}',np.mean([f(r) for r in G]).round(4))
# rank-penalty sweep, simple: key = score - pen*index, no protections
def pen_ok(r,pen):
    k=[s-pen*i for i,s in enumerate(r['gscores'])]
    return r['gtexts'][int(np.argmax(k))]==r['chosen']
for pen in [0,1,2,3,4,6,8,12,20]:
    print(' pen',pen,np.mean([pen_ok(r,pen) for r in G]).round(4))
# split: same reading homophone-only groups vs mixed
def kind(r): 
    h=r['ghomo']; return 'all-homophone' if all(h) else 'mixed lengths/readings'
b=collections.defaultdict(list)
for r in G: b[kind(r)].append(r)
for k,v in b.items():
    print(k,len(v),'first',np.mean([first_ok(r) for r in v]).round(3),'model',np.mean([model_ok(r) for r in v]).round(3),'current',np.mean([cur_ok(r) for r in v]).round(3), 'best pen2', np.mean([pen_ok(r,2) for r in v]).round(3))
# errors: where model argmax is right but current wrong, and vice versa
mr_cw=[r for r in G if model_ok(r) and not cur_ok(r)]; mw_cr=[r for r in G if (not model_ok(r)) and cur_ok(r)]
print('model right/current wrong',len(mr_cw),' model wrong/current right',len(mw_cr))
for r in mr_cw[:15]:
    i=r['gtexts'].index(r['chosen']); print(' ',r['raw'],'chosen',r['chosen'],'now',r['now'],'scores',[(t,round(s,1)) for t,s in zip(r['gtexts'][:4],r['gscores'][:4])],'idx',i)
