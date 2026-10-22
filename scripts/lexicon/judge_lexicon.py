"""DeepSeek review of a proposed lexicon: drop non-words, fix polyphone readings.

Each batch of `word<TAB>reading` lines goes to deepseek-flash, which returns two lists:
words to delete (fragments, junk, obscure proper nouns) and reading corrections.
Single-pass deletions proved erratic (复现, 待办, 我可以 deleted in one batch, kept in
another), so the list is reviewed in several passes with reshuffled batches and a word is
deleted only when every pass deletes it. A word in a failed batch counts as deleted for
that pass. A correction is accepted only if it has one valid syllable per character and
no pass proposes a different one. Output: <dir>/reviewed.tsv (same columns as
lexicon.tsv, readings corrected) and <dir>/review.json (what was removed / corrected).
"""

import argparse
import random
import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "context_model", "zhizi"))
from synth_generate import deepseek_call  # noqa: E402

from pypinyin import pinyin_dict  # noqa: E402

LEAD = set("的了着过得地和与及")
TRAIL = set("的了着得地一和与及")
PROMPT = """下面每行是“词<TAB>拼音”，是准备加进拼音输入法词库的候选。请审查：

一、找出应当删除的：不成词的片段（如“来吃一”“东西比较”“理成”）、错误切分、生僻或只在某个地方用的专名、
没人会在输入法里整体打出来的组合。常用词、常用口语短语（如“还有个问题”“我们现在”“考虑一下”）、常用专有名词要保留。
二、找出拼音标错的（多音字），给出正确拼音，音节间用空格，不标声调，ü 写作 v。

严格按下面格式输出，没有就写“无”，不要解释：
删除：词1、词2、……
订正：词=pin yin、词=pin yin、……

{items}"""


def valid_syllables():
    syl = set()
    for value in pinyin_dict.pinyin_dict.values():
        for p in value.split(","):
            s = (p.translate(str.maketrans("āáǎàēéěèīíǐìōóǒòūúǔùǖǘǚǜüńňǹḿ", "aaaaeeeeiiiioooouuuuvvvvvnnnm")))
            syl.add(s)
    return syl


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default=str(WORK_DIR / 'lexicon/v2'))
    ap.add_argument("--batch", type=int, default=150)
    ap.add_argument("--workers", type=int, default=12)
    ap.add_argument("--passes", type=int, default=2)
    args = ap.parse_args()
    lines = open(os.path.join(args.dir, "lexicon.tsv"), encoding="utf-8").read().splitlines()
    header, rows = lines[0], [line.split("\t") for line in lines[1:]]
    pre = [r for r in rows if r[0][0] not in LEAD and r[0][-1] not in TRAIL]
    print(f"rows={len(rows)} after function-char filter={len(pre)}", flush=True)
    syllables = valid_syllables()

    def review(batch):
        items = "\n".join(f"{r[0]}\t{r[1]}" for r in batch)
        try:
            text, usage = deepseek_call(PROMPT.format(items=items), 240)
        except Exception as e:
            print("batch failed:", type(e).__name__, flush=True)
            return None, {}, 0
        delete, fixes = set(), {}
        for line in text.splitlines():
            line = line.strip()
            if line.startswith("删除"):
                body = re.split(r"[：:]", line, 1)[-1]
                delete |= {x.strip() for x in re.split(r"[、,，]", body) if x.strip() and x.strip() != "无"}
            elif line.startswith("订正"):
                body = re.split(r"[：:]", line, 1)[-1]
                for item in re.split(r"[、,，]", body):
                    if "=" in item:
                        w, p = item.split("=", 1)
                        fixes[w.strip()] = " ".join(p.strip().lower().replace("ü", "v").split())
        return delete, fixes, int(usage.get("completion_tokens", 0) or 0)

    deleted_by = {r[0]: 0 for r in pre}
    proposals = {}
    failed, tokens = 0, 0
    for k in range(args.passes):
        order = pre[:]
        random.Random(k).shuffle(order)
        batches = [order[i:i + args.batch] for i in range(0, len(order), args.batch)]
        with ThreadPoolExecutor(args.workers) as pool:
            for batch, (delete, fixes, t) in zip(batches, pool.map(review, batches)):
                tokens += t
                if delete is None:
                    failed += len(batch)
                    delete = {r[0] for r in batch}
                for r in batch:
                    if r[0] in delete:
                        deleted_by[r[0]] += 1
                    fix = fixes.get(r[0])
                    if fix and fix != r[1]:
                        proposals.setdefault(r[0], set()).add(fix)
        print(f"pass {k + 1}: deleted so far in every pass="
              f"{sum(1 for v in deleted_by.values() if v == k + 1)}", flush=True)

    kept, removed, corrected, rejected_fix = [], [], {}, {}
    for r in pre:
        if deleted_by[r[0]] == args.passes:
            removed.append(r[0])
            continue
        fixes = proposals.get(r[0], set())
        if len(fixes) == 1:
            fix = next(iter(fixes))
            parts = fix.split()
            if len(parts) == len(r[0]) and all(p in syllables for p in parts):
                corrected[r[0]] = (r[1], fix)
                r = [r[0], fix] + r[2:]
            else:
                rejected_fix[r[0]] = fix
        elif len(fixes) > 1:
            rejected_fix[r[0]] = sorted(fixes)
        kept.append(r)
    with open(os.path.join(args.dir, "reviewed.tsv"), "w", encoding="utf-8") as fh:
        fh.write(header + "\n")
        for r in kept:
            fh.write("\t".join(r) + "\n")
    json.dump({"input": len(rows), "after_function_filter": len(pre), "kept": len(kept),
               "removed": len(removed), "failed_batches_rows": failed, "corrected": len(corrected),
               "rejected_fixes": len(rejected_fix), "output_tokens": tokens,
               "removed_sample": removed[:300], "corrected_sample": dict(list(corrected.items())[:200]),
               "rejected_fix_sample": dict(list(rejected_fix.items())[:50])},
              open(os.path.join(args.dir, "review.json"), "w"), ensure_ascii=False, indent=1)
    print(f"kept={len(kept)} removed={len(removed)} corrected={len(corrected)} failed_rows={failed} tokens={tokens}")


if __name__ == "__main__":
    main()
