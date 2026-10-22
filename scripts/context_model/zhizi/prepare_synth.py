"""Fold finished synthesis batches (DONE marker only) into a corpus directory as
the `synth` source: synth.{train,valid}.{txt,bin}. Character ids come from the
corpus's existing vocab.json so the pretrained checkpoint stays compatible;
characters outside it map to <unk> and are counted."""

import argparse
import glob
import hashlib
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

import numpy as np


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--batches", default=str(WORK_DIR / 'synth/v1'))
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--backends", default="", help="comma list, e.g. deepseek: only fold files <backend>.jsonl (licence-clean release corpus)")
    ap.add_argument("--first-batch", default="", help="only batches whose name sorts >= this, e.g. batch-007")
    args = ap.parse_args()
    itos = json.load(open(os.path.join(args.corpus, "vocab.json"), encoding="utf-8"))
    stoi = {c: i for i, c in enumerate(itos)}
    lines, seen = [], set()
    done = sorted(d for d in glob.glob(os.path.join(args.batches, "batch-*")) if os.path.exists(os.path.join(d, "DONE"))
                  and os.path.basename(d) >= args.first_batch)
    only = [b for b in args.backends.split(",") if b]
    for d in done:
        for p in sorted(glob.glob(os.path.join(d, "*.jsonl"))):
            if only and os.path.basename(p).split(".")[0] not in only:
                continue
            for rec in map(json.loads, open(p, encoding="utf-8")):
                for x in rec["lines"]:
                    if x not in seen:
                        seen.add(x)
                        lines.append(x)
    out = {"train": [], "valid": []}
    for x in lines:
        h = hashlib.blake2b(x.encode(), digest_size=4).digest()
        out["valid" if h[0] < 3 else "train"].append(x)
    unk = 0
    for split, rows in out.items():
        with open(os.path.join(args.corpus, f"synth.{split}.txt"), "w", encoding="utf-8") as f:
            f.write("\n".join(rows) + "\n")
        ids = []
        for x in rows:
            ids.append(1)
            for c in x:
                i = stoi.get(c, 2)
                unk += i == 2
                ids.append(i)
        np.array(ids, dtype=np.uint16).tofile(os.path.join(args.corpus, f"synth.{split}.bin"))
    manifest_path = os.path.join(args.corpus, "manifest.json")
    manifest = json.load(open(manifest_path, encoding="utf-8"))
    manifest["synth"] = {"license": ("generated (deepseek-flash API only; terms 4.2(3) allow training other models)" if only == ["deepseek"]
                                     else "generated (Qwen3.8-Flash via QoderWork, GLM-5.3-flash via opencode, deepseek-flash API)"),
                         "register": "colloquial/work-chat/cn-en", "batches": [os.path.basename(d) for d in done],
                         "train_lines": len(out["train"]), "valid_lines": len(out["valid"]),
                         "chars": sum(len(x) for x in lines), "unk": unk}
    json.dump(manifest, open(manifest_path, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
    print(json.dumps(manifest["synth"], ensure_ascii=False))


if __name__ == "__main__":
    main()
