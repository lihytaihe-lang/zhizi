"""Build a candidate general-vocabulary layer for the IME from permissive word lists,
weighted by how often each word actually occurs in our licensed corpora.

Why: the base dictionary (rime-pinyin-simp, AOSP-derived) has 65k entries and predates
much of everyday modern vocabulary; owners hit "no candidate" or compose words from
pieces. This script proposes new words only. It never edits the shipped dictionaries;
its output is reviewed and then integrated by a task card with gates.

Sources (word lists, not text):
  - jieba 0.42.1 dict.txt (MIT): ~350k words with POS tags
  - pypinyin 0.55 phrases_dict (MIT): phrase-level readings for polyphones
  - our mined/generated colloquial phrase layer (phrase_layer_v1.jsonl)
Frequencies come from our own corpora (colloquial: asap, tatoeba, synthetic chat;
written: a Wikipedia/UN sample), so weights reflect use, not the list's own counts.

Outputs in --out:
  lexicon.tsv       text, reading, weight, colloq, written, source, flags
  collisions.tsv    new entries whose weight would put them above an existing
                    first choice with the same full reading (displacements)
  summary.json
"""

import argparse
import glob
import json
import math
import os
import re
from collections import Counter, defaultdict
from multiprocessing import Pool
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

from pypinyin import Style, lazy_pinyin, pinyin_dict
from pypinyin.phrases_dict import phrases_dict

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WORK = str(WORK_DIR)
HAN = re.compile(r"^[一-鿿]{2,6}$")
HAN_RUN = re.compile(r"[一-鿿]+")
# jieba POS tags for proper nouns: person, place, organization, other proper noun, transliterated name
PROPER = {"nr", "nrt", "nrfg", "ns", "nt", "nz"}


def existing_entries(exclude_dictionaries=()):
    """word -> list of (reading, weight) across every shipped dictionary."""
    paths = glob.glob(os.path.join(REPO, "native/resources/Rime/*.dict.yaml"))
    paths += glob.glob(os.path.join(REPO, ".build-deps/rime-pinyin-simp/*/pinyin_simp.dict.yaml"))
    entries = defaultdict(list)
    for path in paths:
        if os.path.basename(path).removesuffix(".dict.yaml") in exclude_dictionaries:
            continue
        body = False
        for line in open(path, encoding="utf-8"):
            if line.startswith("..."):
                body = True
                continue
            if not body or line.startswith("#") or not line.strip():
                continue
            cols = line.rstrip("\n").split("\t")
            if len(cols) >= 2:
                weight = int(cols[2]) if len(cols) >= 3 and cols[2].strip().lstrip("-").isdigit() else 0
                entries[cols[0]].append((cols[1], weight))
    return entries


def candidate_words(jieba_dict):
    words = {}
    for line in open(jieba_dict, encoding="utf-8"):
        parts = line.split()
        if len(parts) >= 2 and HAN.match(parts[0]):
            words[parts[0]] = {"source": "jieba", "pos": parts[2] if len(parts) > 2 else "", "list_freq": int(parts[1])}
    for w in phrases_dict:
        if HAN.match(w) and w not in words:
            words[w] = {"source": "pypinyin-phrases", "pos": "", "list_freq": 0}
    layer = os.path.join(WORK, "phrases/phrase_layer_v1.jsonl")
    for line in open(layer, encoding="utf-8"):
        row = json.loads(line)
        w = row["phrase"]
        if HAN.match(w):
            words.setdefault(w, {"source": f"phrase-layer-{row.get('src', '')}", "pos": "", "list_freq": 0})
    return words


def corpus_files():
    v2 = os.path.join(WORK, "corpus/v2")
    colloq = [(os.path.join(v2, f), None) for f in ("asap.train.txt", "tatoeba.train.txt")]
    colloq += [(p, "jsonl") for p in sorted(glob.glob(os.path.join(WORK, "synth/v1/batch-*/*.jsonl")))]
    written = [(os.path.join(v2, "wikipedia.train.txt"), 80_000_000), (os.path.join(v2, "un.train.txt"), 20_000_000)]
    return colloq, written


