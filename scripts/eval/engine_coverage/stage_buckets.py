import json,glob,os,collections,re
R=os.path.expanduser("~/Library/Input Methods/Agent Input Method.app/Contents/Resources/Rime")
words=set()
for f in glob.glob(R+"/*.dict.yaml"):
    for l in open(f,encoding='utf-8'):
        if l.startswith('#') or '\t' not in l: continue
        words.add(l.split('\t')[0])
def key(r): return (r['raw'],r['context'],r['chosen'])
for arm,pool,rep in [('clean','pool_clean.jsonl','replay_clean.jsonl'),('owner','pool_owner.jsonl','replay_owner.jsonl')]:
    P=[json.loads(l) for l in open(pool)]
    V={}
    for l in open(rep):
        d=json.loads(l); V[(d['raw'],d['context'],d['chosen'])]=d
    n=len(P); c=collections.Counter()
    for r in P:
        v=V.get(key(r))
        final_ok = v is not None and v['now']==r['chosen']
        gen = r['rank_all']>=0 or r['in_comp']
        if final_ok: c['V top1 ok']+=1; continue
        if not gen:
            ch=r['chosen']
            if re.search('[A-Za-z]',ch): c['G miss: ascii/English']+=1
            elif ch in words: c['G miss: whole word IS in lexicon']+=1
            else: c['G miss: not a lexicon word']+=1
        elif r['rank_p1']>=0: c['in page1 but not final top1 (D)']+=1
        elif r['rank_all']>=0: c['later page only']+=1
        else: c['only in C3 compositions (not on any page)']+=1
    print('##',arm,n,'joined',sum(1 for r in P if key(r) in V))
    for k,v in c.most_common(): print(f'  {v:5d} {v/n:6.3f}  {k}')
