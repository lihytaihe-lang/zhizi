"""Build the pretraining corpus for the context model from licensed JD P07 sources.

Only the normalized public-source layer (01_normalized) is read; paid label /
delivery layers are never touched. Every output line keeps its source id so
the license manifest can be regenerated, and an open-weights decision can
drop a source (e.g. CC BY-SA) without re-running everything.

Output (under --out):
  <source>.train.txt / <source>.valid.txt   one paragraph per line (<= 512 chars)
  vocab.json                                 character vocabulary + special tokens
  manifest.json                              per-source license, files, chars
"""

import argparse
import glob
import hashlib
import json
import os
import re
from collections import Counter
from multiprocessing import Pool
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

NORMALIZED_ROOT = str(ROOT.parent / 'JD_P07_DATA/01_normalized')
SOURCES = {
    # source id: (glob, field, license, register)
    "wikipedia": ("classification/zh/P07-WIKIMEDIA/**/*.jsonl", "text", "CC BY-SA 4.0", "written"),
    "un": ("translation/en-zh/P07-UN/**/*.jsonl", "target_text", "UN Parallel Corpus terms", "formal"),
    "asap": ("sentiment/zh/P07-ASAP/**/*.jsonl", "text", "Apache-2.0", "colloquial"),
    "tatoeba": ("translation/en-zh/P07-TATOEBA/**/*.jsonl", "target_text", "CC BY 2.0 FR", "everyday"),
}
SPECIALS = ["<pad>", "<bos>", "<unk>", "<chat>", "<doc>", "<code>", "<other>"]
CJK = re.compile(r"[㐀-鿿]")
SPLIT = re.compile(r"(?<=[。！？；!?;\n])")
WS = re.compile(r"[ \t　]+")
MAX_LEN = 512


def chunks(text):
    text = WS.sub(" ", text.replace("\r", "")).strip()
    buf = ""
    for piece in SPLIT.split(text):
        piece = piece.strip()
        if not piece:
            continue
        if len(buf) + len(piece) > MAX_LEN and buf:
            yield buf
            buf = ""
        buf += piece[:MAX_LEN]
    if buf:
        yield buf


def keep(line):
    if len(line) < 4:
        return False
    visible = [c for c in line if not c.isspace()]
    return sum(1 for c in visible if CJK.match(c)) >= 0.3 * len(visible)


def process(job):
    path, field = job
    out, chars = [], Counter()
    with open(path, encoding="utf-8") as f:
        for raw in f:
            text = json.loads(raw)["content"].get(field) or ""
            for line in chunks(text.replace("\n", "。")):
                if keep(line):
                    out.append(line)
                    chars.update(line)
    return out, chars


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--min-char-count", type=int, default=30)
    ap.add_argument("--valid-per-mille", type=int, default=5)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    total_chars, manifest = Counter(), {}
    seen = set()
    for sid, (pattern, field, license_name, register) in SOURCES.items():
        files = sorted(glob.glob(os.path.join(NORMALIZED_ROOT, pattern), recursive=True))
        n_train = n_valid = n_chars = 0
        with open(os.path.join(args.out, f"{sid}.train.txt"), "w", encoding="utf-8") as tr, \
             open(os.path.join(args.out, f"{sid}.valid.txt"), "w", encoding="utf-8") as va, \
             Pool(min(10, max(1, len(files)))) as pool:
            for lines, chars in pool.imap_unordered(process, [(p, field) for p in files]):
                total_chars.update(chars)
                for line in lines:
                    h = hashlib.blake2b(line.encode(), digest_size=8).digest()
                    if h in seen:
                        continue
                    seen.add(h)
                    n_chars += len(line)
                    if h[0] * 1000 // 256 < args.valid_per_mille:
                        va.write(line + "\n")
                        n_valid += 1
                    else:
                        tr.write(line + "\n")
                        n_train += 1
        manifest[sid] = {"license": license_name, "register": register, "files": len(files),
                         "train_lines": n_train, "valid_lines": n_valid, "chars": n_chars}
        print(sid, manifest[sid], flush=True)

    vocab = SPECIALS + sorted(
        c for c, n in total_chars.items()
        if n >= args.min_char_count and not c.isspace() or (c.isascii() and c.isprintable())
    )
    covered = sum(n for c, n in total_chars.items() if c in set(vocab))
    with open(os.path.join(args.out, "vocab.json"), "w", encoding="utf-8") as f:
        json.dump(vocab, f, ensure_ascii=False)
    manifest["_vocab"] = {"size": len(vocab), "coverage": round(covered / sum(total_chars.values()), 6)}
    with open(os.path.join(args.out, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)
    print(manifest["_vocab"])


if __name__ == "__main__":
    main()
