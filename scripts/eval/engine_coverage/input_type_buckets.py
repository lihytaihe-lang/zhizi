import json,collections,re
for arm,pool,rep in [('clean','pool_clean.jsonl','replay_clean.jsonl'),('owner','pool_owner.jsonl','replay_owner.jsonl')]:
    P=[json.loads(l) for l in open(pool)]
    V={(d['raw'],d['context'],d['chosen']):d for d in map(json.loads,open(rep))}
    b=collections.defaultdict(lambda:[0,0,0])
    for r in P:
        ch=r['chosen']; L=len(re.findall(r'[一-鿿]',ch))
        if L==0: k='non-Chinese'
        else:
            ratio=len(r['raw'])/L
            k='abbrev/initials (<=1.5 letters/char)' if ratio<=1.5 else 'mixed (1.5-3)' if ratio<=3 else 'full-ish (>3)'
        v=V[(r['raw'],r['context'],r['chosen'])]
        gen=r['rank_all']>=0 or r['in_comp']
        b[k][0]+=1; b[k][1]+= (not gen); b[k][2]+= (v['now']!=ch)
    print('##',arm)
    for k,(n,g,w) in sorted(b.items()): print(f'  {k:40s} n={n:5d}  G-miss={g:4d} ({g/n:5.1%})  final-wrong={w:4d} ({w/n:5.1%})')
