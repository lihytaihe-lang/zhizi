"""P0 of LATTICE-NBEST-PLAN: how many "never produced" misses could a word+character lattice produce at all?

  python3 scripts/eval/lattice_upper_bound.py <Rime resources dir> <traced eval output.jsonl> [dump.tsv]

A miss is "never produced" when the target is not on the first page, not among the whole-reading
channel's alternatives (`alternatives`) and not on Rime's next pages (`onNextPages`); the output
must come from a ZHIZI_EVAL_TRACE=1 run. For each such miss the target is cut into pieces that are
dictionary entries (words or single characters); each piece must match a slice of the typed keys,
syllable by syllable, where a syllable may be typed whole or as any prefix of it (hais = 还是).
  exact  producible with whole syllables only (no letter left out) and no edit
  tier1  producible with no edit to the keys but some syllable typed as a prefix
  tier2  producible after one edit to the keys (a letter missing / extra / wrong / swapped)
  none   no piece sequence matches: a word the dictionaries lack, or a reading they do not carry
Read-only: nothing is written except the optional dump of the misses and their tier."""
import collections, glob, json, os, sys

res, out = sys.argv[1], sys.argv[2]
dump = sys.argv[3] if len(sys.argv) > 3 else None

readings = collections.defaultdict(set)
for path in glob.glob(os.path.join(res, "*.dict.yaml")):
    body = open(path, encoding="utf-8").read().split("\n...\n", 1)
    if len(body) < 2:
        continue
    for line in body[1].splitlines():
        parts = line.split("\t")
        if len(parts) < 2 or not parts[0] or parts[0].startswith("#"):
            continue
        text, reading = parts[0], tuple(parts[1].split())
        if len(reading) == len(text):
            readings[text].add(reading)
LONGEST = 8
LETTERS = "abcdefghijklmnopqrstuvwxyz"


def producible(keys, target, abbreviate=True):
    n, m = len(target), len(keys)
    seen = set()
    stack = [(0, 0)]
    while stack:
        i, j = stack.pop()
        if (i, j) in seen:
            continue
        seen.add((i, j))
        if i == n and j == m:
            return True
        if i == n:
            continue
        for length in range(1, min(LONGEST, n - i) + 1):
            for reading in readings.get(target[i:i + length], ()):
                positions = {j}
                for syllable in reading:
                    nxt = set()
                    for p in positions:
                        for k in range(1, len(syllable) + 1) if abbreviate else (len(syllable),):
                            if keys[p:p + k] == syllable[:k] and p + k <= m:
                                nxt.add(p + k)
                            elif keys[p:p + k] != syllable[:k]:
                                break
                    positions = nxt
                    if not positions:
                        break
                for p in positions:
                    stack.append((i + length, p))
    return False


def one_edit_variants(keys):
    seen = set()
    for i in range(len(keys)):
        seen.add(keys[:i] + keys[i + 1:])                      # extra letter
        for c in LETTERS:
            seen.add(keys[:i] + c + keys[i + 1:])              # wrong letter
        if i + 1 < len(keys):
            seen.add(keys[:i] + keys[i + 1] + keys[i] + keys[i + 2:])  # swapped
    for i in range(len(keys) + 1):
        for c in LETTERS:
            seen.add(keys[:i] + c + keys[i:])                  # missing letter
    seen.discard(keys)
    return seen


rows = [json.loads(line) for line in open(out, encoding="utf-8")]
table = collections.defaultdict(collections.Counter)
dumped = []
for r in rows:
    target = r["target"]
    if r.get("onPage") or target in (r.get("alternatives") or []) or r.get("onNextPages"):
        continue
    keys = r["keys"].lower()
    if not keys.isalpha() or not all("一" <= c <= "鿿" for c in target):
        tier = "other"
    elif producible(keys, target, abbreviate=False):
        tier = "exact"
    elif producible(keys, target):
        tier = "tier1"
    elif any(producible(v, target) for v in one_edit_variants(keys)):
        tier = "tier2"
    else:
        tier = "none"
    table[r["bucket"]][tier] += 1
    dumped.append((r["bucket"], r["id"], keys, target, r.get("page", [""])[0] if r.get("page") else "", tier))

print("bucket\tnever\texact\ttier1\ttier2\tnone\tother")
for bucket in sorted(table):
    c = table[bucket]
    print(f"{bucket}\t{sum(c.values())}\t{c['exact']}\t{c['tier1']}\t{c['tier2']}\t{c['none']}\t{c['other']}")
if dump:
    with open(dump, "w", encoding="utf-8") as fh:
        for row in dumped:
            fh.write("\t".join(row) + "\n")
