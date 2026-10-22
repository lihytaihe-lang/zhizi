"""Mine colloquial multi-character phrases (还是什么 / 有问题 / 什么时候) that the IME's
dictionaries lack, from the colloquial corpora (synthetic chat + reviews + everyday sentences).

A phrase is a CJK n-gram (2..5) that is frequent and behaves like a unit: varied
characters on both sides (branching entropy) and not just a fragment of a longer
frequent n-gram. Readings come from pypinyin (phrase-aware). Output is a candidate
list for review, never written into the IME directly.

Then measures coverage on the owner's raw-pinyin fallbacks: events where no Chinese
candidate fit and the owner committed the pinyin itself (`haishenm`, `zenmkn`).
"""

import argparse
import glob
import json
import math
import os
import re
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

from pypinyin import Style, lazy_pinyin

RIME = os.path.expanduser("~/Library/Input Methods/Agent Input Method.app/Contents/Resources/Rime")
EVENTS = os.path.expanduser("~/Library/Application Support/AgentInputMethod/Quality/events.jsonl")
CJK_RUN = re.compile(r"[一-鿿]+")
ENGLISH = {"codex", "agent", "notion", "app", "kimi", "spec", "claude", "git", "github", "excel", "goal",
           "windows", "appstore", "deepseek", "api", "ok", "pr", "ui", "ai", "ide", "mac", "ios", "json",
           "cli", "mcp", "bug", "steam", "linux", "python", "gpt", "work", "skill", "pinyin"}


def dictionary_words():
    words = set()
    for path in glob.glob(os.path.join(RIME, "*.dict.yaml")):
        body = False
        for line in open(path, encoding="utf-8"):
            if line.startswith("..."):
                body = True
            elif body and not line.startswith("#"):
                words.add(line.split("\t", 1)[0])
    return words


def entropy(counter):
    total = sum(counter.values())
    return -sum(c / total * math.log(c / total) for c in counter.values()) if total else 0.0


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
    ap.add_argument("--corpus", default=str(WORK_DIR / 'corpus/v2'))
    ap.add_argument("--sources", default="synth,asap,tatoeba")
    ap.add_argument("--min-count", type=int, default=20)
    ap.add_argument("--min-entropy", type=float, default=1.0)
    ap.add_argument("--out", default=str(WORK_DIR / 'phrases'))
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    grams = Counter()
    left, right = defaultdict(Counter), defaultdict(Counter)
    for src in args.sources.split(","):
        for line in open(os.path.join(args.corpus, f"{src}.train.txt"), encoding="utf-8"):
            for run in CJK_RUN.findall(line):
                padded = "^" + run + "$"
                for n in range(2, 6):
                    for i in range(1, len(padded) - n):
                        g = padded[i:i + n]
                        grams[g] += 1
    frequent = {g: c for g, c in grams.items() if c >= args.min_count}
    del grams
    for src in args.sources.split(","):
        for line in open(os.path.join(args.corpus, f"{src}.train.txt"), encoding="utf-8"):
            for run in CJK_RUN.findall(line):
                padded = "^" + run + "$"
                for n in range(2, 6):
                    for i in range(1, len(padded) - n):
                        g = padded[i:i + n]
                        if g in frequent:
                            left[g][padded[i - 1]] += 1
                            right[g][padded[i + n]] += 1

    known = dictionary_words()
    rows = []
    for g, c in frequent.items():
        if g in known:
            continue
        le, re_ = entropy(left[g]), entropy(right[g])
        if min(le, re_) < args.min_entropy:
            continue
        # drop fragments: most occurrences sit inside one longer frequent n-gram
        longest_parent = max((frequent.get(p, 0) for p in
                              [g + ch for ch in right[g]] + [ch + g for ch in left[g]]), default=0)
        if longest_parent > 0.7 * c:
            continue
        rows.append({"phrase": g, "count": c, "reading": " ".join(lazy_pinyin(g, style=Style.NORMAL)),
                     "left_entropy": round(le, 2), "right_entropy": round(re_, 2)})
    rows.sort(key=lambda r: -r["count"])
    with open(os.path.join(args.out, "candidates.jsonl"), "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"frequent n-grams={len(frequent)} new phrase candidates={len(rows)}")
    print("top:", " ".join(r["phrase"] for r in rows[:80]))

    # coverage on the owner's raw-pinyin fallbacks
    fallbacks = []
    for line in open(EVENTS, encoding="utf-8"):
        e = json.loads(line)
        typed = e["pinyin"].lower()
        if (e.get("selectedText") or "").lower() == typed and re.fullmatch(r"[a-z]+", typed) and typed not in ENGLISH:
            fallbacks.append(typed)
    mined = [(r["phrase"], r["reading"].split()) for r in rows]
    covered = {}
    for typed in set(fallbacks):
        hits = [p for p, syl in mined if reading_matches(syl, typed)]
        if hits:
            covered[typed] = sorted(hits, key=lambda p: -frequent[p])[:3]
    n_cov = sum(1 for t in fallbacks if t in covered)
    print(f"owner raw-pinyin fallbacks={len(fallbacks)} covered by mined phrases={n_cov}")
    for t, hits in sorted(covered.items()):
        print(f"  {t:<22} -> {' / '.join(hits)}")
    json.dump({"fallbacks": len(fallbacks), "covered": n_cov, "examples": covered},
              open(os.path.join(args.out, "coverage.json"), "w"), ensure_ascii=False, indent=2)


if __name__ == "__main__":
    main()
