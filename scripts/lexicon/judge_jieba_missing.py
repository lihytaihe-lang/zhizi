#!/usr/bin/env python3
"""Candidate general words: jieba words the shipped lexicon lacks, judged by DeepSeek.

    scripts/lexicon/judge_jieba_missing.py --jieba <jieba/dict.txt> --rime <Resources/Rime> \
        [--min-freq 5] [--batch 200] [--workers 8] [--max-tokens 3000000] --out <dir>

jieba 0.42.1 dict.txt (MIT; see docs/current/licenses) supplies word, frequency and part of
speech.  Names, places, organisations, numerals and function-word tags are dropped before
any call.  The rest goes to deepseek-flash in batches; the model returns the words that are
common in everyday or professional modern Chinese.  Output (out/approved.tsv): word,
jieba frequency, jieba tag; out/usage.json keeps token totals.  Words only; nothing private
is sent.  Stops when the token budget is spent.  Synthetic judgements use DeepSeek only
(owner 10-15 data policy).
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
DROP_TAGS = {"nr", "ns", "nt", "nz", "nrt", "nrfg", "t", "m", "mq", "q", "eng", "x", "j", "r", "c", "p", "u", "y",
             "o", "e", "h", "k", "nl", "ng", "vg", "ag", "dg", "tg", "rg", "zg", "ug", "yg", "mg", "an"}
PROMPT = """下面每行一个中文词。请只保留「今天的人在微信聊天、写工作文档、写文章时，会自己打出来的现代常用词」。
保留示例：滞涩、顺畅、落地、复盘、兜底、颗粒度、卡顿、迭代。
一律去掉：
- 人名、地名、机构名、作品或角色名；
- 古风、武侠、小说、戏曲、旧时官制用语（如 兵刃、穴道、廷杖、通衢、登时、首辅）；
- 方言词、生僻词、书面古语；
- 不成词的片段；
- 只在很窄的专业领域里才出现的冷僻术语。
拿不准的就去掉。只输出被保留的词本身，用顿号「、」分隔，不要编号、不要解释。

{items}"""


def lexicon_words(rime_dir):
    words = set()
    for path in glob.glob(os.path.join(rime_dir, "*.dict.yaml")):
        body = False
        for line in open(path, encoding="utf-8", errors="ignore"):
            if line.startswith("..."):
                body = True
            elif body and "\t" in line and not line.startswith("#"):
                words.add(line.split("\t", 1)[0])
    return words


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jieba", required=True)
    ap.add_argument("--rime", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--min-freq", type=int, default=5)
    ap.add_argument("--batch", type=int, default=200)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--max-tokens", type=int, default=3_000_000)
    ap.add_argument("--shuffle-seed", type=int, default=0, help="nonzero: random order instead of by frequency")
    ap.add_argument("--passes", type=int, default=2, help="independent judgements per batch; a word must pass all")
    ap.add_argument("--limit-batches", type=int, default=0, help="0 = all (trial runs use a few)")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    have = lexicon_words(args.rime)
    rows = []
    for line in open(args.jieba, encoding="utf-8"):
        parts = line.split()
        if len(parts) >= 3 and HAN.match(parts[0]) and parts[0] not in have and parts[2] not in DROP_TAGS \
                and int(parts[1]) >= args.min_freq:
            rows.append((parts[0], int(parts[1]), parts[2]))
    rows.sort(key=lambda r: -r[1])
    if args.shuffle_seed:
        import random
        random.Random(args.shuffle_seed).shuffle(rows)
    batches = [rows[i:i + args.batch] for i in range(0, len(rows), args.batch)]
    if args.limit_batches:
        batches = batches[:args.limit_batches]
    print(f"lexicon {len(have)} words; candidates {len(rows)}; calls {len(batches)}", flush=True)
    spent = collections.Counter()
    lock = threading.Lock()
    approved = {}

    def judge(batch):
        with lock:
            if spent["total_tokens"] >= args.max_tokens:
                return
        items = "\n".join(w for w, _, _ in batch)
        keep = None
        for _ in range(args.passes):
            try:
                text, usage = deepseek_call(PROMPT.format(items=items), 180)
            except Exception as error:  # a failed batch is simply not approved
                print("batch failed:", type(error).__name__, flush=True)
                return
            kept = {w for w in re.split(r"[、\s,，]+", text) if w}
            keep = kept if keep is None else keep & kept
            with lock:
                for key in ("prompt_tokens", "completion_tokens", "total_tokens"):
                    spent[key] += int(usage.get(key, 0))
                spent["calls"] += 1
        with lock:
            for word, freq, tag in batch:
                if word in (keep or ()):
                    approved[word] = (freq, tag)

    with ThreadPoolExecutor(args.workers) as pool:
        list(pool.map(judge, batches))
    with open(os.path.join(args.out, "approved.tsv"), "w", encoding="utf-8") as handle:
        for word, (freq, tag) in sorted(approved.items(), key=lambda item: -item[1][0]):
            handle.write(f"{word}\t{freq}\t{tag}\n")
    json.dump(dict(spent), open(os.path.join(args.out, "usage.json"), "w"))
    print(f"approved {len(approved)} of {sum(len(b) for b in batches)} judged; usage {dict(spent)}", flush=True)


if __name__ == "__main__":
    main()
