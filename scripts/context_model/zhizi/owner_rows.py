"""The owner's own homophone choices as extra fine-tuning rows for the PUBLIC base model (owner decision 2026-10-15: his records may
train the published 顺手·知字; the friends' test logs may not).  Runs only on the Mac; the output stays under ime_context_model/ (never in Git).

Safeguards (docs/current/tasks/ROADMAP-2026-10-14.md "训练数据口径"):
  - only clean events: no redo / delete after the commit, not abandoned (the likeliest silent wrong characters);
  - any row whose context or candidates hold an e-mail, URL, phone number, long digit run, token-looking string or a file path is dropped;
  - events that are also cases of the evaluation set (same context and committed text) are dropped, so the owner set stays an honest test;
  - the last days (--holdout-from) never enter training: they are the held-out test of whether the rows help;
  - non-first picks (the owner deliberately chose something other than the first candidate) are repeated more than ordinary rows.

  python owner_rows.py --out DIR [--holdout-from 10-13] [--repeat 3] [--repeat-nonfirst 6]
Writes DIR/owner_rows.jsonl (rows already repeated), DIR/stats.json and DIR/sample.txt (60 random contexts for the owner to read).
"""
import argparse
import datetime
import json
import os
import random
import re
import sys
import types

for name in ("mlx", "mlx.core", "mlx_lm"):  # probe_rerank imports mlx at module level; not needed here
    sys.modules.setdefault(name, types.ModuleType(name))
sys.modules["mlx_lm"].load = None
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
from probe_rerank import EVENTS, homophone_set, load_events  # noqa: E402

QUALITY = os.path.expanduser("~/Library/Application Support/AgentInputMethod/Quality")
SENSITIVE = [
    re.compile(r"[\w.+-]+@[\w-]+\.[\w.]+"),                 # e-mail
    re.compile(r"https?://|www\.", re.I),                   # URL
    re.compile(r"1[3-9]\d{9}"),                             # mobile number
    re.compile(r"\d{8,}"),                                  # long digit run (card, id, phone-like)
    re.compile(r"[A-Za-z0-9_\-]{20,}"),                     # token-looking
    re.compile(r"/Users/|/Volumes/|[A-Za-z]:\\|~/"),        # file paths
    re.compile(r"sk-[A-Za-z0-9]|ghp_|AKIA[0-9A-Z]"),        # key prefixes
]


def day(e):
    return datetime.datetime.fromtimestamp(e["recordedAtMs"] / 1000).strftime("%m-%d")


def clean_ok(e):
    return not (e.get("redoAfterCommit") or e.get("deletedAfterCommit") or e.get("outcome") == "abandoned")


def sensitive(*texts):
    return any(p.search(t) for t in texts for p in SENSITIVE)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--events", default=EVENTS)
    ap.add_argument("--eval", default=os.path.join(QUALITY, "Eval", "owner-2026-10.jsonl"))
    ap.add_argument("--holdout-from", default="10-13", help="MM-DD; events from this day on are never trained on")
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--repeat-nonfirst", type=int, default=6)
    ap.add_argument("--seed", type=int, default=7)
    args = ap.parse_args()

    evaluated = set()
    for line in open(args.eval, encoding="utf-8"):
        r = json.loads(line)
        evaluated.add((r.get("context", ""), r.get("target", "")))
    rows, stats = [], {"events": 0, "clean": 0, "homophone": 0, "dropped_sensitive": 0, "dropped_eval": 0, "held_out": 0}
    for e in load_events(args.events):
        stats["events"] += 1
        if not clean_ok(e):
            continue
        stats["clean"] += 1
        s = homophone_set(e)
        if not s:
            continue
        stats["homophone"] += 1
        ctx = (e.get("precedingText") or e["_recon"])[-32:]
        if day(e) >= args.holdout_from and day(e) >= "10-00":
            stats["held_out"] += 1
            continue
        if (e.get("precedingText", ""), e["selectedText"]) in evaluated or (ctx, e["selectedText"]) in evaluated:
            stats["dropped_eval"] += 1
            continue
        if sensitive(ctx, *s):
            stats["dropped_sensitive"] += 1
            continue
        gold = s.index(e["selectedText"])
        rows.append({"ctx": ctx, "cands": s, "gold": gold, "src": "owner"})
    rng = random.Random(args.seed)
    seen, unique = set(), []
    for r in rows:
        key = (r["ctx"], tuple(r["cands"]), r["gold"])
        if key not in seen:
            seen.add(key)
            unique.append(r)
    nonfirst = sum(1 for r in unique if r["gold"] > 0)
    out = []
    for r in unique:
        out += [r] * (args.repeat_nonfirst if r["gold"] > 0 else args.repeat)
    rng.shuffle(out)
    stats.update({"unique_rows": len(unique), "unique_nonfirst": nonfirst, "rows_after_repeat": len(out)})
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "owner_rows.jsonl"), "w", encoding="utf-8") as f:
        for r in out:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    json.dump(stats, open(os.path.join(args.out, "stats.json"), "w"), ensure_ascii=False, indent=1)
    with open(os.path.join(args.out, "sample.txt"), "w", encoding="utf-8") as f:
        for r in rng.sample(unique, min(60, len(unique))):
            f.write(f"{r['ctx']!r:<50} -> {r['cands'][r['gold']]}  (候选: {' / '.join(r['cands'][:5])})\n")
    print(json.dumps(stats, ensure_ascii=False))


if __name__ == "__main__":
    main()
