"""New-word layer: words and phrases that appeared or caught on in recent years
(具身智能, 搭子, 显眼包, 鸿蒙, 新质生产力), which no static word list has.

1. deepseek-flash lists candidates per (year x domain), each with its pinyin.
2. Keep 2-8 Han characters, not already shipped (any agent_* dict, pinyin_simp, or the
   general layer export); every syllable must be a valid reading of its character.
3. Count presence in the September 2026 Wikipedia dump and the synthetic chat corpus
   (recorded as evidence; recent slang is often absent from Wikipedia, so not required).
4. Three independent DeepSeek review passes with reshuffled batches; a word is dropped
   only when every pass says it is not a real, widely used term (same rule as
   judge_lexicon.py, which proved single passes erratic).

Output: <out>/new_words.tsv  text, reading, year, domain, wiki, synth, kept_by
Re-run monthly; the year list is the only thing to extend.
"""

import argparse
import glob
import json
import os
import random
import re
import sys
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from multiprocessing import Pool
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, os.path.dirname(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "context_model", "zhizi"))
from build_general_lexicon import existing_entries  # noqa: E402
from synth_generate import deepseek_call  # noqa: E402

from pypinyin import Style, pinyin  # noqa: E402

WORK = str(WORK_DIR)
YEARS = ["2023", "2024", "2025", "2026"]
DOMAINS = ["人工智能与大模型", "软件开发与编程", "互联网产品、平台与应用", "职场与办公", "网络流行语与梗",
           "社会热点、政策与经济", "消费、生活方式与旅游", "手机、汽车与科技硬件", "游戏、影视与娱乐", "金融与投资",
           "教育与考试", "健康与运动",
           # second run: product, brand and yearly hot-word lists the broad domains missed
           "人工智能产品、大模型与 AI 公司的中文名（如文心一言、通义千问、智谱、月之暗面、豆包、元宝）",
           "国产手机、操作系统、芯片与电子产品的品牌和型号中文名（如鸿蒙、麒麟、澎湃）",
           "国产新能源汽车品牌、车型和智驾系统的中文名（如问界、尊界、智界、极氪、小米汽车）",
           "App、小程序与互联网服务的中文名",
           "当年的年度十大流行语、年度热词、年度网络用语（如遥遥领先、情绪价值、松弛感、city不city 里的中文部分）",
           "当年的热门影视剧、综艺、游戏和网络小说名"]
GEN_PROMPT = """请列出在 {year} 年前后出现或明显流行起来的、中文用户会在输入法里打出来的「{domain}」领域新词或新说法，
尽量多，最多 80 个。要求：真实存在且被广泛使用（新闻、社交媒体、工作沟通里常见），不要编造；不要成语、不要老词；
纯汉字，2 到 8 个字（含英文字母的不要）。
每行一个，格式：词=拼音（音节用空格隔开，不标声调，ü 写作 v），不要编号、不要解释。"""
JUDGE_PROMPT = """下面每行是“词<TAB>年份<TAB>领域”，是候选的近年新词。请找出应当删除的：
编造的、不存在或几乎没人用的、意思不通的、只是普通老词（不是新词也没关系，但必须是常用词）、过于小众的专名。
真实存在并被广泛使用的新词、新说法、新梗、新产品名要保留。
严格按格式输出，没有就写“无”，不要解释：
删除：词1、词2、……

{items}"""
HAN = re.compile(r"^[一-鿿]{2,8}$")

# --mode common (owner 2026-10-10: "很多常识词汇或者短语词库中没有……尽量用一些新词汇，太老的感觉意义不大"):
# everyday words people use now, by domain and angle, rather than coinages by year.
COMMON_ANGLES = ["常见事物、物品、地点和机构的名称", "常用的动作、做法和动词短语", "日常口语里常说的说法和短语"]
COMMON_DOMAINS = ["吃喝与餐饮外卖", "穿着与日用品", "住房、租房与装修", "交通出行与驾车", "医疗看病与健康",
                  "家庭、育儿与亲戚", "购物、电商与快递", "工作职场与开会", "学校、学习与考试", "银行、理财与支付",
                  "政务办事与法律常识", "体育运动与健身", "天气、季节与自然", "常见动物与植物", "人际关系与情绪感受",
                  "微信聊天与社交网络", "手机、电脑与数码产品", "软件开发、AI 与互联网产品", "影视、音乐、游戏与娱乐",
                  "中国城市、省份与著名景点", "新闻时事与社会生活", "美妆、穿搭与个人护理"]
