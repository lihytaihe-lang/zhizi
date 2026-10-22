"""Sweep rerank policies over probe_rerank.py's real_scores.jsonl.

margin  - model overrides the IME's pick only when it beats it by > tau nats
blend   - score = lm - beta * ime_rank (the IME order carries the owner's learned habits)

Every row reports fixes and breaks together (07-29 lesson: never gains alone).
`--sample N` prints N random breaks of the chosen policy for a manual label-noise audit.
"""

import argparse
import json
import random


def pick_margin(x, tau):
    s = x["scores"]
    best = max(range(len(s)), key=lambda i: s[i])
    return best if best != 0 and s[best] - s[0] > tau else 0


def pick_blend(x, beta):
    s = x["scores"]
    return max(range(len(s)), key=lambda i: s[i] - beta * i)


def tally(rows, pick):
    fixes = breaks = 0
    broken = []
    for x in rows:
        p = x["set"][pick(x)]
        ime = x["set"][0]
        if ime != x["gold"] and p == x["gold"]:
            fixes += 1
        elif ime == x["gold"] and p != x["gold"]:
            breaks += 1
            broken.append((x, p))
    return fixes, breaks, broken


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scores")
    ap.add_argument("--sample", type=int, default=0)
    ap.add_argument("--policy", default="blend:2")
    args = ap.parse_args()
    rows = [json.loads(line) for line in open(args.scores, encoding="utf-8")]
    wrong = sum(1 for x in rows if x["set"][0] != x["gold"])
    print(f"cases={len(rows)} ime_wrong={wrong}")
    for tau in [0, 1, 2, 3, 5, 8]:
        f, b, _ = tally(rows, lambda x: pick_margin(x, tau))
        print(f"margin tau={tau:<4} fixes={f:<4} breaks={b:<4} net={f - b:+}")
    for beta in [0, 1, 2, 3, 4, 6, 8]:
        f, b, _ = tally(rows, lambda x: pick_blend(x, beta))
        print(f"blend  beta={beta:<4} fixes={f:<4} breaks={b:<4} net={f - b:+}")
    if args.sample:
        kind, val = args.policy.split(":")
        fn = pick_blend if kind == "blend" else pick_margin
        _, _, broken = tally(rows, lambda x: fn(x, float(val)))
        random.seed(7)
        for x, p in random.sample(broken, min(args.sample, len(broken))):
            print(json.dumps({"ctx": x["context"], "py": x["pinyin"], "gold": x["gold"], "model": p},
                             ensure_ascii=False))


if __name__ == "__main__":
    main()
