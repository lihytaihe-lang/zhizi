"""Offline benefit/cost of a proposed lexicon layer on the owner's local typing records.

Benefit
  pieces:    the owner committed a word in consecutive pieces (现在 + 这个) within a few
             seconds in the same app, and the joined text is a proposed new word.
  fallbacks: the owner committed raw pinyin (no fitting candidate) and a proposed word's
             reading covers that pinyin (full or per-syllable prefix).
Cost
  displaced: the owner took the first candidate T for a full-pinyin input, and a proposed
             word W != T with the same full reading would get a higher weight than T's
             shipped weight, so W would take the first slot.

Records never leave the machine; this prints counts and short examples only.
"""

import argparse
import json
import os
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, os.path.dirname(__file__))
from build_general_lexicon import existing_entries  # noqa: E402

EVENTS = os.path.expanduser("~/Library/Application Support/AgentInputMethod/Quality/events.jsonl")
HAN = re.compile(r"^[一-鿿]+$")


def reading_matches(syllables, typed):
    """Each syllable contributes a non-empty prefix, in order, covering `typed` exactly."""
    pos = {0}
    for syl in syllables:
        nxt = set()
        for p in pos:
            for n in range(1, len(syl) + 1):
                if typed.startswith(syl[:n], p):
                    nxt.add(p + n)
        pos = nxt
        if not pos:
            return False
    return len(typed) in pos


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lexicon", default=str(WORK_DIR / 'lexicon/v1/lexicon.tsv'))
    ap.add_argument("--gap-ms", type=int, default=6000)
    args = ap.parse_args()

    new = {}
    for line in list(open(args.lexicon, encoding="utf-8"))[1:]:
        text, reading, weight = line.split("\t")[:3]
        new[text] = (reading, int(weight))
    by_full = defaultdict(list)
    for text, (reading, weight) in new.items():
        by_full[reading.replace(" ", "")].append((text, weight))
    existing = existing_entries()

    events = [json.loads(line) for line in open(EVENTS, encoding="utf-8")]
    events.sort(key=lambda e: e.get("recordedAtMs", 0))
    committed = [e for e in events if e.get("selectedText") and e.get("outcome") != "abandoned"]

    # pieces: 2-3 consecutive commits in one app whose join is a proposed word
    pieces = Counter()
    for i in range(len(committed)):
        joined = committed[i]["selectedText"]
        for j in range(i + 1, min(i + 3, len(committed))):
            a, b = committed[j - 1], committed[j]
            if b.get("appHash") != a.get("appHash") or b["recordedAtMs"] - a["recordedAtMs"] > args.gap_ms:
                break
            joined += b["selectedText"]
            if HAN.match(joined) and joined in new:
                pieces[joined] += 1

    # fallbacks: raw pinyin committed
    fallbacks, covered = [], Counter()
    syl = {t: r.split() for t, (r, _) in new.items()}
    for e in committed:
        typed = (e.get("pinyin") or "").lower()
        if (e["selectedText"] or "").lower() == typed and re.fullmatch(r"[a-z]{2,}", typed):
            fallbacks.append(typed)
    fallback_set = set(fallbacks)
    hits = {}
    for typed in fallback_set:
        h = [t for t, s in syl.items() if len(s) <= len(typed) and reading_matches(s, typed)]
        if h:
            hits[typed] = sorted(h, key=lambda t: -new[t][1])[:3]
    n_fallback_cov = sum(1 for t in fallbacks if t in hits)

    # displaced: first-choice commits of a full-pinyin input
    displaced, checked = Counter(), 0
    for e in committed:
        if e.get("selectedIndex", 0) != 0:
            continue
        t, typed = e["selectedText"], (e.get("pinyin") or "").lower()
        if not HAN.match(t) or len(t) < 2 or typed not in by_full:
            continue
        rows = [w for r, w in existing.get(t, []) if r.replace(" ", "") == typed]
        if not rows:
            continue
        checked += 1
        t_weight = max(rows)
        for w_text, w_weight in by_full[typed]:
            if w_text != t and w_weight > t_weight:
                displaced[(typed, t, w_text)] += 1

    result = {
        "proposed_words": len(new),
        "pieces_events": sum(pieces.values()), "pieces_distinct": len(pieces),
        "pieces_examples": pieces.most_common(25),
        "fallbacks": len(fallbacks), "fallbacks_covered": n_fallback_cov,
        "fallback_examples": dict(list(hits.items())[:25]),
        "first_choice_full_pinyin_checked": checked,
        "displaced_events": sum(displaced.values()), "displaced_distinct": len(displaced),
        "displaced_examples": [f"{k[0]}: {k[1]} -> {k[2]} x{v}" for k, v in displaced.most_common(25)],
    }
    print(json.dumps(result, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
