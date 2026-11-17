"""Sentence-level abbreviation cases from ASAP and Wikipedia validation text.

Cuts 4–10 character han spans on jieba word boundaries and spells each span four ways.
Pinyin matches build_independent_set.pinyin_reader (pypinyin NORMAL, no tone). An
initial is zh/ch/sh when the syllable starts that way, otherwise the syllable's first
letter, so a zero-initial syllable contributes the first letter of its written final
(a/o/e, and y/w for the syllables spelled that way).

  all_initials  every character is an initial
  head_full     first word in full, every later character an initial
  random_mix    30–60% of the words (inclusive) are initials, the rest full
  tail_full     last word in full, every earlier character an initial

head_full, tail_full and random_mix need at least two words; a single word would be
full pinyin or all initials. Seed 20261111. Writes tune.jsonl and test.jsonl at 7:3
inside each sub (tune count is n * 7 // 10).
"""
import argparse
from collections import Counter
import json
from pathlib import Path
import random
import re
import sys

_BENCH = Path(__file__).resolve().parent
_EVAL = _BENCH.parent
sys.path.insert(0, str(_EVAL))
sys.path.insert(0, str(_BENCH))
import build_external_set as external
import build_independent_set as independent

SEED = 20261111
SUBS = ("all_initials", "head_full", "random_mix", "tail_full")
SOURCES = (("asap", "asap.valid.txt"), ("wiki", "wikipedia.valid.txt"))
RAW = re.compile(r"[a-z]{2,}")
FIELDS = ("id", "bucket", "sub", "src", "source", "raw", "context", "target")


def pinyin_reader():
    return independent.pinyin_reader()


def mix_options(n):
    """Word counts k with 30% <= k/n <= 60%, as integers 3n <= 10k and 5k <= 3n."""
    return [k for k in range(1, n) if 3 * n <= 10 * k and 5 * k <= 3 * n]


def syllable_initial(syllable):
    if not syllable.isascii() or not syllable.isalpha():
        raise ValueError(f"bad syllable {syllable!r}")
    if syllable.startswith(("zh", "ch", "sh")):
        return syllable[:2]
    return syllable[:1]


def render_keys(words, syllables, abbreviated):
    parts = []
    offset = 0
    for index, word in enumerate(words):
        chunk = syllables[offset:offset + len(word)]
        offset += len(word)
        if len(chunk) != len(word):
            return None
        if index in abbreviated:
            parts.append("".join(syllable_initial(item) for item in chunk))
        else:
            parts.append("".join(chunk))
    if offset != len(syllables):
        return None
    return "".join(parts)


def keys_for(sub, words, syllables, rng=None):
    count = len(words)
    if sub == "all_initials":
        chosen = set(range(count))
    elif sub == "head_full":
        if count < 2:
            return None
        chosen = set(range(1, count))
    elif sub == "tail_full":
        if count < 2:
            return None
        chosen = set(range(count - 1))
    elif sub == "random_mix":
        options = mix_options(count)
        if not options:
            return None
        if rng is None:
            raise ValueError("random_mix needs a rng")
        chosen = set(rng.sample(range(count), rng.choice(options)))
    else:
        raise ValueError(sub)
    return render_keys(words, syllables, chosen)


def prefix_context(line, start, limit=32):
    return line[:start][-limit:]


def spelling_matches(sub, raw, words, syllables):
    """True when raw is one legal spelling of this sub for this word split."""
    parts = []
    offset = 0
    for word in words:
        chunk = tuple(syllables[offset:offset + len(word)])
        offset += len(word)
        if len(chunk) != len(word):
            return False
        parts.append(chunk)
    if offset != len(syllables):
        return False

    def fits(kinds):
        count = len(kinds)
        if sub == "all_initials":
            return all(kind != "full" for kind in kinds)
        if sub == "head_full":
            return count >= 2 and kinds[0] != "initial" and all(kind != "full" for kind in kinds[1:])
        if sub == "tail_full":
            return count >= 2 and kinds[-1] != "initial" and all(kind != "full" for kind in kinds[:-1])
        if sub == "random_mix":
            forced = sum(kind == "initial" for kind in kinds)
            free = sum(kind == "either" for kind in kinds)
            for extra in range(free + 1):
                chosen = forced + extra
                if 0 < chosen < count and 3 * count <= 10 * chosen and 5 * chosen <= 3 * count:
                    return True
            return False
        return False

    def walk(index, at, kinds):
        if index == len(parts):
            return at == len(raw) and fits(kinds)
        full = "".join(parts[index])
        initial = "".join(syllable_initial(item) for item in parts[index])
        options = []
        if raw.startswith(full, at):
            options.append(("either" if initial == full else "full", len(full)))
        if initial != full and raw.startswith(initial, at):
            options.append(("initial", len(initial)))
        return any(walk(index + 1, at + size, kinds + [kind]) for kind, size in options)

    return walk(0, 0, [])


