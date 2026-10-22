"""Offline estimate for "model first" ranking (C2): let 顺手·知字 rank every visible
candidate that covers the whole typed input (whole-sentence compositions included),
not only same-length same-reading homophones as in C1.

Truth = what the owner committed (records keep the displayed top 5 and the choice).
For each event: candidates whose reading consumes the full typed pinyin (full syllables
or syllable prefixes, the owner's abbreviation style) form the group; the model scores
each with the recorded preceding text; order = score - beta * original_rank. A "fix" is
an event where the owner did not take the first candidate and the new first equals the
owner's choice; a "break" is one where the owner took the first and the new first differs.
beta is chosen on days before 07-22 and reported on 07-22 and later (held out).
Records stay on this machine; output is counts and short examples.
"""

import json
import os
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, str(ROOT / 'scripts/context_model/zhizi'))
from eval_owner import ZhiziScorer  # noqa: E402
from pypinyin import Style, pinyin  # noqa: E402

EVENTS = os.path.expanduser("~/Library/Application Support/AgentInputMethod/Quality/events.jsonl")
CKPT = str(WORK_DIR / 'runs/gpu-v2-30m/runs/v2-30m-homo/best.pt')
HAN = re.compile(r"^[一-鿿]+$")
SPLIT_MS = 1784649600000  # 2026-07-22 00:00 +08:00


def readings(ch):
    return {p.replace("ü", "v") for p in pinyin(ch, style=Style.NORMAL, heteronym=True)[0]}


def covers(text, typed):
    """Each character contributes a non-empty prefix of one of its readings, in order,
    consuming `typed` exactly."""
    pos = {0}
    for ch in text:
        nxt = set()
        for p in pos:
            for r in readings(ch):
                for n in range(1, len(r) + 1):
                    if typed.startswith(r[:n], p):
                        nxt.add(p + n)
        pos = nxt
        if not pos:
            return False
    return len(typed) in pos


def main():
    scorer = ZhiziScorer(CKPT, "mps" if __import__("torch").backends.mps.is_available() else "cpu")
    rows = []
    for line in open(EVENTS, encoding="utf-8"):
        e = json.loads(line)
        typed = (e.get("pinyin") or "").lower()
        cands = e.get("candidates") or []
        chosen = e.get("selectedText")
        if e.get("outcome") == "abandoned" or not chosen or not re.fullmatch(r"[a-z]+", typed):
            continue
        group = [i for i, c in enumerate(cands[:5]) if HAN.match(c) and covers(c, typed)]
        if len(group) < 2 or 0 not in group:
            continue
        texts = [cands[i] for i in group]
        scores = scorer.score(e.get("precedingText") or "", texts)
        rows.append({"t": e.get("recordedAtMs", 0), "typed": typed, "texts": texts, "ranks": group,
                     "scores": scores, "chosen": chosen, "len": len(texts[0])})
    print(f"events with a full-coverage group of 2+: {len(rows)}", flush=True)

    def evaluate(subset, beta):
        fix = brk = 0
        ex_fix, ex_brk = [], []
        for r in subset:
            order = sorted(range(len(r["texts"])), key=lambda k: -(r["scores"][k] - beta * r["ranks"][k]))
            new_first = r["texts"][order[0]]
            old_first = r["texts"][0]
            if new_first == old_first:
                continue
            if r["chosen"] == new_first:
                fix += 1
                ex_fix.append((r["typed"], old_first, new_first))
            elif r["chosen"] == old_first:
                brk += 1
                ex_brk.append((r["typed"], old_first, new_first))
        return fix, brk, ex_fix, ex_brk

    train = [r for r in rows if r["t"] < SPLIT_MS]
    held = [r for r in rows if r["t"] >= SPLIT_MS]
    print(f"train={len(train)} held_out={len(held)}")
    best = None
    for beta in (0, 0.5, 1, 1.5, 2, 3, 4, 6):
        f, b, _, _ = evaluate(train, beta)
        print(f"train beta={beta}: fixes={f} breaks={b} net={f - b}")
        if best is None or f - b > best[1]:
            best = (beta, f - b)
    beta = best[0]
    f, b, ef, eb = evaluate(held, beta)
    owner_nonfirst = sum(1 for r in held if r["chosen"] != r["texts"][0])
    print(f"HELD-OUT beta={beta}: fixes={f} breaks={b} net={f - b} "
          f"(owner non-first in these groups: {owner_nonfirst}, events {len(held)})")
    by_len = Counter()
    for typed, old, new in ef:
        by_len["fix len " + str(min(len(new), 5))] += 1
    for typed, old, new in eb:
        by_len["break len " + str(min(len(old), 5))] += 1
    print(dict(sorted(by_len.items())))
    print("fix examples:", ef[:20])
    print("break examples:", eb[:20])


if __name__ == "__main__":
    main()
