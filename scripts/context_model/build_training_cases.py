"""Typing-simulation cases from the TRAINING splits of the 顺手·知字 corpus (MODEL-V3-PLAN, step S1).

The same five ways of typing as the external test set (scripts/eval/build_external_set.py, whose
helpers are reused) but drawn from `*.train.txt`, so no sentence here appears in the test set
(that one is cut from `*.valid.txt`). Output is `eval`-harness cases:
  {"id","bucket","source","context","keys","target"}
Run them through native/host/examples/eval (no model needed) to get the candidates the core
produces; `make_rerank_set.py` turns that into "context + candidates -> gold" training rows.

No owner data is read: the corpus is public / synthetic text only.

Usage: python build_training_cases.py --corpus DIR --words dict.txt --out cases.jsonl
           [--count 20000] [--mix wikipedia:0.2,un:0.05,asap:0.25,tatoeba:0.15,synth:0.35] [--seed N]
"""
import argparse
import json
import random
import sys
from pathlib import Path
import os

ROOT = Path(__file__).resolve().parents[2]
EVAL_OUT = Path(os.environ.get("ZHIZI_EVAL_OUT", ROOT / "eval-runs"))

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "eval"))
from build_external_set import HAN, Segmenter, initial, pieces, slip, syllables  # noqa: E402

BUCKETS = (("full", 0.25), ("abbrev", 0.15), ("mixed", 0.25), ("noisy", 0.25), ("short", 0.10))


def sample_lines(path, want, rng, min_len=6, max_len=80):
    """About `want` usable lines of a (possibly very large) file, spread over the whole file."""
    size = path.stat().st_size
    out = []
    with open(path, "rb") as fh:
        while len(out) < want:
            fh.seek(rng.randrange(max(1, size - 4096)))
            fh.readline()  # land on a line start
            line = fh.readline().decode("utf-8", "ignore").strip()
            if min_len <= len(line) <= max_len * 4 and HAN.search(line):
                out.append(line)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--words", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--count", type=int, default=20000)
    ap.add_argument("--mix", default="wikipedia:0.2,un:0.05,asap:0.25,tatoeba:0.15,synth:0.35")
    ap.add_argument("--seed", type=int, default=20261012)
    ap.add_argument("--only-abbrev", action="store_true",
                    help="top-up pass: only initials-only cases, over one or two words of 2-5 characters (hd, cwfx)")
    ap.add_argument("--external", default=str(EVAL_OUT / 'external.jsonl'),
                    help="cases whose text must not appear here (belt and braces; the splits are disjoint)")
    args = ap.parse_args()
    from pypinyin import Style, lazy_pinyin

    rng = random.Random(args.seed)
    jieba = Segmenter(args.words)
    mix = {k: float(v) for k, v in (item.split(":") for item in args.mix.split(","))}
    banned = set()
    if Path(args.external).exists():
        for line in open(args.external, encoding="utf-8"):
            case = json.loads(line)
            banned.add(case["target"] + "|" + case["context"][-8:])

    # Each sentence yields a few pieces; ask for twice what is needed and stop at `count`.
    sources = []
    for name, share in mix.items():
        path = Path(args.corpus) / f"{name}.train.txt"
        if path.exists():
            sources += [(name, line) for line in sample_lines(path, int(args.count * share * 0.9) + 200, rng)]
    rng.shuffle(sources)

    quota = {b: int(args.count * share) for b, share in BUCKETS}
    quota["full"] += args.count - sum(quota.values())
    if args.only_abbrev:
        quota = {b: 0 for b in quota}
        quota["abbrev"] = args.count
    made = {b: 0 for b in quota}
    # Each source keeps to its share of the cases: a long Wikipedia sentence yields many
    # pieces and would otherwise crowd the colloquial sources out.
    total_share = sum(mix.values())
    source_quota = {name: int(args.count * share / total_share) + 1 for name, share in mix.items()}
    source_made = {name: 0 for name in mix}
    seen = set()
    rows = []
    for source, sentence in sources:
        if all(made[b] >= quota[b] for b in quota):
            break
        if source_made[source] >= source_quota[source]:
            continue
        for context, text, words in pieces(sentence, jieba, rng):
            if source_made[source] >= source_quota[source]:
                break
            sy = syllables(text, lazy_pinyin, Style)
            if len(sy) != len(text) or not all(s.isalpha() and s.isascii() for s in sy):
                continue
            full = "".join(sy)
            choice = rng.random()
            case = None
            if args.only_abbrev:
                if made["abbrev"] < quota["abbrev"] and len(words) <= 2 and 2 <= len(text) <= 5:
                    case = ("abbrev", "".join(initial(x) for x in sy))
                if case is None:
                    continue
                choice = 2.0  # skip the ordinary buckets below
            if choice < 0.25 and made["full"] < quota["full"]:
                case = ("full", full)
            elif choice < 0.4 and made["abbrev"] < quota["abbrev"] and len(words) == 1 and 2 <= len(text) <= 4:
                case = ("abbrev", "".join(initial(s) for s in sy))
            elif choice < 0.65 and made["mixed"] < quota["mixed"] and len(text) >= 3:
                if len(words) >= 2 and rng.random() < 0.35:
                    last = len(words[-1])
                    keys = "".join(sy[:-last]) + "".join(initial(s) for s in sy[-last:])
                else:
                    keys = "".join(sy[:-1]) + initial(sy[-1])
                case = ("mixed", keys)
            elif choice < 0.9 and made["noisy"] < quota["noisy"] and len(full) >= 4:
                keys = slip(full, rng)
                if keys:
                    case = ("noisy", keys)
            if case is None and made["short"] < quota["short"] and len(words[-1]) <= 2 and context:
                tail = words[-1]
                tail_sy = syllables(tail, lazy_pinyin, Style)
                if len(tail_sy) == len(tail) and HAN.match(tail):
                    ctx = context + text[: len(text) - len(tail)]
                    case = ("short", "".join(tail_sy))
                    text, context = tail, ctx
            if case is None:
                continue
            bucket, keys = case
            key = (bucket, keys, text, context[-8:])
            if key in seen or (text + "|" + context[-8:]) in banned:
                continue
            seen.add(key)
            made[bucket] += 1
            source_made[source] += 1
            rows.append({"id": f"tr-{len(rows):06d}", "bucket": bucket, "source": source,
                         "context": context[-32:], "keys": keys, "target": text})
    with open(args.out, "w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False) + "\n")
    by_source = {}
    for row in rows:
        by_source[row["source"]] = by_source.get(row["source"], 0) + 1
    print(json.dumps({"cases": len(rows), "buckets": made, "sources": by_source}, ensure_ascii=False))


if __name__ == "__main__":
    main()
