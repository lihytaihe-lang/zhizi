"""Case-by-case comparison of two eval outputs: python3 compare.py before.jsonl after.jsonl [shown]"""
import json
import sys


def load(path):
    return {r["id"]: r for r in map(json.loads, open(path, encoding="utf-8"))}


before, after = load(sys.argv[1]), load(sys.argv[2])
good = [i for i in before if i in after and not before[i]["first"] and after[i]["first"]]
bad = [i for i in before if i in after and before[i]["first"] and not after[i]["first"]]
print("improved", len(good), "regressed", len(bad))
for i in bad:
    print("  BAD", after[i]["bucket"], after[i]["keys"], after[i]["target"], "->", after[i]["page"][:2])
lost = [i for i in before if i in after and before[i]["onPage"] and not after[i]["onPage"]]
gained = [i for i in before if i in after and not before[i]["onPage"] and after[i]["onPage"]]
print("first page: gained", len(gained), "lost", len(lost))
for i in lost:
    print("  LOST", after[i]["bucket"], after[i]["keys"], after[i]["target"], "->", after[i]["page"])
for i in good[: int(sys.argv[3]) if len(sys.argv) > 3 else 15]:
    print("  GOOD", after[i]["bucket"], after[i]["keys"], after[i]["target"], "<-", before[i]["page"][:1])
