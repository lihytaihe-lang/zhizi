import json,sys
base,new=sys.argv[1:3]
b={}
for l in open(base):
    d=json.loads(l); b[d['id']]=d
n=diff=f=t=0; ex=[]
for l in open(new):
    try:d=json.loads(l)
    except: continue
    n+=1; o=b[d['id']]
    if o['page']!=d['page']:
        diff+=1; ex.append((d['keys'],o['page'],d['page']))
    f+= o['first']!=d['first']; t+= o['onPage']!=d['onPage']
print(n,'pages changed',diff,'first flipped',f,'top5 flipped',t); [print(e) for e in ex[:8]]
