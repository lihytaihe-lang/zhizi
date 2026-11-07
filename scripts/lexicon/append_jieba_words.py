"""Append DeepSeek-judged jieba words onto the shipped agent_newwords layer, in place.

Input: the approved.tsv of scripts/lexicon/judge_jieba_missing.py (word, jieba frequency,
tag).  Readings come from pypinyin 0.55 phrase readings (MIT) and must pass the pinyin
syllable inventory and a syllable-count check.  Merge rules are those of export_new_words.py:
spelled-out numbers dropped, anything already shipped skipped, weight 250 (no corpus
evidence), never above the existing first choice for the same reading.  Splicing, row
alignment with the provenance file and idempotency are as in append_owner_words.py.

    scripts/lexicon/append_jieba_words.py --approved <approved.tsv> --pypinyin <dir> [--dry-run]
"""
import argparse
import hashlib
import json
import os
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from append_owner_words import DICT_NAME, line_key, sort_key  # noqa: E402
from build_general_lexicon import existing_entries  # noqa: E402
from check_contribution import SYLLABLES  # noqa: E402
from export_new_words import HAN_CHAR, NUMERALS, first_choice_top  # noqa: E402

SOURCE = "jieba 0.42.1 dict.txt (MIT), DeepSeek-judged, 2-pass"
LICENSE = "MIT"
HEADER_NOTE = ("# jieba-sourced words are appended in place by scripts/lexicon/append_jieba_words.py"
               " (weight 60, below every existing first choice).\n")
WEIGHT = 60


def provenance_row(text, reading, weight, freq):
    return f"{text}\t{reading}\t{weight}\tjieba\t{freq}\t0\t0\t{SOURCE}\t{LICENSE}\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--approved", required=True)
    ap.add_argument("--pypinyin", required=True, help="directory holding the pypinyin package (.build-deps/pypinyin/<ver>)")
    ap.add_argument("--rime", default=str(ROOT / "native/resources/Rime"))
    ap.add_argument("--provenance", default=str(ROOT / "quality/corpus/agent-newwords-provenance.tsv"))
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    sys.path.insert(0, args.pypinyin)
    from pypinyin import Style, lazy_pinyin  # noqa: E402

    dict_path = os.path.join(args.rime, DICT_NAME)
    lines = open(dict_path, encoding="utf-8", newline="").readlines()
    marker = next(i for i, line in enumerate(lines) if line.startswith("..."))
    head, body = lines[:marker + 1], lines[marker + 1:]
    while body and not body[0].strip():
        head.append(body.pop(0))
    if any(line_key(line) is None for line in body):
        raise RuntimeError(f"unexpected non-entry line in the body of {dict_path}")
    prov_lines = open(args.provenance, encoding="utf-8", newline="").readlines()
    prov_head, prov_rows = prov_lines[:1], prov_lines[1:]
    cols = [line.rstrip("\n").split("\t") for line in body]
    if len(prov_rows) != len(cols) or any(c[:3] != p.split("\t")[:3] for c, p in zip(cols, prov_rows)):
        raise RuntimeError("provenance is not row-aligned with the dictionary; refusing to append")

    existing = existing_entries(exclude_dictionaries=("agent_newwords",))
    for text, reading, weight in ((c[0], c[1], int(c[2])) for c in cols):
        existing.setdefault(text, []).append((reading, weight))
    top = first_choice_top(existing)

    stats, new_rows, seen = Counter(), [], set()
    for line in open(args.approved, encoding="utf-8"):
        text, freq, _tag = line.rstrip("\n").split("\t")[:3]
        if text in seen:
            continue
        seen.add(text)
        if NUMERALS.search(text):
            stats["spelled_out_numbers"] += 1
        elif text in existing:
            stats["already_shipped"] += 1
        else:
            syllables = lazy_pinyin(text, style=Style.NORMAL, errors="ignore")
            if len(syllables) != len(HAN_CHAR.findall(text)) or any(s not in SYLLABLES for s in syllables):
                stats["reading_rejected"] += 1
                continue
            reading = " ".join(syllables)
            weight = WEIGHT
            if reading in top and weight >= top[reading]:
                weight = max(1, top[reading] - 1)
                stats["capped_below_first_choice"] += 1
            new_rows.append((text, reading, weight, freq))
    new_body, new_prov = list(body), list(prov_rows)
    if new_rows:
        if HEADER_NOTE not in head:
            head.insert(next(i for i, line in enumerate(head) if line.startswith("---")), HEADER_NOTE)
        # one sorted merge instead of an insert per row (12k rows)
        merged = [(line_key(l), l, p) for l, p in zip(new_body, new_prov)]
        merged += [(sort_key(t, r, w), f"{t}\t{r}\t{w}\n", provenance_row(t, r, w, f)) for t, r, w, f in new_rows]
        merged.sort(key=lambda item: item[0])
        new_body, new_prov = [m[1] for m in merged], [m[2] for m in merged]
    dict_text, prov_text = "".join(head + new_body), "".join(prov_head + new_prov)
    if new_rows and not args.dry_run:
        open(dict_path, "w", encoding="utf-8", newline="").write(dict_text)
        open(args.provenance, "w", encoding="utf-8", newline="").write(prov_text)
    print(json.dumps({"appended": len(new_rows), "dry_run": args.dry_run, "stats": stats,
                      "rows_after": len(new_body),
                      "newwords_sha256": hashlib.sha256(dict_text.encode()).hexdigest(),
                      "newwords_provenance_sha256": hashlib.sha256(prov_text.encode()).hexdigest()}, ensure_ascii=False))


if __name__ == "__main__":
    main()
