"""Find silent mis-commits: the IME's first candidate was wrong, the owner
committed it anyway and never fixed it, so no redo/non-top1 metric saw it
(e.g. `bushi` -> 不时 committed 24 times). Those then get learned as habits.

For every commit that took the IME's first candidate, a pretrained LM scores
the true homophone set with the preceding text. Disagreements are aggregated
per (pinyin, committed text): a pair is listed only when the model objects to
most of its occurrences, so a single noisy sentence or a habit the model
merely dislikes on average (的/地) does not make the list.

Output is a review list for the owner, never an automatic change.
"""

import argparse
import json
import os
import sys
import time
from collections import defaultdict

from probe_rerank import EVENTS, Scorer, homophone_set, load_events


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--margin", type=float, default=3.0)
    ap.add_argument("--min-flagged", type=int, default=2)
    ap.add_argument("--min-ratio", type=float, default=0.6)
    ap.add_argument("--export", help="also write the versioned review file the manager reads, "
                    "e.g. ~/Library/Application Support/AgentInputMethod/Quality/model_audit/silent_errors.v1.json")
    args = ap.parse_args()

    scorer = Scorer(args.model)
    pairs = defaultdict(lambda: {"n": 0, "flagged": 0, "alts": defaultdict(int),
                                 "margins": [], "examples": []})
    events = load_events(EVENTS)
    for k, e in enumerate(events):
        s = homophone_set(e)
        if not s or s[0] != e["selectedText"]:
            continue
        ctx = e.get("precedingText", e["_recon"])
        scores = scorer.score(ctx, s)
        best = max(range(len(s)), key=lambda i: scores[i])
        p = pairs[(e["pinyin"], e["selectedText"])]
        p["n"] += 1
        if best != 0 and scores[best] - scores[0] > args.margin:
            p["flagged"] += 1
            p["alts"][s[best]] += 1
            p["margins"].append(scores[best] - scores[0])
            if len(p["examples"]) < 3:
                p["examples"].append(ctx[-16:])
        if k % 2000 == 0:
            print(f"{k}/{len(events)}", file=sys.stderr)

    rows = []
    for (py, text), p in pairs.items():
        if p["flagged"] >= args.min_flagged and p["flagged"] / p["n"] >= args.min_ratio:
            alt = max(p["alts"], key=p["alts"].get)
            rows.append({
                "pinyin": py, "committed": text, "suggested": alt,
                "occurrences": p["n"], "flagged": p["flagged"],
                "avg_margin": round(sum(p["margins"]) / len(p["margins"]), 2),
                "examples": p["examples"],
            })
    rows.sort(key=lambda r: (-r["flagged"], -r["avg_margin"]))
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "silent_errors.json"), "w", encoding="utf-8") as f:
        json.dump(rows, f, ensure_ascii=False, indent=2)
    if args.export:
        path = os.path.expanduser(args.export)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            json.dump({"schema": 1, "model": args.model, "generatedAtMs": int(time.time() * 1000),
                       "margin": args.margin, "pairs": rows}, f, ensure_ascii=False, indent=2)
    for r in rows:
        print(f'{r["pinyin"]:<14} {r["committed"]}→{r["suggested"]}  '
              f'{r["flagged"]}/{r["occurrences"]}  margin {r["avg_margin"]}  {r["examples"]}')
    print(f"listed {len(rows)} pairs", file=sys.stderr)


if __name__ == "__main__":
    main()
