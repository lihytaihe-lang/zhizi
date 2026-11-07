#!/usr/bin/env python3
"""Everyday-usage order for homophone words whose shipped weights are nearly tied.

    scripts/lexicon/judge_homophone_order.py --rime <Resources/Rime> --out <dir> \
        [--ratio 2] [--min-weight 100] [--max-words 6] [--batch 40] [--passes 2]

Groups: readings (2-4 Han characters) with at least two words whose two highest shipped
weights are within `ratio` of each other, the highest being at least `min-weight` (shipped
weight = highest over all layers; the owner's own layer is not in the shipped directory).
Each group goes to deepseek-flash, which orders its words by how likely a person is to want
each one when typing that reading today, judged by meaning, part of speech and usual
collocations; field-specific terms come after general words unless they are common in
everyday talk.  Every group is judged `passes` times independently; the order that
results is the sum of ranks (ties keep the shipped order), and a group is written only if
every pass put the same word first, so unstable judgements change nothing.
Output: out/orders.tsv (reading, ordered words, shipped weights, changed flag), out/usage.json.
Words only; nothing private is sent.  Synthetic judgements use DeepSeek only.
"""
import argparse
import collections
import glob
import json
import os
import re
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "context_model" / "zhizi"))
from synth_generate import deepseek_call  # noqa: E402

HAN = re.compile(r"^[一-鿿]{2,4}$")
PROMPT = """下面每一行是一组读音相同的中文词，格式「编号: 词1 词2 词3」。
请对每一组，按「今天的中文使用者在微信聊天、写工作文档、写文章时，打出这个读音，最可能想要哪个词」从最常用到最不常用排序。
判断时请结合词义、词性和常见搭配来想，例如：「出圈」是网络和传播里的常用说法，「除权」是股票里的专业术语，通用场景里前者用得多；
领域专业词排在通用词之后，除非它在日常说话里也很常用；不要因为字形生僻、书面、古风就把它排前面。
每一组都要把给出的所有词排进去。只输出排序结果，每组一行，格式「编号: 词A>词B>词C」，不要解释。

{items}"""


def shipped_weights(rime_dir):
    by = collections.defaultdict(dict)
    for path in glob.glob(os.path.join(rime_dir, "*.dict.yaml")):
        body = False
        for line in open(path, encoding="utf-8", errors="ignore"):
            if line.startswith("..."):
                body = True
            elif body and not line.startswith("#"):
                parts = line.rstrip("\n").split("\t")
                if len(parts) >= 3 and HAN.match(parts[0]) and parts[2].isdigit():
                    weight = int(parts[2])
                    if weight > by[parts[1]].get(parts[0], 0):
                        by[parts[1]][parts[0]] = weight
    return by


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rime", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--ratio", type=float, default=2.0)
    ap.add_argument("--min-weight", type=int, default=100)
    ap.add_argument("--max-words", type=int, default=6)
    ap.add_argument("--batch", type=int, default=40)
    ap.add_argument("--passes", type=int, default=2)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--limit-batches", type=int, default=0)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    groups = []
    for reading, words in shipped_weights(args.rime).items():
        if len(words) < 2:
            continue
        ranked = sorted(words.items(), key=lambda item: (-item[1], item[0]))[:args.max_words]
        (_, top), (_, second) = ranked[0], ranked[1]
        if top >= args.min_weight and top <= second * args.ratio:
            groups.append((reading, ranked))
    groups.sort(key=lambda g: g[0])
    batches = [groups[i:i + args.batch] for i in range(0, len(groups), args.batch)]
    if args.limit_batches:
        batches = batches[:args.limit_batches]
    print(f"near-tie groups {len(groups)}; calls {len(batches) * args.passes}", flush=True)
    spent = collections.Counter()
    lock = threading.Lock()
    results = {}

    def judge(batch):
        lines = "\n".join(f"{i}: {' '.join(w for w, _ in ranked)}" for i, (_, ranked) in enumerate(batch))
        orders = []
        for _ in range(args.passes):
            try:
                text, usage = deepseek_call(PROMPT.format(items=lines), 240)
            except Exception as error:  # a failed batch changes nothing
                print("batch failed:", type(error).__name__, flush=True)
                return
            with lock:
                for key in ("prompt_tokens", "completion_tokens", "total_tokens"):
                    spent[key] += int(usage.get(key, 0))
                spent["calls"] += 1
            parsed = {}
            for line in text.splitlines():
                match = re.match(r"^\s*(\d+)\s*[:：]\s*(.+)$", line)
                if match:
                    parsed[int(match.group(1))] = [w.strip() for w in re.split(r"[>＞]", match.group(2)) if w.strip()]
            orders.append(parsed)
        for i, (reading, ranked) in enumerate(batch):
            words = [w for w, _ in ranked]
            passes = [o.get(i) for o in orders]
            if any(p is None or sorted(p) != sorted(words) for p in passes):
                continue  # a pass dropped, added or repeated a word: leave this group alone
            if len({p[0] for p in passes}) != 1:
                continue  # the passes disagree on the first word
            score = {w: sum(p.index(w) for p in passes) for w in words}
            order = sorted(words, key=lambda w: (score[w], words.index(w)))
            with lock:
                results[reading] = (order, ranked)

    with ThreadPoolExecutor(args.workers) as pool:
        list(pool.map(judge, batches))
    changed = 0
    with open(os.path.join(args.out, "orders.tsv"), "w", encoding="utf-8") as handle:
        for reading, (order, ranked) in sorted(results.items()):
            shipped = [w for w, _ in ranked]
            flag = int(order != shipped)
            changed += flag
            weights = ",".join(str(x) for _, x in ranked)
            handle.write(f"{reading}\t{'>'.join(order)}\t{'>'.join(shipped)}\t{weights}\t{flag}\n")
    json.dump(dict(spent), open(os.path.join(args.out, "usage.json"), "w"))
    print(f"judged {len(results)} of {sum(len(b) for b in batches)} groups; order changed in {changed}; usage {dict(spent)}", flush=True)


if __name__ == "__main__":
    main()
