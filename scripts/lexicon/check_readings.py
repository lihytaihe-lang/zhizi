#!/usr/bin/env python3
"""Readings that no character of the word can have: shipped dictionary rows whose pinyin does not fit the hanzi.

    scripts/lexicon/check_readings.py --rime <Resources/Rime> --out <dir> [--glob 'agent_*.dict.yaml']

A row `word<TAB>py py py<TAB>weight` is flagged when its syllable count differs from the character count, or when a
syllable is not among the readings pypinyin knows for that character (heteronym mode, plus the word's own phrase
reading).  pypinyin lacks some rare readings, so a flag is a lead to review, not a proven error.
Output: out/flagged.tsv (file, line, word, reading, expected-by-phrase, bad positions, weight) and out/summary.json.
Words only; nothing private is read.  Run with a Python that has pypinyin (.build-deps/pypinyin/0.55.0/venv).
"""
import argparse
import collections
import glob
import json
import os
import re

from pypinyin import Style, lazy_pinyin, pinyin

HAN = re.compile(r"^[一-鿿]+$")
_CHAR_CACHE = {}


def char_readings(ch):
    if ch not in _CHAR_CACHE:
        _CHAR_CACHE[ch] = {r for group in pinyin(ch, style=Style.NORMAL, heteronym=True) for r in group}
    return _CHAR_CACHE[ch]


def norm(syllable):
    return syllable.replace("ü", "v").replace("u:", "v")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rime", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--glob", default="agent_*.dict.yaml")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    flagged, per_file = [], collections.Counter()
    rows_seen = collections.Counter()
    for path in sorted(glob.glob(os.path.join(args.rime, args.glob))):
        name = os.path.basename(path)
        body = False
        for number, line in enumerate(open(path, encoding="utf-8", errors="ignore"), 1):
            if line.startswith("..."):
                body = True
                continue
            if not body or line.startswith("#"):
                continue
            parts = line.rstrip("\n").split("\t")
            if len(parts) < 2 or not HAN.match(parts[0]):
                continue
            word, reading = parts[0], parts[1].split()
            rows_seen[name] += 1
            weight = parts[2] if len(parts) > 2 else ""
            expected = lazy_pinyin(word, style=Style.NORMAL, v_to_u=False)
            bad = []
            if len(reading) != len(word):
                bad = ["count"]
            else:
                for i, (ch, syl) in enumerate(zip(word, reading)):
                    syl = norm(syl)
                    if syl not in {norm(r) for r in char_readings(ch)}:
                        bad.append(f"{i}:{ch}")
            if bad:
                per_file[name] += 1
                flagged.append((name, number, word, " ".join(reading), " ".join(expected), ",".join(bad), weight))
    with open(os.path.join(args.out, "flagged.tsv"), "w", encoding="utf-8") as handle:
        for row in flagged:
            handle.write("\t".join(str(x) for x in row) + "\n")
    summary = {"rows": dict(rows_seen), "flagged": dict(per_file), "flagged_total": len(flagged)}
    json.dump(summary, open(os.path.join(args.out, "summary.json"), "w"), ensure_ascii=False, indent=1)
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