def han_runs(line):
    runs = []
    start = None
    for index, char in enumerate(line):
        if "一" <= char <= "鿿":
            if start is None:
                start = index
        elif start is not None:
            runs.append((start, line[start:index]))
            start = None
    if start is not None:
        runs.append((start, line[start:]))
    return runs


def fragments(line, segmenter, rng):
    """Non-overlapping word groups of 4–10 hanzi. Length is drawn from the rng."""
    found = []
    for run_start, text in han_runs(line):
        words = [word for word in segmenter.cut(text, HMM=False) if word]
        if "".join(words) != text:
            continue
        starts = []
        cursor = run_start
        for word in words:
            starts.append(cursor)
            cursor += len(word)
        index = 0
        while index < len(words):
            if len(words[index]) > 10:
                index += 1
                continue
            want = rng.randint(4, 10)
            group = []
            total = 0
            end = index
            while end < len(words) and total + len(words[end]) <= 10:
                group.append(words[end])
                total += len(words[end])
                end += 1
                if total >= want:
                    break
            if total >= 4:
                found.append((starts[index], "".join(group), group))
                index = end
            else:
                index += 1
    return found


def _lines(path):
    rows = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if line.strip():
            rows.append((number, line))
    return rows


def collect(corpus, words, seed=SEED, per_sub=1000):
    corpus = Path(corpus)
    segmenter = external.Segmenter(words)
    read = pinyin_reader()
    sources = []
    for index, (src, filename) in enumerate(SOURCES):
        lines = _lines(corpus / filename)
        random.Random(seed + 10 * index).shuffle(lines)
        sources.append(dict(src=src, lines=lines, cursor=0, rng=random.Random(seed + 10 * index + 1)))
    seen = set()
    rows = []
    counts = Counter()
    while any(counts[sub] < per_sub for sub in SUBS):
        progressed = False
        for source in sources:
            if source["cursor"] >= len(source["lines"]) or all(counts[sub] >= per_sub for sub in SUBS):
                continue
            _line_number, line = source["lines"][source["cursor"]]
            source["cursor"] += 1
            progressed = True
            rng = source["rng"]
            for start, target, pieces in fragments(line, segmenter, rng):
                if all(counts[sub] >= per_sub for sub in SUBS):
                    break
                context = prefix_context(line, start)
                if target in context:
                    continue
                syllables = read(target)
                if len(syllables) != len(target):
                    continue
                for sub in SUBS:
                    if counts[sub] >= per_sub:
                        continue
                    raw = keys_for(sub, pieces, syllables, rng)
                    if not raw or not RAW.fullmatch(raw):
                        continue
                    key = (sub, raw, context, target)
                    if key in seen:
                        continue
                    seen.add(key)
                    number = counts[sub]
                    counts[sub] += 1
                    rows.append(dict(id=f"sa-{seed}-{source['src']}-{sub}-{number:05d}",
                                     bucket="sent_abbrev", sub=sub, src=source["src"], source="ext",
                                     raw=raw, context=context, target=target))
        if not progressed:
            break
    return rows


def split_rows(rows, seed):
    rng = random.Random(seed)
    tune, test = [], []
    for sub in SUBS:
        group = [row for row in rows if row["sub"] == sub]
        rng.shuffle(group)
        cut = len(group) * 7 // 10
        tune.extend(group[:cut])
        test.extend(group[cut:])
    tune.sort(key=lambda row: row["id"])
    test.sort(key=lambda row: row["id"])
    return tune, test


def write_jsonl(path, rows):
    with Path(path).open("w", encoding="utf-8") as stream:
        for row in rows:
            stream.write(json.dumps({key: row[key] for key in FIELDS}, ensure_ascii=False) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--words", type=Path, required=True)
    parser.add_argument("--out", type=Path, default=Path("tmp/sent-abbrev"))
    parser.add_argument("--seed", type=int, default=SEED)
    parser.add_argument("--per-sub", type=int, default=1000)
    args = parser.parse_args()
    rows = collect(args.corpus, args.words, args.seed, args.per_sub)
    tune, test = split_rows(rows, args.seed)
    args.out.mkdir(parents=True, exist_ok=True)
    write_jsonl(args.out / "tune.jsonl", tune)
    write_jsonl(args.out / "test.jsonl", test)
    summary = {}
    for sub in SUBS:
        summary[sub] = dict(total=sum(row["sub"] == sub for row in rows),
                            tune=sum(row["sub"] == sub for row in tune),
                            test=sum(row["sub"] == sub for row in test),
                            asap=sum(row["sub"] == sub and row["src"] == "asap" for row in rows),
                            wiki=sum(row["sub"] == sub and row["src"] == "wiki" for row in rows))
    print(json.dumps(summary, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
