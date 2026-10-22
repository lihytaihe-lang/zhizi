"""Merge generated spoken phrases with LCCC fragment counts into dictionary candidates (phrase layer, shared part).

  python spoken_candidates.py <spoken phrases.tsv> <lccc counts.tsv> <Rime resources dir> <out.tsv>

Evidence per phrase: `gen` = independent generation calls that produced it, `lccc` = occurrences in the LCCC sample.
A phrase is `strong` when LCCC has it (>= 8) and it was generated twice or LCCC has it >= 40 times; `weak` when only
generated, or only seen in LCCC a few times. Phrases already dictionary entries are dropped. Readings come from
pypinyin; a phrase containing a heteronym character is flagged (`poly=1`) so the exporter can leave it out or have it
reviewed. Nothing here touches the shipped dictionaries.
"""
import glob
import os
import sys

from pypinyin import Style, lazy_pinyin, pinyin


def dictionary(res):
    words = set()
    for path in glob.glob(os.path.join(res, "*.dict.yaml")):
        if os.path.basename(path) == "agent_spoken.dict.yaml":
            continue  # this layer is what is being rebuilt
        body = open(path, encoding="utf-8").read().split("\n...\n", 1)
        if len(body) < 2:
            continue
        for line in body[1].splitlines():
            parts = line.split("\t")
            if len(parts) >= 2 and parts[0] and not parts[0].startswith("#"):
                words.add(parts[0])
    return words


def main():
    spoken, lccc, res, out = sys.argv[1:5]
    gen = {}
    for line in open(spoken, encoding="utf-8"):
        s, n, backends = line.rstrip("\n").split("\t")
        gen[s] = (int(n), len(backends.split(",")))
    seen = {}
    if lccc != "-":  # "-": no outside corpus at all (the shipped layer is built this way)
        for line in open(lccc, encoding="utf-8"):
            s, n, _ = line.rstrip("\n").split("\t")
            seen[s] = int(n)
    known = dictionary(res)
    rows = []
    for s, (n, nb) in gen.items():
        if s in known:
            continue
        c = seen.get(s, 0)
        if lccc == "-":
            # Independent generations only: it came up at least three times (calls are independent draws).
            grade = "strong" if n >= 3 else "weak"
        else:
            grade = "strong" if c >= 8 and (n >= 2 or c >= 40) else "weak"
        poly = int(any(len(alts) > 1 for alts in pinyin(s, style=Style.NORMAL, heteronym=True)))
        rows.append((s, " ".join(lazy_pinyin(s)), n, nb, c, grade, poly))
    rows.sort(key=lambda r: (r[5] != "strong", -r[4], -r[2]))
    with open(out, "w", encoding="utf-8") as fh:
        fh.write("phrase\treading\tgen\tbackends\tlccc\tgrade\tpoly\n")
        for r in rows:
            fh.write("\t".join(map(str, r)) + "\n")
    strong = [r for r in rows if r[5] == "strong"]
    print(f"candidates (not in dictionary): {len(rows)}; strong: {len(strong)}; strong without heteronym: {sum(1 for r in strong if not r[6])}")


if __name__ == "__main__":
    main()
