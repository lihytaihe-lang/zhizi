"""Can 顺手·知字 pick the right sentence when it is among the choices?

For each evaluated case, the choices are the target plus the page candidates of the
same length (the decoder's competing segmentations and homophones). Reports how often
the model scores the target highest, separately for cases the current build gets wrong
(would the model rescue them if the decoder offered the answer?) and right (would it
break them?). Local only; the owner cases never leave this machine.
"""
import json, sys, collections
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

sys.path.insert(0, str(ROOT / 'scripts/context_model/zhizi'))
import torch
from eval_owner import ZhiziScorer

ckpt, *outs = sys.argv[1:]
scorer = ZhiziScorer(ckpt, "mps" if torch.backends.mps.is_available() else "cpu")
for path in outs:
    tally = collections.defaultdict(lambda: [0, 0])
    examples = []
    for line in open(path):
        r = json.loads(line)
        t = r["target"]
        rivals = [p for p in r["page"] if p != t and len(p) == len(t) and all("一" <= c <= "鿿" for c in p)]
        if not rivals or not all("一" <= c <= "鿿" for c in t):
            continue
        cands = [t] + rivals[:7]
        scores = scorer.score(r.get("context") or "", cands)
        win = max(range(len(cands)), key=lambda i: scores[i]) == 0
        key = (r["bucket"], "now-wrong" if not r["first"] else "now-right")
        tally[key][0] += win
        tally[key][1] += 1
        if not r["first"] and len(examples) < 12:
            examples.append((r["keys"], t, cands[max(range(len(cands)), key=lambda i: scores[i])], rivals[:2]))
    print("==", path.split("/")[-1])
    for key in sorted(tally):
        w, n = tally[key]
        print("  %-14s %-9s model picks target %4d / %4d = %.1f%%" % (key[0], key[1], w, n, 100 * w / n))
    for e in examples:
        print("   ", e)
