"""Append owner-sourced words onto the shipped agent_newwords layer, in place.

Why: the shipped layer matches no export_new_words.py output any more (post-processing
such as apply_general_recount.py re-weighted it after export), so re-running the export
would lose those fixes. This tool appends only: every existing line stays byte for
byte, each accepted word is spliced in where the layer's order (reading, weight
descending, text) puts it, its provenance row is spliced into the same position
(validate_agent_daily_dictionary.mjs compares the two files row by row), and a second
run with the same input appends nothing.

The merge rules are owner_row() from export_new_words.py, unchanged; the layer being
appended to counts as published too (its words are skipped, its weights join the
first-choice cap). Prints the appended count and the new sha256 of both files, for
re-pinning scripts/validate_agent_daily_dictionary.mjs.
"""

import argparse
import csv
import hashlib
import json
import os
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_general_lexicon import existing_entries  # noqa: E402
from export_new_words import HAN_CHAR, OWNER_CONFIRMED_SOURCE, first_choice_top, owner_row, provenance_line, run_row  # noqa: E402
from check_contribution import SYLLABLES  # noqa: E402

DICT_NAME = "agent_newwords.dict.yaml"
# added under the existing header notes once owner-sourced words are appended
HEADER_NOTE = ("# Owner-sourced words are appended in place by scripts/lexicon/append_owner_words.py"
               " (same merge rules as the export).\n")


def sort_key(text, reading, weight):
    """The layer's entry order: reading, weight descending, text."""
    return (reading, -weight, text)


def line_key(line):
    """sort_key of an entry line; None for anything that is not a weighted entry."""
    if line.startswith("#") or not line.strip():
        return None
    cols = line.rstrip("\n").split("\t")
    if len(cols) < 3 or not cols[2].strip().lstrip("-").isdigit():
        return None
    return sort_key(*cols[:2], int(cols[2]))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--owner-sourced", metavar="TSV",
                    help="owner input records; columns text/reading/occurrences/review, as in export_new_words.py")
    ap.add_argument("--run-words", metavar="TSV",
                    help="gen_category_words.py new_words.tsv (text/reading/year/domain/wiki/synth); merged with the category-word rules")
    ap.add_argument("--rime", default=str(ROOT / "native/resources/Rime"))
    ap.add_argument("--provenance", default=str(ROOT / "quality/corpus/agent-newwords-provenance.tsv"))
    ap.add_argument("--owner-confirmed", action="store_true",
                    help="label the --owner-sourced rows as owner-confirmed colloquial words that the DeepSeek pass dropped")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    dict_path = os.path.join(args.rime, DICT_NAME)
    lines = open(dict_path, encoding="utf-8", newline="").readlines()
    try:
        marker = next(i for i, line in enumerate(lines) if line.startswith("..."))
    except StopIteration:
        raise RuntimeError(f"{dict_path} has no '...' header terminator") from None
    head, body = lines[:marker + 1], lines[marker + 1:]
    while body and not body[0].strip():  # the blank separator line belongs to the head
        head.append(body.pop(0))
    if any(line_key(line) is None for line in body):
        raise RuntimeError(f"unexpected non-entry line in the body of {dict_path}")

    prov_lines = open(args.provenance, encoding="utf-8", newline="").readlines()
    prov_head, prov_rows = prov_lines[:1], prov_lines[1:]
    cols = [line.rstrip("\n").split("\t") for line in body]
    if len(prov_rows) != len(cols) or any(c[:3] != p.split("\t")[:3] for c, p in zip(cols, prov_rows)):
        raise RuntimeError(f"{args.provenance} is not row-aligned with {dict_path}; refusing to append")

    # Everything already published, including this layer itself (its own words are the
    # idempotency guard, and its weights belong to the first-choice cap).
    existing = existing_entries(exclude_dictionaries=("agent_newwords",))
    for text, reading, weight in ((c[0], c[1], int(c[2])) for c in cols):
        existing.setdefault(text, []).append((reading, weight))
    if not existing:
        raise RuntimeError("no shipped dictionaries found; run from the repository")
    top = first_choice_top(existing)

    new_rows, stats, seen = [], Counter(), set()
    if not (args.owner_sourced or args.run_words):
        ap.error("give --owner-sourced or --run-words")
    if args.owner_sourced:
        with open(args.owner_sourced, encoding="utf-8") as fh:
            for number, r in enumerate(csv.DictReader(fh, delimiter="\t"), 2):  # header on line 1
                row, reason = owner_row(r["text"], r["reading"], r["occurrences"], seen, existing, top, stats)
                if row:
                    if args.owner_confirmed:
                        row = row[:7] + (OWNER_CONFIRMED_SOURCE,)
                    new_rows.append(row)
                elif reason:
                    print(f"{args.owner_sourced}:{number}: {reason}", file=sys.stderr)
    if args.run_words:
        with open(args.run_words, encoding="utf-8") as fh:
            for r in csv.DictReader(fh, delimiter="\t"):
                syl = r["reading"].split()
                if any(x not in SYLLABLES for x in syl) or len(syl) != len(HAN_CHAR.findall(r["text"])):
                    stats["run_bad_reading"] += 1
                    continue
                row = run_row(r, seen, existing, top, stats)
                if row:
                    new_rows.append(row)

    new_body, new_prov = list(body), list(prov_rows)
    if new_rows:
        if HEADER_NOTE not in head:
            head.insert(next(i for i, line in enumerate(head) if line.startswith("---")), HEADER_NOTE)
        for row in sorted(new_rows, key=lambda x: sort_key(x[0], x[1], x[2])):
            key = sort_key(row[0], row[1], row[2])
            keys = [line_key(line) for line in new_body]
            pos = next((i for i, k in enumerate(keys) if k > key), len(new_body))
            new_body.insert(pos, f"{row[0]}\t{row[1]}\t{row[2]}\n")
            new_prov.insert(pos, provenance_line(row))
    dict_text, prov_text = "".join(head + new_body), "".join(prov_head + new_prov)
    if new_rows and not args.dry_run:
        open(dict_path, "w", encoding="utf-8", newline="").write(dict_text)
        open(args.provenance, "w", encoding="utf-8", newline="").write(prov_text)
    print(json.dumps({"appended": len(new_rows), "dry_run": args.dry_run, "stats": stats,
                      "newwords_sha256": hashlib.sha256(dict_text.encode()).hexdigest(),
                      "newwords_provenance_sha256": hashlib.sha256(prov_text.encode()).hexdigest()},
                     ensure_ascii=False))


if __name__ == "__main__":
    main()
