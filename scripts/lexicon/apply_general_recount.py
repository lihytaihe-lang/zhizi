# -*- coding: utf-8 -*-
"""Write the clean-corpus recount (recount_general_tokens.py --corpus clean) into agent_general and its provenance table.

  python3 scripts/lexicon/apply_general_recount.py --recount <recount-clean.tsv> --reproduce <recount-old-max20.tsv> [--dry-run]

Per row of quality/corpus/agent-general-provenance.tsv:
  * owner-missing-curated rows keep their fixed weight and their zero token counts;
  * rows whose stored weight the old-corpus formula did not reproduce keep the stored weight (reason unverified, see
    docs/current/licenses/GLM-QWEN-CLEANUP-2026-10-21.md) but get the clean token counts;
  * every other row gets the clean-corpus formula weight and the clean token counts.
The dictionary keeps its header and its order (reading, weight descending, text).  --dry-run writes nothing and prints what would change;
with an unchanged recount it must reproduce both files byte for byte (checked by --selfcheck).
"""
import argparse
import csv
import os
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DICT = os.path.join(ROOT, "native/resources/Rime/agent_general.dict.yaml")
PROVENANCE = os.path.join(ROOT, "quality/corpus/agent-general-provenance.tsv")


def read_table(path):
    return list(csv.DictReader(open(path, encoding="utf-8"), delimiter="\t"))


def render(rows, header_lines):
    body = "".join(f"{r['text']}\t{r['reading']}\t{r['weight']}\n" for r in sorted(rows, key=lambda r: (r["reading"], -int(r["weight"]), r["text"])))
    return "".join(header_lines) + body


def render_provenance(rows, fieldnames):
    # same order as the dictionary: scripts/validate_agent_daily_dictionary.mjs compares the two files row by row
    out = ["\t".join(fieldnames) + "\n"]
    for r in sorted(rows, key=lambda r: (r["reading"], -int(r["weight"]), r["text"])):
        out.append("\t".join(r[f] for f in fieldnames) + "\n")
    return "".join(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--recount", required=True)
    ap.add_argument("--reproduce", required=True, help="recount over the old corpus (batches 001-020), to find the rows the formula did not reproduce")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--selfcheck", action="store_true", help="re-render the files from their own content and compare byte for byte")
    args = ap.parse_args()

    dict_text = open(DICT, encoding="utf-8").read()
    marker = "...\n\n"
    cut = dict_text.index(marker) + len(marker)
    header_lines = [dict_text[:cut]]
    prov_text = open(PROVENANCE, encoding="utf-8").read()
    fieldnames = prov_text.split("\n", 1)[0].split("\t")
    prov = read_table(PROVENANCE)
    for r in prov:
        r["reading"] = r["reading"].strip()

    if args.selfcheck:
        again = render(prov, header_lines)
        ok_dict = again == dict_text
        ok_prov = render_provenance(read_table(PROVENANCE), fieldnames) == prov_text
        print(f"selfcheck dictionary byte-identical: {ok_dict}; provenance byte-identical: {ok_prov}")
        sys.exit(0 if ok_dict and ok_prov else 1)

    new = {r["text"]: r for r in read_table(args.recount)}
    old = {r["text"]: r for r in read_table(args.reproduce)}
    changed_weight = kept = fixed = 0
    for r in prov:
        w = r["text"]
        if r["source"] == "owner-missing-curated":
            fixed += 1
            continue
        n = new[w]
        reproduced = old[w]["formula_weight"] != "" and int(old[w]["formula_weight"]) == int(r["weight"])
        if not reproduced or n["formula_weight"] == "":
            kept += 1   # weight and token counts stay together; see the cleanup record for why these rows are open
            continue
        r["colloquial_tokens"] = n["new_colloquial"]
        r["written_tokens"] = n["new_written"]
        if int(n["formula_weight"]) != int(r["weight"]):
            changed_weight += 1
        r["weight"] = n["formula_weight"]
    print(f"rows={len(prov)} weight changed={changed_weight} kept (not reproduced)={kept} fixed (owner-missing)={fixed}")
    if args.dry_run:
        return
    open(DICT, "w", encoding="utf-8").write(render(prov, header_lines))
    open(PROVENANCE, "w", encoding="utf-8").write(render_provenance(prov, fieldnames))
    print("written")


if __name__ == "__main__":
    main()
