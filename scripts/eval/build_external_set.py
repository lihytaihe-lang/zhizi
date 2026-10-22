"""Bucketed IME test set from public text the context model never trained on.

Reads the validation splits of the 顺手·知字 corpus (synth: work chat and everyday
messages; tatoeba: everyday sentences), cuts them into the pieces people type in one
composition (2-8 characters along word boundaries, the text before it as context), and
writes the keys for five ways of typing:

  full     full pinyin                          你看看这个 -> nikankanzhege
  abbrev   initials of a 2-4 character word     没问题 -> mwt
  mixed    full pinyin, the last syllable or word reduced to initials
                                                再发给朋友 -> zaifageipy, 没问题 -> meiwent
  noisy    full pinyin with one slip, drawn from the Sogou log distribution
           (omission 47%, insertion 28%, substitution 19%, transposition 7.5%;
           omitted letters favour i/g/e/u/h; substitutions take a QWERTY neighbour)
  short    1-2 characters with context (homophones such as 在/再, 他/它)
  erhua    pieces ending in 儿, typed in full (放这儿 -> fangzheer); a separate pass
           with its own random stream, so the buckets above stay as they were

Usage: python build_external_set.py --corpus DIR --words jieba/dict.txt --out cases.jsonl [--per-bucket 300]
Deterministic for a given seed.
"""
import argparse
import json
import math
import random
import re
from pathlib import Path

HAN = re.compile(r"^[一-鿿]+$")
SPLIT = re.compile(r"[，。！？；：、,.!?;:\s“”\"'（）()《》【】…—~～\-]+")
ROWS = ["qwertyuiop", "asdfghjkl", "zxcvbnm"]


def neighbours(letter):
    for row in ROWS:
        i = row.find(letter)
        if i >= 0:
            return [row[j] for j in (i - 1, i + 1) if 0 <= j < len(row)]
    return []


def syllables(text, lazy_pinyin, Style):
    return lazy_pinyin(text, style=Style.NORMAL, errors="ignore", v_to_u=False, neutral_tone_with_five=False)


def initial(syllable):
    return syllable[:1]


def slip(keys, rng):
    """One typing slip; None when the result would equal the input."""
    kind = rng.choices(["omit", "insert", "substitute", "transpose"], [47, 28, 19, 7.5])[0]
    if kind == "omit":
        favoured = [i for i, c in enumerate(keys) if c in "ighue"]
        i = rng.choice(favoured) if favoured and rng.random() < 0.7 else rng.randrange(len(keys))
        out = keys[:i] + keys[i + 1:]
    elif kind == "insert":
        i = rng.randrange(len(keys))
        extra = keys[i] if rng.random() < 0.5 else (rng.choice(neighbours(keys[i]) or [keys[i]]))
        out = keys[:i + 1] + extra + keys[i + 1:]
    elif kind == "substitute":
        i = rng.randrange(len(keys))
        options = neighbours(keys[i])
        if not options:
            return None
        out = keys[:i] + rng.choice(options) + keys[i + 1:]
    else:
        if len(keys) < 2:
            return None
        i = rng.randrange(len(keys) - 1)
        if keys[i] == keys[i + 1]:
            return None
        out = keys[:i] + keys[i + 1] + keys[i] + keys[i + 1 + 1:]
    return out if out and out != keys else None


class Segmenter:
    """Most probable split by word frequency (jieba's dictionary, no HMM)."""

    def __init__(self, path):
        self.freq = {}
        for line in Path(path).read_text(encoding="utf-8").splitlines():
            parts = line.split()
            if len(parts) >= 2 and parts[1].isdigit():
                self.freq[parts[0]] = int(parts[1])
        self.total = math.log(sum(self.freq.values()))
        self.longest = max(map(len, self.freq))

    def cut(self, text, HMM=False):
        n = len(text)
        best = [(0.0, 0)] * (n + 1)
        for i in range(n - 1, -1, -1):
            options = []
            for j in range(i + 1, min(n, i + self.longest) + 1):
                word = text[i:j]
                if word in self.freq or j == i + 1:
                    options.append((math.log(self.freq.get(word, 1)) - self.total + best[j][0], j))
            best[i] = max(options)
        out, i = [], 0
        while i < n:
            j = best[i][1]
            out.append(text[i:j])
            i = j
        return out


