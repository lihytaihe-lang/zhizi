"""Step-0 probe: can a pretrained LM, given the preceding text, pick the right
homophone among the owner's real candidate lists?

No training. Measures the ceiling a dedicated context model could reach on the
owner's 12252 real commits (docs/current/decisions/2026-10-08-owner-context-model-route.md).

Protocol (true homophone set):
  S = CJK-only candidates with the committed text's length and reading (pypinyin).
  Only events with the committed text in S and |S| >= 2 count.
  IME baseline = first member of S in shown order; model = argmax log P(c | context).
  A margin gate (model overrides the IME only when it beats the IME's pick by > tau nats)
  is swept so fixes and breaks are reported together, never gains alone.

Contexts:
  none   - no preceding text
  real   - the precedingText the IME read from the app (<= 8 UTF-16 units), when recorded
  recon  - previous commits in the same app within 120 s, last 32 chars
"""

import argparse
import json
import os
import re
import sys
import time
from collections import defaultdict

import mlx.core as mx
from mlx_lm import load
from pypinyin import Style, pinyin

CJK = re.compile(r"^[㐀-鿿]+$")
EVENTS = os.path.expanduser(
    "~/Library/Application Support/AgentInputMethod/Quality/events.jsonl"
)
TAUS = [0.0, 0.5, 1.0, 2.0, 3.0, 5.0]


def load_events(path):
    events = [json.loads(line) for line in open(path, encoding="utf-8")]
    events.sort(key=lambda e: e["recordedAtMs"])
    last = {}  # appHash -> (time, text)
    for e in events:
        prev = last.get(e["appHash"])
        if prev and e["recordedAtMs"] - prev[0] <= 120_000:
            text = prev[1]
        else:
            text = ""
        e["_recon"] = text[-32:]
        last[e["appHash"]] = (e["recordedAtMs"], text + (e.get("selectedText") or ""))
    return events


def char_readings(word):
    """Toneless readings per character, heteronyms included."""
    return [set(r) for r in pinyin(word, style=Style.NORMAL, heteronym=True)]


def same_reading(a, b):
    """True when every position shares at least one reading. Fuzzy-pinyin,
    typo-correction and abbreviation candidates the IME mixes in fail this,
    so the set holds true homophones only - a reranker must never trade the
    typed reading for a different one."""
    ra, rb = char_readings(a), char_readings(b)
    return len(ra) == len(rb) and all(x & y for x, y in zip(ra, rb))


def homophone_set(e):
    gold = e.get("selectedText") or ""
    if not CJK.match(gold):
        return None
    seen, s = set(), []
    for c in e["candidates"]:
        if CJK.match(c) and len(c) == len(gold) and c not in seen and same_reading(c, gold):
            seen.add(c)
            s.append(c)
    if gold not in s or len(s) < 2:
        return None
    return s


class Scorer:
    def __init__(self, model_id):
        self.model, self.tok = load(model_id)
        self.prefix = self.tok.encode("\n", add_special_tokens=False)

    def score(self, context, cands):
        """Sum of log P(candidate tokens | prefix + context), one batch per event."""
        ctx = self.prefix + self.tok.encode(context, add_special_tokens=False)
        seqs = [ctx + self.tok.encode(c, add_special_tokens=False) for c in cands]
        width = max(len(s) for s in seqs)
        pad = self.tok.pad_token_id or 0
        batch = mx.array([s + [pad] * (width - len(s)) for s in seqs])
        logits = self.model(batch[:, :-1]).astype(mx.float32)
        logp = logits - mx.logsumexp(logits, axis=-1, keepdims=True)
        targets = batch[:, 1:]
        picked = mx.take_along_axis(logp, targets[..., None], axis=-1)[..., 0]
        rows = []
        for i, s in enumerate(seqs):
            row = [0.0] * (width - 1)
            for j in range(len(ctx) - 1, len(s) - 1):
                row[j] = 1.0
            rows.append(row)
        mask = mx.array(rows)
        out = (picked * mask).sum(axis=1)
        mx.eval(out)
        return out.tolist()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    events = load_events(EVENTS)
    cases = [(e, s) for e in events if (s := homophone_set(e))]
    if args.limit:
        cases = cases[: args.limit]
    scorer = Scorer(args.model)

    modes = ["none", "real", "recon"]
    tally = {m: defaultdict(lambda: [0, 0, 0]) for m in modes}  # tau -> [n, fixes, breaks]
    real_n = 0
    examples = []
    dump = []  # every real-context case with all scores, for offline policy sweeps
    started = time.time()
    for k, (e, s) in enumerate(cases):
        gold = e["selectedText"]
        ime = s[0]
        contexts = {"none": "", "recon": e["_recon"]}
        if "precedingText" in e:
            contexts["real"] = e["precedingText"]
            real_n += 1
        for mode, ctx in contexts.items():
            scores = scorer.score(ctx, s)
            best = max(range(len(s)), key=lambda i: scores[i])
            margin = scores[best] - scores[0]
            for tau in TAUS:
                pick = s[best] if best != 0 and margin > tau else ime
                t = tally[mode][tau]
                t[0] += 1
                if ime != gold and pick == gold:
                    t[1] += 1
                elif ime == gold and pick != gold:
                    t[2] += 1
            if mode == "real":
                dump.append({"context": ctx, "pinyin": e["pinyin"], "set": s,
                             "gold": gold, "scores": [round(x, 3) for x in scores]})
            if mode == "real" and best != 0 and len(examples) < 300:
                examples.append({
                    "context": ctx, "pinyin": e["pinyin"], "set": s,
                    "gold": gold, "model": s[best], "margin": round(margin, 2),
                })
        if k % 500 == 0:
            print(f"{k}/{len(cases)} {time.time() - started:.0f}s", file=sys.stderr)

    ime_wrong = sum(1 for e, s in cases if s[0] != e["selectedText"])
    ime_wrong_real = sum(
        1 for e, s in cases if "precedingText" in e and s[0] != e["selectedText"]
    )
    summary = {
        "model": args.model,
        "events": len(events),
        "homophone_cases": len(cases),
        "homophone_cases_with_real_context": real_n,
        "ime_wrong": ime_wrong,
        "ime_wrong_with_real_context": ime_wrong_real,
        "seconds": round(time.time() - started, 1),
        "by_mode": {
            m: {str(t): dict(zip(["n", "fixes", "breaks"], v)) for t, v in tally[m].items()}
            for m in modes
        },
    }
    os.makedirs(args.out, exist_ok=True)
    with open(os.path.join(args.out, "summary.json"), "w", encoding="utf-8") as f:
        json.dump(summary, f, ensure_ascii=False, indent=2)
    with open(os.path.join(args.out, "examples.jsonl"), "w", encoding="utf-8") as f:
        for x in examples:
            f.write(json.dumps(x, ensure_ascii=False) + "\n")
    with open(os.path.join(args.out, "real_scores.jsonl"), "w", encoding="utf-8") as f:
        for x in dump:
            f.write(json.dumps(x, ensure_ascii=False) + "\n")
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
