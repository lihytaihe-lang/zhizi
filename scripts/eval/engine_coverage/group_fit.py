import json,numpy as np
from scipy.optimize import minimize
R=[json.loads(l) for l in open('replay_gc.jsonl')]
G=[r for r in R if r['gtexts'] and r['chosen'] in r['gtexts']]
def feats(r):
    s=np.array(r['gscores']);n=np.array([len(t) for t in r['gtexts']],float);i=np.arange(len(s),dtype=float);h=np.array(r['ghomo'],float)
    return np.stack([s,(i==0)*1.0,np.log1p(i),s/n,n,h,(i==0)*h],1)
X=[feats(r) for r in G]; y=[r['gtexts'].index(r['chosen']) for r in G]
def nll(w,idx,l2=1e-2):
    t=0
    for i in idx:
        z=X[i]@w;z=z-z.max();t-=z[y[i]]-np.log(np.exp(z).sum())
    return t/len(idx)+l2*(w@w)
n=len(G);k=5;sz=n//k;res=[]
for cols in [[0],[0,1],[0,1,2],[0,1,2,3],[0,1,2,3,4,5,6]]:
    acc=[]
    for f in range(k):
        te=set(range(f*sz,(f+1)*sz));tr=[i for i in range(n) if i not in te]
        mask=np.zeros(7);mask[cols]=1
        fun=lambda w:nll(w*mask,tr)
        w=minimize(fun,np.zeros(7),method='L-BFGS-B').x*mask
        acc.append(np.mean([(X[i]@w).argmax()==y[i] for i in te]))
    print(cols,'blocked-CV top1',np.mean(acc).round(4))
print('decoder first',np.mean([0==y[i] for i in range(n)]).round(4),' current',np.mean([r['now']==r['chosen'] for r in G]).round(4))
