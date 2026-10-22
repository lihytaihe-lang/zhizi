"""Score held-out real_scores.jsonl files against the adjudicated answer key.

The raw ruler ("what the owner committed") counts the model's correction of a
silent mis-commit as a break. The key fixes that:
  - owner picked a non-first candidate  -> deliberate, committed is the answer
  - adjudicated (blind, options unordered) -> the labeled option; "either" accepts
    every adjudicated option; "unclear" drops the case
  - otherwise                            -> committed is the answer
Fixes/breaks are then counted against the IME's first candidate, under the same
margin/blend policies as analyze_policies.py.
"""

import glob
import json
import sys
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

ADJ = str(WORK_DIR / 'adjudicate')


def load_key():
    opts = {}
    for p in glob.glob(f"{ADJ}/part*.jsonl"):
        for x in map(json.loads, open(p, encoding="utf-8")):
            opts[x["id"]] = x["options"]
    key = {}
    for p in glob.glob(f"{ADJ}/labels*.jsonl"):
        for x in map(json.loads, open(p, encoding="utf-8")):
            lab = x["label"]
            key[x["id"]] = None if lab == "unclear" else (set(opts[x["id"]]) if lab == "either" else {lab})
    return key


def score(rows, key, pick):
    fixes = breaks = n = ime_ok = model_ok = silent = 0
    for i, x in enumerate(rows):
        s, gold = x["set"], x["gold"]
        if s[0] != gold:
            ok = {gold}
        elif i in key:
            ok = key[i]
            if ok is None:
                continue
        else:
            ok = {gold}
        n += 1
        p = s[pick(x)]
        silent += s[0] == gold and s[0] not in ok
        ime_ok += s[0] in ok
        model_ok += p in ok
        fixes += s[0] not in ok and p in ok
        breaks += s[0] in ok and p not in ok
    return n, ime_ok, model_ok, fixes, breaks, silent


def main():
    key = load_key()
    for path in sys.argv[1:]:
        rows = [json.loads(l) for l in open(path, encoding="utf-8")]
        print(f"== {path.split('/')[-2]}")
        policies = [("margin tau=0", lambda x: max(range(len(x["set"])), key=lambda k: x["scores"][k]))]
        for b in (1, 2, 3, 4, 6):
            policies.append((f"blend beta={b}", lambda x, b=b: max(range(len(x["set"])), key=lambda k: x["scores"][k] - b * k)))
        for name, pick in policies:
            n, ime_ok, model_ok, f, br, silent = score(rows, key, pick)
            print(f"  {name:<14} cases={n} ime_right={ime_ok} model_right={model_ok} "
                  f"fixes={f} breaks={br} net={f - br:+}  (silent mis-commits in key: {silent})")


if __name__ == "__main__":
    main()
