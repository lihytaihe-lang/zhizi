"""Offline prefilter study for initials input: does the word that follows this context make it
into the top-N when candidates are ranked by (a) the engine's dictionary weight, (b) corpus
frequency, (c) corpus frequency conditioned on the preceding 1-2 characters?

No engine, no model. Candidates = lexicon words whose syllable initials equal the typed keys.
Counts come from the TRAIN splits only (cases come from validation splits), segmentation-free:
a word is counted wherever its string occurs, with the 1 and 2 characters before it.

usage: abbrev_prefilter.py <shared dir> <events.jsonl> <corpus dir> [--workers 6]
events: {"pinyin": keys, "selectedText": target, "precedingText": context} per line.
"""
import argparse, json, math, sys
from collections import Counter, defaultdict
from multiprocessing import Pool
from pathlib import Path

CORPORA = ("synth.train.txt", "asap.train.txt", "tatoeba.train.txt")


def load_lexicon(shared, keys):
    """text -> (best dictionary weight, set of initials strings) for words matching the wanted keys."""
    want = set(keys)
    lex = {}
    for f in sorted(Path(shared).glob("*.dict.yaml")):
        body = False
        for line in f.read_text(encoding="utf-8").splitlines():
            if line.startswith("..."):
                body = True
                continue
            if not body:
                continue
            p = line.split("\t")
            if len(p) < 2 or not p[0]:
                continue
            syl = p[1].split()
            if len(syl) != len(p[0]) or not 2 <= len(syl) <= 4:
                continue
            ini = "".join(s[0] for s in syl)
            if ini not in want:
                continue
            w = int(p[2]) if len(p) > 2 and p[2].isdigit() else 1
            old = lex.get(p[0])
            lex[p[0]] = (max(w, old[0]) if old else w, (old[1] if old else set()) | {ini})
    return lex


def count_chunk(args):
    path, start, end, words, need2 = args
    uni, c1, c2, ch, bg = Counter(), Counter(), Counter(), Counter(), Counter()
    with open(path, "rb") as fh:
        fh.seek(start)
        if start:
            fh.readline()
        while fh.tell() < end:
            raw = fh.readline()
            if not raw:
                break
            line = raw.decode("utf-8", "ignore").rstrip("\n")
            ch.update(line)
            n = len(line)
            for i in range(n - 1):
                two = line[i:i + 2]
                if two in need2:
                    bg[two] += 1
                for L in (2, 3, 4):
                    s = line[i:i + L]
                    if s in words:
                        uni[s] += 1
                        c1[(line[i - 1] if i else "", s)] += 1
                        c2[(line[max(0, i - 2):i], s)] += 1
    return uni, c1, c2, ch, bg


def count_corpus(corpus, words, need2, workers, names):
    jobs = []
    for name in names:
        path = Path(corpus) / name
        size = path.stat().st_size
        step = max(size // (workers * 2), 1)
        jobs += [(str(path), s, min(s + step, size), words, need2) for s in range(0, size, step)]
    uni, c1, c2, ch, bg = Counter(), Counter(), Counter(), Counter(), Counter()
    with Pool(workers) as pool:
        for u, a, b, c, d in pool.imap_unordered(count_chunk, jobs):
            uni.update(u); c1.update(a); c2.update(b); ch.update(c); bg.update(d)
    return uni, c1, c2, ch, bg


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("shared"); ap.add_argument("events"); ap.add_argument("corpus")
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out")
    ap.add_argument("--corpora", default=",".join(CORPORA), help="train files to count from")
    a = ap.parse_args()
    cases = [json.loads(l) for l in open(a.events, encoding="utf-8")]
    keys = {c["pinyin"] for c in cases}
    lex = load_lexicon(a.shared, keys)
    words = frozenset(lex)
    need2 = frozenset(c["precedingText"][-2:] for c in cases if len(c["precedingText"]) >= 2)
    print(f"cases {len(cases)} keys {len(keys)} lexicon words matching those keys {len(words)}", file=sys.stderr)
    uni, c1, c2, ch, bg = count_corpus(a.corpus, words, need2, a.workers, a.corpora.split(","))
    total = sum(ch.values())
    print(f"corpus chars {total:,}", file=sys.stderr)
    by_key = defaultdict(list)
    for t, (w, inis) in lex.items():
        for i in inis:
            by_key[i].append(t)

    MU = 20.0  # smoothing mass toward the lower-order estimate

    def p_uni(t):
        return (uni[t] + 0.5) / total

    def p1(t, prev):
        base = p_uni(t)
        d = ch[prev] if prev else 0
        return (c1[(prev, t)] + MU * base) / (d + MU) if prev else base

    def p2(t, prev2):
        lower = p1(t, prev2[-1:] if prev2 else "")
        d = bg[prev2] if len(prev2) == 2 else 0
        return (c2[(prev2, t)] + MU * lower) / (d + MU) if len(prev2) == 2 else lower

    scorers = {
        "engine weight (现状)": lambda t, c: math.log(max(lex[t][0], 1)),
        "corpus frequency": lambda t, c: math.log(p_uni(t)),
        "cond. on 1 char": lambda t, c: math.log(p1(t, c[-1:])),
        "cond. on 2 chars": lambda t, c: math.log(p2(t, c[-2:])),
        "cond.2 + 0.5*engine": lambda t, c: math.log(p2(t, c[-2:])) + 0.5 * math.log(max(lex[t][0], 1)),
    }
    Ns = (12, 20, 40, 100)
    hit = {k: Counter() for k in scorers}
    known = n = 0
    rows = []
    for c in cases:
        keys_, target, ctx = c["pinyin"], c["selectedText"], c["precedingText"]
        cand = by_key.get(keys_, [])
        n += 1
        if target not in lex:
            continue
        known += 1
        for name, f in scorers.items():
            ranked = sorted(cand, key=lambda t: -f(t, ctx))
            r = ranked.index(target) if target in ranked else 10 ** 9
            for N in Ns:
                hit[name][N] += r < N
            if name == "cond.2 + 0.5*engine":
                rows.append({"keys": keys_, "target": target, "rank": r, "n_cand": len(cand)})
    print(f"\ncases {n}, target in lexicon {known} ({known/n:.0%}); candidates per key (median) "
          f"{sorted(len(by_key[k]) for k in keys)[len(keys)//2]}")
    print("recall@N of target among ALL lexicon words with those initials (over all cases)")
    print(f"{'scorer':<24}" + "".join(f"{'@'+str(N):>8}" for N in Ns))
    for name in scorers:
        print(f"{name:<24}" + "".join(f"{hit[name][N]/n:>8.1%}" for N in Ns))
    if a.out:
        Path(a.out).write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in rows))


if __name__ == "__main__":
    main()