def pieces(sentence, jieba, rng):
    """(context, piece) pairs: words grouped into 2-8 character pieces."""
    out = []
    consumed = ""
    for clause in SPLIT.split(sentence):
        if not clause:
            continue
        start = sentence.find(clause, len(consumed))
        words = list(jieba.cut(clause, HMM=False))
        i = 0
        offset = start
        while i < len(words):
            size = rng.choice([1, 1, 2, 2, 3, 4])
            group = words[i:i + size]
            text = "".join(group)
            if HAN.match(text) and 2 <= len(text) <= 8:
                out.append((sentence[:offset], text, group))
            offset += len(text)
            i += size
        consumed = sentence[: start + len(clause)]
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--words", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--per-bucket", type=int, default=300)
    ap.add_argument("--seed", type=int, default=20261010)
    args = ap.parse_args()
    from pypinyin import Style, lazy_pinyin  # noqa: E402

    jieba = Segmenter(args.words)
    rng = random.Random(args.seed)
    sources = []
    for name in ("synth", "tatoeba"):
        for line in (Path(args.corpus) / f"{name}.valid.txt").read_text(encoding="utf-8").splitlines():
            if line.strip():
                sources.append((name, line.strip()))
    rng.shuffle(sources)

    buckets = {b: [] for b in ("full", "abbrev", "mixed", "noisy", "short")}
    seen = set()
    for source, sentence in sources:
        if all(len(v) >= args.per_bucket for v in buckets.values()):
            break
        for context, text, words in pieces(sentence, jieba, rng):
            sy = syllables(text, lazy_pinyin, Style)
            if len(sy) != len(text) or not all(s.isalpha() and s.isascii() for s in sy):
                continue
            full = "".join(sy)
            choice = rng.random()
            case = None
            if choice < 0.25 and len(buckets["full"]) < args.per_bucket:
                case = ("full", full)
            elif choice < 0.4 and len(buckets["abbrev"]) < args.per_bucket and len(words) == 1 and 2 <= len(text) <= 4:
                case = ("abbrev", "".join(initial(s) for s in sy))
            elif choice < 0.65 and len(buckets["mixed"]) < args.per_bucket and len(text) >= 3:
                if len(words) >= 2 and rng.random() < 0.35:
                    last = len(words[-1])
                    keys = "".join(sy[:-last]) + "".join(initial(s) for s in sy[-last:])
                else:
                    keys = "".join(sy[:-1]) + initial(sy[-1])
                case = ("mixed", keys)
            elif choice < 0.9 and len(buckets["noisy"]) < args.per_bucket and len(full) >= 4:
                keys = slip(full, rng)
                if keys:
                    case = ("noisy", keys)
            if case is None and len(buckets["short"]) < args.per_bucket and len(words[-1]) <= 2 and context:
                # The last word alone, after everything before it.
                tail = words[-1]
                tail_sy = syllables(tail, lazy_pinyin, Style)
                if len(tail_sy) == len(tail) and HAN.match(tail):
                    ctx = context + text[: len(text) - len(tail)]
                    case = ("short", "".join(tail_sy))
                    text, context = tail, ctx
            if case is None:
                continue
            bucket, keys = case
            key = (bucket, keys, text)
            if key in seen:
                continue
            seen.add(key)
            buckets[bucket].append({
                "id": f"{bucket}-{len(buckets[bucket]):04d}",
                "bucket": bucket,
                "source": source,
                "context": context[-32:],
                "keys": keys,
                "target": text,
            })
    erhua = []
    erhua_rng = random.Random(args.seed + 1)
    for source, sentence in sources:
        if len(erhua) >= args.per_bucket:
            break
        for context, text, _ in pieces(sentence, jieba, erhua_rng):
            if not text.endswith("儿") or len(text) < 2 or text.count("儿") != 1:
                continue
            sy = syllables(text, lazy_pinyin, Style)
            if len(sy) != len(text) or sy[-1] != "er":
                continue
            erhua.append({
                "id": f"erhua-{len(erhua):04d}", "bucket": "erhua", "source": source,
                "context": context[-32:], "keys": "".join(sy), "target": text,
            })
    buckets["erhua"] = erhua
    with open(args.out, "w", encoding="utf-8") as out:
        for bucket in buckets.values():
            for case in bucket:
                out.write(json.dumps(case, ensure_ascii=False) + "\n")
    print(json.dumps({b: len(v) for b, v in buckets.items()}))


if __name__ == "__main__":
    main()
