# -*- coding: utf-8 -*-
"""Local lexicon-gap discovery from the owner's own quality records (nothing leaves this machine, nothing is added automatically).

  ime_context_model/.venv/bin/python scripts/lexicon/find_gaps.py [--events <events.jsonl>] [--out <dir>] [--min 2]

Signals (each a place where the lexicon probably let the user down):
  raw      the user committed the letters themselves (committed_raw): no candidate was usable  -> missing word
  deep     a candidate at index >= DEEP on the first page was picked (word is there but ranked far down) -> weight too low
  abandon  composition abandoned at >= 5 letters -> unknown (only the letters are recorded, never text)
For each (letters, picked text) the output says how often it happened and whether the text is in the shipped dictionaries with that reading.
Output: <out>/gaps.tsv (kind, letters, text, count, in_lexicon) - private, for human review, never committed.
Records that contain mail / URL / long digit strings are skipped.
"""
import argparse
import json
import os
import re
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_general_lexicon import existing_entries  # noqa: E402

DEEP = 3
EVENTS = os.path.expanduser("~/Library/Application Support/AgentInputMethod/Quality/events.jsonl")
SENSITIVE = re.compile(r"@|https?:|www\.|\d{6,}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--events", default=EVENTS)
    ap.add_argument("--out", default="eval-runs/gaps")
    ap.add_argument("--min", type=int, default=2)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    known = existing_entries()
    counts = Counter()
    for line in open(args.events, encoding="utf-8"):
        try:
            e = json.loads(line)
        except ValueError:
            continue
        letters = re.sub(r"[^a-z]", "", (e.get("pinyin") or e.get("peakPinyin") or "").lower())
        text = e.get("committedText") or e.get("selectedText") or ""
        if SENSITIVE.search(letters + text):
            continue
        outcome = e.get("outcome")
        if outcome == "committed_raw" and len(letters) >= 4:
            counts[("raw", letters, letters)] += 1
        elif outcome == "committed_candidate" and (e.get("selectedIndex") or 0) >= DEEP and len(text) >= 2 and letters:
            counts[("deep", letters, text)] += 1
        elif outcome == "abandoned" and len(letters) >= 5:
            counts[("abandon", letters, "")] += 1
    rows = sorted(((k, c) for k, c in counts.items() if c >= args.min), key=lambda kc: (kc[0][0], -kc[1]))
    summary = Counter()
    with open(os.path.join(args.out, "gaps.tsv"), "w", encoding="utf-8") as fh:
        fh.write("kind\tletters\ttext\tcount\tin_lexicon\n")
        for (kind, letters, text), c in rows:
            in_lex = int(text in known) if kind == "deep" else ""
            fh.write(f"{kind}\t{letters}\t{text}\t{c}\t{in_lex}\n")
            summary[kind] += 1
    print({k: v for k, v in summary.items()}, f"(each seen at least {args.min} times) -> {args.out}/gaps.tsv")


if __name__ == "__main__":
    main()
