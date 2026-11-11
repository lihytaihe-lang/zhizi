import json,sys,itertools
arm=sys.argv[1]
R=[json.loads(l) for l in open(f'replay_{arm}.jsonl')]
C=[r for r in R if len(r['texts'])>=2 and r['scores']]
def lead(r,m,pen,lam=1.0):
    t=r['texts'];s=r['scores'];e=r['edits']
    tot=[lam*s[i]-pen*e[i] for i in range(len(t))]
    best=max(range(1,len(t)),key=lambda i:(tot[i],-i))
    return t[best] if tot[best]-tot[0]>m else None
def final(r,m,pen,lam=1.0):
    l=lead(r,m,pen,lam)
    if l: return l
    return r['texts'][0] if r['outcome']=='composed' else r['now']
base_ok=sum(1 for r in C if r['now']==r['chosen'])
agree=sum(1 for r in C if final(r,4.0,4.0)==r['now'])
print(arm,'cases with C3',len(C),'base ok',base_ok,'sim agrees with recorded now',agree)
res=[]
for m in [0,1,2,3,4,5,6,8,10,100]:
    for pen in [0,2,4,6,8]:
        ok=0;fixed=0;broke=0
        for r in C:
            f=final(r,m,pen); b=r['now']==r['chosen']
            ok+= f==r['chosen']
            fixed+= (f==r['chosen'] and not b); broke+= (f!=r['chosen'] and b)
        res.append((ok,m,pen,fixed,broke))
for ok,m,pen,f,b in sorted(res,reverse=True)[:8]: print(f'margin={m} pen={pen}: ok={ok} (base {base_ok}) fixed={f} broke={b}')
print('current (4,4):',[x for x in res if x[1]==4 and x[2]==4])
print('no C3 lead at all:',[x for x in res if x[1]==100][0])