_WORDS = None
_COST = None
_UNK = 30.0


def _init(words, cost=None):
    global _WORDS, _COST
    _WORDS, _COST = words, cost


def _texts(path, mode):
    if mode == "jsonl":
        for line in open(path, encoding="utf-8"):
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            for text in row.get("lines", []):
                if isinstance(text, str):
                    yield text
    else:
        budget = mode if isinstance(mode, int) else None
        seen = 0
        for line in open(path, encoding="utf-8"):
            yield line
            seen += len(line)
            if budget and seen >= budget:
                break


def _count(job):
    """Pass 1: raw substring counts (lengths 1..6) for every vocabulary word."""
    path, mode = job
    counts, chars = Counter(), 0
    words = _WORDS
    for text in _texts(path, mode):
        for run in HAN_RUN.findall(text):
            chars += len(run)
            n = len(run)
            for i in range(n):
                for k in range(1, min(6, n - i) + 1):
                    s = run[i:i + k]
                    if s in words:
                        counts[s] += 1
    return counts, chars


def _segment(job):
    """Pass 2: count a word only where it is a token of the best unigram segmentation,
    so fragments of longer words (入法 inside 输入法) stop collecting their parent's count."""
    path, mode = job
    counts, chars = Counter(), 0
    cost = _COST
    for text in _texts(path, mode):
        for run in HAN_RUN.findall(text):
            n = len(run)
            chars += n
            best = [0.0] + [float("inf")] * n
            back = [0] * (n + 1)
            for i in range(n):
                bi = best[i]
                if bi == float("inf"):
                    continue
                for k in range(1, min(6, n - i) + 1):
                    c = cost.get(run[i:i + k])
                    if c is None:
                        if k > 1:
                            continue
                        c = _UNK
                    if bi + c < best[i + k]:
                        best[i + k] = bi + c
                        back[i + k] = i
            j = n
            while j > 0:
                i = back[j]
                if j - i > 1:
                    counts[run[i:j]] += 1
                j = i
    return counts, chars


def split_jobs(files):
    return list(files)


def count_corpus(words, files, procs, cost=None):
    total, chars = Counter(), 0
    fn = _segment if cost is not None else _count
    with Pool(procs, initializer=_init, initargs=(words, cost)) as pool:
        for c, n in pool.imap_unordered(fn, split_jobs(files)):
            total.update(c)
            chars += n
    return total, chars


def reading(word):
    syl = lazy_pinyin(word, style=Style.NORMAL, errors="ignore")
    syl = [s.replace("ü", "v").replace("lve", "lue").replace("nve", "nue") for s in syl]
    return " ".join(syl) if len(syl) == len(word) else None


