"""First-candidate accuracy by context length, from replay outputs rx<N>.jsonl over the same cases.
usage: context_length.py <dir> [pool_nomodel.jsonl]   (rows align by line number)"""
import json, sys, collections
d = sys.argv[1]
lens = [0, 2, 4, 8, 16, 32]
rows = {n: [json.loads(l) for l in open(f"{d}/rx{n}.jsonl")] for n in lens}
base = [json.loads(l) for l in open(sys.argv[2])] if len(sys.argv) > 2 else None
N = min(len(v) for v in rows.values())
print(f"cases {N}")
def bucket(r):
    k = r["raw"]; return "short(<=5)" if len(k) <= 5 else "mid(6-9)" if len(k) <= 9 else "long(10+)"
print("len  first-correct  model-ready  changed-vs-0")
for n in lens:
    ok = sum(1 for r in rows[n][:N] if r["now"] == r["chosen"])
    ready = sum(1 for r in rows[n][:N] if r["outcome"] not in ("pending", "no_group", "no_model", ""))
    chg = sum(1 for a, b in zip(rows[0][:N], rows[n][:N]) if a["now"] != b["now"])
    print(f"{n:>3}  {ok/N*100:6.2f}%  {ready/N*100:6.1f}%  {chg}")
if base:
    ok = sum(1 for r in base[:N] if r["rank_p1"] == 0)
    print(f"no-model first: {ok/N*100:.2f}%")
print("outcomes @32:", collections.Counter(r["outcome"] for r in rows[32][:N]).most_common(6))
# paired gain 32 vs 0 with sign test
w = sum(1 for a, b in zip(rows[0][:N], rows[32][:N]) if a["now"] != a["chosen"] and b["now"] == b["chosen"])
l = sum(1 for a, b in zip(rows[0][:N], rows[32][:N]) if a["now"] == a["chosen"] and b["now"] != b["chosen"])
print(f"32 vs 0: gained {w}, lost {l}, net {w-l}")
for n in lens[1:]:
    w = sum(1 for a, b in zip(rows[0][:N], rows[n][:N]) if a["now"] != a["chosen"] and b["now"] == b["chosen"])
    l = sum(1 for a, b in zip(rows[0][:N], rows[n][:N]) if a["now"] == a["chosen"] and b["now"] != b["chosen"])
    print(f"  {n:>2} vs 0: +{w} -{l} net {w-l}")
