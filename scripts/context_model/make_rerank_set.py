"""Turn eval-harness output (run with ZHIZI_EVAL_TRACE=1) into rerank training rows (MODEL-V3-PLAN S1).

  python make_rerank_set.py cases.jsonl eval-output.jsonl out.jsonl

Each row: {"ctx","keys","bucket","source","cands":[...],"gold":index}. The candidates are what the
core offers the model in deployment: the page candidates in the model's masks (covering the whole input, or
the homophone group), then the whole-sentence alternatives, de-duplicated in that order. Rows whose candidates do not contain the target (gold = -1) are kept in the file but
flagged `available: false`: the trainer ignores them; they measure how often the core cannot even
offer the answer. Public / synthetic text only: no owner data is read.
"""
import collections
import json
import sys

cases = {}
for line in open(sys.argv[1], encoding="utf-8"):
    row = json.loads(line)
    cases[row["id"]] = row
stats = collections.defaultdict(lambda: [0, 0, 0])  # bucket -> [rows, available, candidates]
with open(sys.argv[3], "w", encoding="utf-8") as out:
    for line in open(sys.argv[2], encoding="utf-8"):
        result = json.loads(line)
        case = cases.get(result["id"])
        if not case:
            continue
        cands, seen = [], set()
        # What the model scores in deployment: the page candidates in its masks (the ones that cover the
        # whole input, and the homophone group) and the whole-sentence alternatives. Older outputs
        # without `scored` fall back to the whole first page.
        scored = result["scored"] if "scored" in result else (result.get("page") or [])
        for text in list(scored) + list(result.get("alternatives") or []):
            if text and text not in seen:
                seen.add(text)
                cands.append(text)
        gold = cands.index(case["target"]) if case["target"] in cands else -1
        out.write(json.dumps({"id": case["id"], "ctx": case["context"], "keys": case["keys"], "bucket": case["bucket"],
                              "source": case.get("source", ""), "cands": cands, "gold": gold,
                              "available": gold >= 0}, ensure_ascii=False) + "\n")
        s = stats[case["bucket"]]
        s[0] += 1
        s[1] += gold >= 0
        s[2] += len(cands)
print("bucket\trows\tavailable\tavg_candidates")
total = [0, 0, 0]
for bucket, (n, a, c) in sorted(stats.items()):
    print(f"{bucket}\t{n}\t{a} ({100*a/n:.0f}%)\t{c/n:.1f}")
    total = [total[0] + n, total[1] + a, total[2] + c]
print(f"all\t{total[0]}\t{total[1]} ({100*total[1]/total[0]:.0f}%)\t{total[2]/total[0]:.1f}")
