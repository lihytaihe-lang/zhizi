"""Second-pass filter for mined phrases: DeepSeek keeps only units people type in one go.

Heuristic pre-filter drops obvious fragments (leading/trailing function characters),
then the top-N by frequency go to deepseek-flash in batches of 200; the model returns the
indices of real phrases. Output: phrases/approved.jsonl (phrase, reading, count).
"""

import argparse
import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

sys.path.insert(0, os.path.dirname(__file__))
from synth_generate import deepseek_call  # noqa: E402

LEAD = set("的了着过得地")
TRAIL = set("的了着得地")
PROMPT = """下面是从中文聊天和评论语料里统计出来的高频字串，每行一个。
请挑出其中“人们打字时会作为一个整体连着打出来的常用词组或短语”（例如：应该是、我们现在、可以继续、什么时候、考虑一下、还要不要），
去掉不成词的片段（例如：的一、了两、个小、面有、是吃）、去掉只在餐饮点评里才出现的生僻搭配。
只输出被保留的字串本身，用顿号“、”分隔，不要编号、不要任何解释。

{items}"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default=str(WORK_DIR / 'phrases'))
    ap.add_argument("--top", type=int, default=50000)
    ap.add_argument("--batch", type=int, default=200)
    ap.add_argument("--workers", type=int, default=12)
    args = ap.parse_args()
    rows = [json.loads(l) for l in open(os.path.join(args.dir, "candidates.jsonl"), encoding="utf-8")]
    rows = [r for r in rows if r["phrase"][0] not in LEAD and r["phrase"][-1] not in TRAIL]
    rows = rows[:args.top]
    batches = [rows[i:i + args.batch] for i in range(0, len(rows), args.batch)]
    print(f"after heuristic filter, judging top {len(rows)} in {len(batches)} calls", flush=True)

    def judge(batch):
        items = "\n".join(r["phrase"] for r in batch)
        try:
            text, usage = deepseek_call(PROMPT.format(items=items), 180)
        except Exception as e:  # keep going; a failed batch is simply not approved
            print("batch failed:", type(e).__name__, flush=True)
            return [], 0
        keep = {x.strip() for x in re.split(r"[、,，\s]+", text) if x.strip()}
        return [r for r in batch if r["phrase"] in keep], int(usage.get("completion_tokens", 0) or 0)

    approved, tokens = [], 0
    with ThreadPoolExecutor(args.workers) as pool:
        for kept, t in pool.map(judge, batches):
            approved += kept
            tokens += t
    approved.sort(key=lambda r: -r["count"])
    with open(os.path.join(args.dir, "approved.jsonl"), "w", encoding="utf-8") as f:
        for r in approved:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"approved {len(approved)} of {len(rows)}; output tokens {tokens}")
    print("sample:", " ".join(r["phrase"] for r in approved[:120]))


if __name__ == "__main__":
    main()
