import json,collections
R=[json.loads(l) for l in open('replay_owner.jsonl')]
P={(d['raw'],d['context'],d['chosen']):d for d in map(json.loads,open('pool_owner.jsonl'))}
err=[r for r in R if r['now']!=r['chosen']]
print('errors',len(err))
tab=collections.Counter()
for r in err:
    p=P[(r['raw'],r['context'],r['chosen'])]
    gen=p['rank_all']>=0 or p['in_comp']
    if not gen: tab[('not generated',r['outcome'])]+=1; continue
    t=r['texts']; has=len(t)>=2 and r['scores']
    inc3 = has and r['chosen'] in t
    tab[(('chosen in C3' if inc3 else ('C3 exists, chosen not in it' if has else 'no C3 list')), r['outcome'])]+=1
for k,v in sorted(tab.items(), key=lambda x:-x[1]): print(v,k)
import random
random.seed(3)
def show(label,cond,n=12):
    xs=[r for r in err if cond(r)]
    print('\n##',label,len(xs))
    for r in random.sample(xs,min(n,len(xs))):
        t=r['texts']; s=r['scores']; e=r['edits']
        extra=''
        if len(t)>=2 and r['chosen'] in t:
            i=t.index(r['chosen']); tot=[s[k]-4*e[k] for k in range(len(t))]
            extra=f" C3: first={t[0]}({tot[0]:.1f}) chosen#{i}({tot[i]:.1f}) best={t[max(range(len(t)),key=lambda k:tot[k])]}({max(tot):.1f})"
        print(f"{r['raw']:16s} chosen={r['chosen']} now={r['now']} out={r['outcome']} page={[x.split('|')[0] for x in r['page'][:4]]}{extra}")
def nogen(r):
    p=P[(r['raw'],r['context'],r['chosen'])]; return not(p['rank_all']>=0 or p['in_comp'])
show('no C3 list, generated', lambda r: not nogen(r) and not(len(r['texts'])>=2 and r['scores']))
show('chosen in C3 / composed', lambda r: not nogen(r) and r['chosen'] in r['texts'] and r['outcome']=='composed')
show('chosen in C3 / no_group+unchanged', lambda r: not nogen(r) and r['chosen'] in r['texts'] and r['outcome'] in('no_group','unchanged'))