COMMON_GEN_PROMPT = """请列出现在的中国人在「{domain}」方面，日常会在输入法里打出来的「{angle}」，尽量多，最多 120 个。
要求：现在常用（近十年里大家还在用的说法），不要古旧的、文言的、历史书里才用的词；不要编造；不要成语；
纯汉字，2 到 6 个字（含英文字母的不要）。
每行一个，格式：词=拼音（音节用空格隔开，不标声调，ü 写作 v），不要编号、不要解释。"""
COMMON_JUDGE_PROMPT = """下面每行是“词<TAB>角度<TAB>领域”，是输入法候选的当代常用词。请找出应当删除的：
编造的、不存在的、几乎没人用的、生僻的、古旧或文言的、意思不通或说法别扭的、过于小众的专名。
现在日常常用的词和说法要保留。
严格按格式输出，没有就写“无”，不要解释：
删除：词1、词2、……

{items}"""


def char_readings(ch):
    return {p.replace("ü", "v") for p in pinyin(ch, style=Style.NORMAL, heteronym=True)[0]}


def reading_ok(word, reading):
    syl = reading.split()
    return len(syl) == len(word) and all(s in char_readings(c) for c, s in zip(word, syl))


def generate(jobs, workers, common=False):
    def run(job):
        year, domain = job
        prompt = (COMMON_GEN_PROMPT.format(angle=year, domain=domain) if common
                  else GEN_PROMPT.format(year=year, domain=domain))
        try:
            text, usage = deepseek_call(prompt, 360 if common else 240)
        except Exception as e:
            print("gen failed", year, domain, type(e).__name__, flush=True)
            return job, [], 0
        rows = []
        for line in text.splitlines():
            if "=" not in line:
                continue
            w, p = line.split("=", 1)
            w = w.strip().strip("0123456789.、-* ")
            p = " ".join(p.strip().lower().replace("ü", "v").split())
            rows.append((w, p))
        return job, rows, int(usage.get("completion_tokens", 0) or 0)

    out, tokens = [], 0
    with ThreadPoolExecutor(workers) as pool:
        for (year, domain), rows, t in pool.map(run, jobs):
            tokens += t
            out += [(w, p, year, domain) for w, p in rows]
    return out, tokens


_WORDS_BY_LEN = None


def _init_scan(by_len):
    global _WORDS_BY_LEN
    _WORDS_BY_LEN = by_len


def _scan_chunk(job):
    path, start, size = job
    counter = Counter()
    with open(path, "rb") as fh:
        fh.seek(start)
        text = fh.read(size).decode("utf-8", errors="ignore")
    for n, ws in _WORDS_BY_LEN.items():
        for i in range(len(text) - n + 1):
            if text[i:i + n] in ws:
                counter[text[i:i + n]] += 1
    return counter


def presence(words, procs=10, chunk=32 << 20):
    """Substring counts in the whole September 2026 Wikipedia dump and the synthetic chat."""
    by_len = defaultdict(set)
    for w in words:
        by_len[len(w)].add(w)

    def count(paths):
        jobs = []
        for path in paths:
            size = os.path.getsize(path)
            jobs += [(path, off, chunk) for off in range(0, size, chunk)]
        total = Counter()
        with Pool(procs, initializer=_init_scan, initargs=(dict(by_len),)) as pool:
            for c in pool.imap_unordered(_scan_chunk, jobs):
                total.update(c)
        return total

    wiki = count([os.path.join(WORK, "corpus/v2/wikipedia.train.txt")])
    synth = count(sorted(glob.glob(os.path.join(WORK, "synth/v1/batch-*/*.jsonl"))))
    return wiki, synth


