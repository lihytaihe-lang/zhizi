#!/usr/bin/env python3
"""Compare two eval outputs of scripts/eval/model_ab.sh: first-choice and top-5 fixed/broken per set.
    scripts/eval/ab_compare.py <outdir> <labelBase> <labelNew> [<set>...]   (sets default: own ext)"""
import json, sys
out, base, new = sys.argv[1:4]
sets = sys.argv[4:] or ["own", "ext"]
load = lambda f: {json.loads(l)["id"]: json.loads(l) for l in open(f)}
for s in sets:
    b, n = load(f"{out}/out-{s}-{base}.jsonl"), load(f"{out}/out-{s}-{new}.jsonl")
    for key, name in (("first", "first"), ("onPage", "top5")):
        fixed = [i for i in b if not b[i][key] and n[i][key]]
        broken = [i for i in b if b[i][key] and not n[i][key]]
        print(f"{s} {name}: {base} {sum(b[i][key] for i in b)} -> {new} {sum(n[i][key] for i in n)}  fixed {len(fixed)} broken {len(broken)}")
        if key == "first":
            print("   fixed :", [(n[i]["keys"], n[i]["target"]) for i in fixed][:12])
            print("   broken:", [(n[i]["keys"], b[i]["page"][0], n[i]["page"][0]) for i in broken][:12])
