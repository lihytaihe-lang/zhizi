import json,sys,collections
def load(p): return [json.loads(l) for l in open(p)]
rows=load(sys.argv[1])
n=len(rows)
p1=sum(1 for r in rows if r['rank_p1']==0)
inp1=sum(1 for r in rows if r['rank_p1']>=0)
allp=sum(1 for r in rows if r['rank_all']>=0)
comp=sum(1 for r in rows if r['in_comp'])
pool=sum(1 for r in rows if r['rank_all']>=0 or r['in_comp'])
nopool=[r for r in rows if r['rank_all']<0 and not r['in_comp']]
print(f"n={n} top1(no model)={p1/n:.3f} in page1={inp1/n:.3f} in all pages={allp/n:.3f} in comps={comp/n:.3f} in pool(any)={pool/n:.3f} not generated={len(nopool)} ({len(nopool)/n:.3f})")
# by raw length / single-char chosen vs multi
def bucket(r):
    L=len(r['chosen']); k=len(r['raw'])
    return 'len1' if L==1 else 'len2-3' if L<=3 else 'len4+'
b=collections.defaultdict(lambda:[0,0])
for r in rows:
    k=bucket(r); b[k][0]+=1; b[k][1]+= (r['rank_all']<0 and not r['in_comp'])
for k,v in sorted(b.items()): print(k,v[0],'notgen',v[1], f"{v[1]/v[0]:.3f}")
json.dump([ (r['raw'],r['context'][-6:],r['chosen'],r['top']) for r in nopool],open(sys.argv[1]+'.nogen.json','w'),ensure_ascii=False)
