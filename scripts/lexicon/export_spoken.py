"""Spoken-phrase layer `agent_spoken` (shared part of the phrase layer) from spoken_candidates.py output.

  python export_spoken.py <candidates.tsv> <out dir> [--part all|even|odd] [--extra-readings reading-disagreements.tsv]

The evidence is generation only (no outside corpus): grade `strong` = it came up in at least three independent generation calls; weight 400 from six calls by both models, else 250; never above the
existing first choice for the same full reading, nothing that is already shipped. --part even|odd takes every other
candidate; phrases shorter than three characters are left out (one half added, the other kept as held-out test phrases). Writes agent_spoken.dict.yaml + provenance.
"""
import argparse
import csv
import os
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_general_lexicon import existing_entries  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("candidates")
    ap.add_argument("out")
    ap.add_argument("--part", default="all", choices=["all", "even", "odd"])
    ap.add_argument("--weak-min-gen", type=int, default=99)
    ap.add_argument("--extra-readings", help="TSV phrase / our reading / second opinion (check_readings.py); a second reading that "
                    "is a valid syllable sequence of the same length is added as another entry for the same phrase")
    args = ap.parse_args()
    existing = existing_entries(exclude_dictionaries=("agent_spoken",))
    if not existing:
        raise RuntimeError("no shipped dictionaries found; run from the repository")
    top = {}
    for rows in existing.values():
        for reading, weight in rows:
            top[reading] = max(top.get(reading, 0), weight)
    extra = {}
    if args.extra_readings:
        # the real syllable inventory is that of single characters (other layers hold a few garbled readings: din, xin used to pass)
        syllables = {syllable for text, readings in existing.items() if len(text) == 1 for reading, _ in readings for syllable in reading.split()}
        for line in open(args.extra_readings, encoding="utf-8"):
            text, ours, theirs = line.rstrip("\n").split("\t")
            if len(theirs.split()) == len(text) and all(syllable in syllables for syllable in theirs.split()):
                extra.setdefault(text, []).append(theirs)
    picked = []
    for r in csv.DictReader(open(args.candidates, encoding="utf-8"), delimiter="\t"):
        if r["grade"] == "strong" or int(r["gen"]) >= args.weak_min_gen:
            picked.append(r)
    if args.part != "all":
        picked = [r for i, r in enumerate(picked) if (i % 2 == 0) == (args.part == "even")]
    stats, rows = Counter(), []
    for r in picked:
        t = r["phrase"]
        if len(t) < 3:  # two-character items are ordinary words; as extra homophones they push single characters off the page (拟好, 悬了)
            stats["two_character"] += 1
            continue
        if t in existing:
            stats["already_shipped"] += 1
            continue
        # Evidence is the number of independent generation calls (and that both models produced it).
        weight = 400 if int(r["gen"]) >= 6 and int(r["backends"]) >= 2 else 250
        if r["reading"] in top and weight >= top[r["reading"]]:
            weight = max(1, top[r["reading"]] - 1)
            stats["capped_below_first_choice"] += 1
        rows.append((t, r["reading"], weight, r["grade"], r["gen"], r["backends"]))
        for second in extra.get(t, []):
            stats["second_reading"] += 1
            capped = weight
            if second in top and capped >= top[second]:
                capped = max(1, top[second] - 1)
            rows.append((t, second, capped, r["grade"], r["gen"], r["backends"]))
    unique = {}
    for row in rows:  # one entry per (phrase, reading); the first (primary) wins
        unique.setdefault((row[0], row[1]), row)
    rows = list(unique.values())
    rows.sort(key=lambda x: (x[1], -x[2], x[0]))
    os.makedirs(args.out, exist_ok=True)
    header = ("# Agent Input Method spoken-phrase layer (generated; do not edit by hand)\n"
              "# Built by scripts/context_model/spoken_candidates.py + scripts/lexicon/export_spoken.py.\n"
              "# Weights never exceed the existing first choice for the same reading.\n"
              "---\nname: agent_spoken\nversion: \"0.1.0\"\nsort: by_weight\n...\n\n")
    with open(os.path.join(args.out, "agent_spoken.dict.yaml"), "w", encoding="utf-8") as fh:
        fh.write(header + "".join(f"{t}\t{rd}\t{w}\n" for t, rd, w, *_ in rows))
    with open(os.path.join(args.out, "agent_spoken.provenance.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\treading\tweight\tgrade\tgen_calls\tbackends\tsource\n")
        for t, rd, w, g, gn, bk in rows:
            fh.write(f"{t}\t{rd}\t{w}\t{g}\t{gn}\t{bk}\tCLI-generated\n")
    print({"rows": len(rows), **stats})


if __name__ == "__main__":
    main()
