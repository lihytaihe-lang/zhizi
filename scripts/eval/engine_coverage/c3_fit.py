import json,sys,numpy as np
from scipy.optimize import minimize
arm=sys.argv[1]
R=[json.loads(l) for l in open(f'replay_{arm}.jsonl')]
C=[r for r in R if len(r['texts'])>=2 and r['scores'] and r['chosen'] in r['texts']]
allC=[r for r in R if len(r['texts'])>=2 and r['scores']]
print(arm,'C3 cases',len(allC),'chosen in C3',len(C))
def feats(r):
    t=r['texts'];s=np.array(r['scores']);e=np.array(r['edits']);n=np.array([max(1,len(x)) for x in t],float)
    idx=np.arange(len(t),dtype=float)
    F=np.stack([s, e, (idx==0).astype(float), np.log1p(idx), s/n, n],1)
    return F
X=[feats(r) for r in C]; y=[r['texts'].index(r['chosen']) for r in C]
def nll(w,idxs,l2=1e-2):
    tot=0
    for i in idxs:
        z=X[i]@w; z=z-z.max(); lp=z-np.log(np.exp(z).sum()); tot-=lp[y[i]]
    return tot/len(idxs)+l2*(w@w)
def acc(w,idxs): return np.mean([ (X[i]@w).argmax()==y[i] for i in idxs])
n=len(C); folds=5; sz=n//folds
cur=lambda i: np.argmax(X[i][:,0]-4*X[i][:,1])  # unconstrained argmax (no margin)
curm=lambda i: (lambda tot:(np.argmax(tot[1:])+1 if tot[1:].max()-tot[0]>4 else 0))(X[i][:,0]-4*X[i][:,1])
a_fit=[];a_cur=[];a_curm=[]
for f in range(folds):
    te=list(range(f*sz,(f+1)*sz)); tr=[i for i in range(n) if i not in set(te)]
    w=minimize(nll,np.array([1,-4,0,0,0,0.0]),args=(tr,),method='L-BFGS-B').x
    a_fit.append(acc(w,te)); a_cur.append(np.mean([cur(i)==y[i] for i in te])); a_curm.append(np.mean([curm(i)==y[i] for i in te]))
print('blocked CV top1 on C3 cases where chosen is in the list:')
print(' current rule (margin 4)',np.mean(a_curm).round(4),' argmax(score-4*edit)',np.mean(a_cur).round(4),' fitted 6-feature',np.mean(a_fit).round(4))
w=minimize(nll,np.array([1,-4,0,0,0,0.0]),args=(list(range(n)),),method='L-BFGS-B').x
print('weights [score, edit, is_first, log1p(idx), score/len, len]:',w.round(3))