def polyphone_risk(word):
    """True when the word has a multi-reading character and no phrase-level reading."""
    if word in phrases_dict:
        return False
    return any(len(pinyin_dict.pinyin_dict.get(ord(ch), "").split(",")) > 1 for ch in word)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jieba-dict", default=os.path.join(WORK, "lexicon/src/jieba-0.42.1/jieba-0.42.1/jieba/dict.txt"))
    ap.add_argument("--out", default=os.path.join(WORK, "lexicon/v1"))
    ap.add_argument("--procs", type=int, default=10)
    ap.add_argument("--min-per-10m", type=float, default=2.0, help="min combined occurrences per 10M chars")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    existing = existing_entries()
    words = candidate_words(args.jieba_dict)
    new = {w: m for w, m in words.items() if w not in existing}
    print(f"existing words={len(existing)} candidates={len(words)} not-yet-shipped={len(new)}", flush=True)

    # count both the new words and existing words (existing counts calibrate the weight scale)
    vocab = set(new) | {w for w in existing if HAN.match(w)} | {w for w in existing if len(w) == 1}
    colloq_files, written_files = corpus_files()
    raw, raw_chars = count_corpus(vocab, colloq_files + written_files, args.procs)
    print(f"pass1 substring counts over {raw_chars} chars", flush=True)
    cost = {w: -math.log((c + 0.5) / raw_chars) for w, c in raw.items() if c > 0}
    # a multi-char token must beat spelling it out char by char, else fragments still win
    colloq, colloq_chars = count_corpus(None, colloq_files, args.procs, cost)
    print(f"colloquial chars={colloq_chars}", flush=True)
    written, written_chars = count_corpus(None, written_files, args.procs, cost)
    print(f"written chars={written_chars}", flush=True)

    def per10m(w):
        # colloquial counts double: the IME is mostly used for chat and work messages
        return 1e7 * (2 * colloq[w] / max(colloq_chars, 1) + written[w] / max(written_chars, 1)) / 3

    # weight scale: fit log(base weight) ~ a + b*log(per10m) on existing pinyin_simp-scale words seen in corpus
    xs, ys = [], []
    for w, rows in existing.items():
        top = max(wt for _, wt in rows)
        f = per10m(w) if HAN.match(w) else 0
        if top > 0 and f > 0:
            xs.append(math.log(f))
            ys.append(math.log(top))
    mx, my = sum(xs) / len(xs), sum(ys) / len(ys)
    b = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sum((x - mx) ** 2 for x in xs)
    a = my - b * mx
    print(f"weight fit on {len(xs)} existing words: log w = {a:.3f} + {b:.3f} log f", flush=True)

    # existing first choice per full reading
    top_by_reading = {}
    for w, rows in existing.items():
        for r, wt in rows:
            if (r not in top_by_reading) or wt > top_by_reading[r][1]:
                top_by_reading[r] = (w, wt)

    out_rows, collisions, stats = [], [], Counter()
    for w, meta in new.items():
        f = per10m(w)
        if f < args.min_per_10m:
            stats["too_rare"] += 1
            continue
        if meta["pos"] in PROPER and f < 20 * args.min_per_10m:
            stats["proper_noun_rare"] += 1
            continue
        r = reading(w)
        if not r:
            stats["no_reading"] += 1
            continue
        flags = []
        if polyphone_risk(w):
            flags.append("polyphone")
        weight = max(1, int(math.exp(a + b * math.log(f))))
        top = top_by_reading.get(r)
        if top and weight >= top[1]:
            top_f = per10m(top[0]) if HAN.match(top[0]) else 0
            if f >= 3 * max(top_f, 1e-9):
                flags.append("displaces")
                collisions.append((w, r, weight, f, top[0], top[1], top_f))
            else:
                weight = max(1, top[1] - 1)
                flags.append("capped")
        stats["kept"] += 1
        out_rows.append((w, r, weight, colloq[w], written[w], meta["source"], ",".join(flags)))

    out_rows.sort(key=lambda x: -x[2])
    with open(os.path.join(args.out, "lexicon.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\treading\tweight\tcolloq\twritten\tsource\tflags\n")
        for row in out_rows:
            fh.write("\t".join(map(str, row)) + "\n")
    with open(os.path.join(args.out, "collisions.tsv"), "w", encoding="utf-8") as fh:
        fh.write("new\treading\tnew_weight\tnew_per10m\texisting_top\texisting_weight\texisting_per10m\n")
        for row in sorted(collisions, key=lambda x: -x[3]):
            fh.write("\t".join(f"{v:.2f}" if isinstance(v, float) else str(v) for v in row) + "\n")
    summary = {"existing": len(existing), "candidates": len(words), "new_candidates": len(new),
               "colloquial_chars": colloq_chars, "written_chars": written_chars,
               "weight_fit": {"a": a, "b": b, "n": len(xs)}, "stats": stats,
               "flags": Counter(f for row in out_rows for f in row[6].split(",") if f),
               "by_length": Counter(len(row[0]) for row in out_rows),
               "by_source": Counter(row[5] for row in out_rows)}
    json.dump(summary, open(os.path.join(args.out, "summary.json"), "w"), ensure_ascii=False, indent=1)
    print(json.dumps(summary, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
