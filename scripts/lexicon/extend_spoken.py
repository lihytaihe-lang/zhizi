"""Extend the shipped spoken-phrase layer with a new generation run, keeping every shipped row.

  python extend_spoken.py <new run phrases.tsv> <Rime resources dir> <out dir> [--extra-readings disagreements.tsv]

The new candidates are the run's phrases that came up in at least three independent calls (spoken_candidates.py rules: not already in a
dictionary, three or more characters). The shipped rows are read back from agent_spoken.provenance.tsv so their grade, call count and
backend count (hence weight) are unchanged; export_spoken.py then rebuilds the whole layer. Prints how many rows are new.
"""
import csv
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "../.."))


def main():
    args = sys.argv[1:]
    extra = None
    if "--extra-readings" in args:
        i = args.index("--extra-readings")
        extra = args[i + 1]
        del args[i:i + 2]
    run, res, out = args
    work = tempfile.mkdtemp(prefix="extend-spoken-")
    new_candidates = os.path.join(work, "new.tsv")
    subprocess.run([sys.executable, "-B", os.path.join(REPO, "scripts/context_model/spoken_candidates.py"), run, "-", res, new_candidates],
                   check=True)
    old = list(csv.DictReader(open(os.path.join(REPO, "quality/corpus/agent-spoken-provenance.tsv"), encoding="utf-8"), delimiter="\t"))
    shipped = {r["text"] for r in old}
    rows = [(r["text"], r["reading"], r["gen_calls"], r["backends"], "0", r["grade"], "0") for r in old]
    added = 0
    for r in csv.DictReader(open(new_candidates, encoding="utf-8"), delimiter="\t"):
        if r["grade"] == "strong" and len(r["phrase"]) >= 3 and r["phrase"] not in shipped:
            rows.append((r["phrase"], r["reading"], r["gen"], r["backends"], "0", "strong", r["poly"]))
            shipped.add(r["phrase"])
            added += 1
    combined = os.path.join(work, "combined.tsv")
    with open(combined, "w", encoding="utf-8") as fh:
        fh.write("phrase\treading\tgen\tbackends\tlccc\tgrade\tpoly\n")
        fh.write("".join("\t".join(r) + "\n" for r in rows))
    command = [sys.executable, "-B", os.path.join(HERE, "export_spoken.py"), combined, out]
    if extra:
        command += ["--extra-readings", extra]
    subprocess.run(command, check=True)
    print(f"new strong phrases: {added}; combined rows in: {len(rows)}; work dir {work}")


if __name__ == "__main__":
    main()
