#!/usr/bin/env python3
"""Q22A measurement: how often are first-page slots 3-5 (index 2..4) a text that is NOT a
dictionary word/phrase while shorter than the first candidate (i.e. a cut-off composition)?

  ladder_measure.py <shared Rime dir> replay <out-*.jsonl> [<out-*.jsonl> ...]
  ladder_measure.py <shared Rime dir> events <events.jsonl> [--min-build N]

"Word" = text is an entry of any *.dict.yaml in the shared dir (all readings). "Partial" =
shorter (in characters) than candidate 1. Approximation: the replay output has no per-slot
consumed length, so shorter-than-first stands in for 'input consumed < full length'."""
import glob, json, os, sys, collections

def load_words(shared):
    words = set()
    for path in glob.glob(os.path.join(shared, '*.dict.yaml')):
        body = False
        for line in open(path, encoding='utf-8'):
            if not body:
                body = line.strip() == '...'
                continue
            if line.startswith('#') or '\t' not in line:
                continue
            words.add(line.split('\t', 1)[0].strip())
    return words

def slot_stats(pages, words, label):
    n = len(pages); long_n = 0
    slots = collections.Counter(); bad = collections.Counter()
    for page in pages:
        if len(page) < 3 or not page[0]:
            continue
        long_n += 1
        for i in range(2, min(5, len(page))):
            t = page[i]
            slots[i] += 1
            if len(t) < len(page[0]) and t not in words and not t.isascii():
                bad[i] += 1
    anyb = sum(1 for page in pages if len(page) >= 3 and any(
        len(page[i]) < len(page[0]) and page[i] not in words and not page[i].isascii() for i in range(2, min(5, len(page)))))
    print(f'{label}: pages={n} with>=3 cands={long_n} pages with a non-word partial in slots3-5={anyb} ({100*anyb/max(long_n,1):.1f}%)')
    print('   per slot non-word partial:', {i + 1: f'{bad[i]}/{slots[i]}' for i in sorted(slots)})

def main():
    shared, mode, rest = sys.argv[1], sys.argv[2], sys.argv[3:]
    words = load_words(shared)
    print(f'dictionary texts loaded: {len(words)}')
    if mode == 'replay':
        for path in rest:
            pages = [json.loads(l)['page'] for l in open(path) if l.strip()]
            slot_stats(pages, words, os.path.basename(path))
            longp = [json.loads(l) for l in open(path) if l.strip()]
            longp = [r['page'] for r in longp if len(r['keys']) >= 15]
            slot_stats(longp, words, os.path.basename(path) + ' (keys>=15 chars)')
    else:
        min_build = 0
        if '--min-build' in rest:
            min_build = int(rest[rest.index('--min-build') + 1])
        pages = []; picks = []
        for l in open(rest[0]):
            try: d = json.loads(l)
            except Exception: continue
            b = d.get('build')
            if not b or not str(b).isdigit() or int(b) < min_build: continue
            c = d['candidates'][:5]
            pages.append(c)
            idx = d['selectedIndex']
            if 2 <= idx <= 4 and idx < len(c) and d['selectedText'] == c[idx]:
                picks.append((d['pinyin'], c, idx))
        slot_stats(pages, words, f'events build>={min_build}')
        nonword = [(p, c, i) for p, c, i in picks if len(c[i]) < len(c[0]) and c[i] not in words and not c[i].isascii()]
        print(f'owner picks of slot 3-5: {len(picks)} of {len(pages)} commits; of those non-word partial: {len(nonword)}')
        for p, c, i in picks: print('   ', 'NONWORD' if (p, c, i) in nonword else 'word   ', i + 1, p, c)
main()
