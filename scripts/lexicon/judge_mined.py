"""Second half of word discovery: DeepSeek picks, from the strings mine_words.py found, those that are real words people type.

  ime_context_model/.venv/bin/python scripts/lexicon/judge_mined.py <candidates.tsv> --out <dir> [--min-count 50] [--limit N] [--passes 3]

Positive selection: in every pass (batches reshuffled) DeepSeek lists only the strings it accepts, each with its pinyin; a string is
kept when at least `--keep-votes` passes accepted it.  The reading is the one most passes gave (it must be a valid reading of every
character, as gen_new_words.py demands), else the reading pypinyin gives.  Output: kept.tsv  text, reading, count, votes.
"""
import argparse
import csv
import os
import random
import re
import sys
import time
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "context_model", "zhizi"))
from gen_new_words import reading_ok  # noqa: E402
from synth_generate import deepseek_call  # noqa: E402
from pypinyin import Style, lazy_pinyin  # noqa: E402

PROMPT = """下面每行是从中文文本里统计出来的一个字串（可能是词，也可能只是句子片段）。请只挑出其中「完整、独立、现在的中国人会在输入法里打出来的词语」，每个一行，格式：词=拼音（音节用空格隔开，不标声调，ü 写作 v）。
可以保留：普通词语和固定说法、常见的动物植物食物物品名、职业身份、常见的机构品牌产品名、广为人知的地名（国家、省市、著名景点）、家喻户晓的人名。
必须丢弃：不完整的片段（如「州市」「的一部分」「是一个」）、句子的一段、音译外国人名地名的碎片、冷僻或只在百科里出现的人名地名、古汉语和文言、日韩人名、数字日期堆叠。
拿不准的丢弃。不要解释，不要编号，没有可保留的写“无”。

{items}"""
HAN = re.compile(r"^[一-鿿]{2,4}$")


def one_pass(rows, k, workers, batch):
    order = rows[:]
    random.Random(700 + k).shuffle(order)
    batches = [order[i:i + batch] for i in range(0, len(order), batch)]

    def run(b):
        for attempt in range(4):
            try:
                text, _ = deepseek_call(PROMPT.format(items="\n".join(r["text"] for r in b)), 300)
                break
            except Exception as e:
                time.sleep(5 * (attempt + 1))
        else:
            print("judge failed after retries", flush=True)
            return {}
        asked = {r["text"] for r in b}
        out = {}
        for line in text.splitlines():
            if "=" in line:
                w, p = line.split("=", 1)
                w, p = w.strip().strip("0123456789.、-* "), " ".join(p.strip().lower().replace("ü", "v").split())
                if w in asked:
                    out[w] = p
        return out

    accepted = {}
    with ThreadPoolExecutor(workers) as pool:
        for part in pool.map(run, batches):
            accepted.update(part)
    print(f"pass {k + 1}: accepted {len(accepted)} of {len(rows)}", flush=True)
    return accepted


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("candidates")
    ap.add_argument("--out", required=True)
    ap.add_argument("--min-count", type=int, default=50)
    ap.add_argument("--limit", type=int, default=0, help="judge only a random sample (trial runs)")
    ap.add_argument("--passes", type=int, default=3)
    ap.add_argument("--keep-votes", type=int, default=2)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--batch", type=int, default=100)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    rows = [r for r in csv.DictReader(open(args.candidates, encoding="utf-8"), delimiter="\t")
            if int(r["count"]) >= args.min_count and HAN.match(r["text"])]
    if args.limit:
        random.Random(1).shuffle(rows)
        rows = rows[:args.limit]
    print(f"judging {len(rows)} strings", flush=True)
    votes, readings = Counter(), defaultdict(Counter)
    for k in range(args.passes):
        for w, p in one_pass(rows, k, args.workers, args.batch).items():
            votes[w] += 1
            readings[w][p] += 1
    count = {r["text"]: int(r["count"]) for r in rows}
    kept = []
    for w, v in votes.items():
        if v < args.keep_votes:
            continue
        valid = [(n, p) for p, n in readings[w].items() if reading_ok(w, p)]
        reading = max(valid)[1] if valid else " ".join(lazy_pinyin(w, style=Style.NORMAL, v_to_u=False)).replace("ü", "v")
        kept.append((w, reading, count[w], v))
    kept.sort(key=lambda r: -r[2])
    with open(os.path.join(args.out, "kept.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\treading\tcount\tvotes\n")
        for r in kept:
            fh.write("\t".join(str(v) for v in r) + "\n")
    print(f"kept {len(kept)} of {len(rows)}", flush=True)


if __name__ == "__main__":
    main()