def judge(rows, passes, workers, batch=120):
    deleted_by = Counter()
    for k in range(passes):
        order = rows[:]
        random.Random(100 + k).shuffle(order)
        batches = [order[i:i + batch] for i in range(0, len(order), batch)]

        def run(b):
            items = "\n".join(f"{r['text']}\t{r['year']}\t{r['domain']}" for r in b)
            try:
                text, _ = deepseek_call(JUDGE_PROMPT.format(items=items), 240)
            except Exception as e:
                print("judge failed", type(e).__name__, flush=True)
                return {r["text"] for r in b}
            out = set()
            for line in text.splitlines():
                if line.strip().startswith("删除"):
                    body = re.split(r"[：:]", line, 1)[-1]
                    out |= {x.strip() for x in re.split(r"[、,，]", body) if x.strip() and x.strip() != "无"}
            return out

        with ThreadPoolExecutor(workers) as pool:
            for delete in pool.map(run, batches):
                for w in delete:
                    deleted_by[w] += 1
        print(f"judge pass {k + 1} done", flush=True)
    return deleted_by


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(WORK, "lexicon/new-words-v1"))
    ap.add_argument("--workers", type=int, default=12)
    ap.add_argument("--passes", type=int, default=3)
    ap.add_argument("--mode", choices=["new", "common"], default="new")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    common = args.mode == "common"
    if common:
        global JUDGE_PROMPT
        JUDGE_PROMPT = COMMON_JUDGE_PROMPT
        jobs = [(a, d) for a in COMMON_ANGLES for d in COMMON_DOMAINS]
    else:
        jobs = [(y, d) for y in YEARS for d in DOMAINS]
    raw, tokens = generate(jobs, args.workers, common)
    json.dump(raw, open(os.path.join(args.out, "raw.json"), "w"), ensure_ascii=False)
    print(f"generated rows={len(raw)} output_tokens={tokens}", flush=True)

    shipped = set(existing_entries())
    general = os.path.join(WORK, "lexicon/export-v1/agent_general.dict.yaml")
    if os.path.exists(general):
        body = open(general, encoding="utf-8").read().split("...", 1)[-1]
        shipped |= {line.split("\t")[0] for line in body.splitlines() if "\t" in line}

    stats, rows, seen = Counter(), [], set()
    for w, p, year, domain in raw:
        if w in seen:
            stats["duplicate"] += 1
            continue
        seen.add(w)
        if not HAN.match(w):
            stats["not_han_2_8"] += 1
        elif w in shipped:
            stats["already_shipped"] += 1
        elif not reading_ok(w, p):
            stats["bad_reading"] += 1
        else:
            rows.append({"text": w, "reading": p, "year": year, "domain": domain})
    print(f"candidates={len(rows)} {dict(stats)}", flush=True)

    wiki, synth = presence({r["text"] for r in rows})
    deleted_by = judge(rows, args.passes, args.workers)
    kept = [r for r in rows if deleted_by[r["text"]] < args.passes]
    with open(os.path.join(args.out, "new_words.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\treading\tyear\tdomain\twiki\tsynth\tdeleted_votes\n")
        for r in sorted(kept, key=lambda r: (r["domain"], r["year"], r["text"])):
            fh.write(f"{r['text']}\t{r['reading']}\t{r['year']}\t{r['domain']}\t{wiki[r['text']]}"
                     f"\t{synth[r['text']]}\t{deleted_by[r['text']]}\n")
    summary = {"generated": len(raw), "candidates": len(rows), "kept": len(kept), "filter": stats,
               "removed_by_review": len(rows) - len(kept),
               "with_wiki_evidence": sum(1 for r in kept if wiki[r["text"]] > 0),
               "with_synth_evidence": sum(1 for r in kept if synth[r["text"]] > 0),
               "by_domain": Counter(r["domain"] for r in kept), "by_year": Counter(r["year"] for r in kept)}
    json.dump(summary, open(os.path.join(args.out, "summary.json"), "w"), ensure_ascii=False, indent=1)
    print(json.dumps(summary, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
