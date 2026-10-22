"""Targeted phrase generation with deepseek-flash for the IME phrase layer.

The mined list misses compositional colloquial phrases built around a few hinge
words (还有个问题 / 没什么问题 / 怎么看 / 我怎么知道). We ask the model, hinge by
hinge, for phrases people type in one go, then keep only phrases that are
2-6 CJK characters, not already in the IME dictionaries, and get a pypinyin reading.

Output: phrases/generated.jsonl {phrase, reading, hinge}
"""

import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

from pypinyin import Style, lazy_pinyin

sys.path.insert(0, os.path.dirname(__file__))
from mine_phrases import dictionary_words  # noqa: E402
from synth_generate import deepseek_call  # noqa: E402

HINGES = ["什么", "问题", "可以", "考虑", "应该", "现在", "之前", "之后", "怎么", "为什么", "是不是", "有没有",
          "要不要", "能不能", "一下", "一点", "这个", "那个", "我们", "你们", "他们", "就是", "还是", "但是",
          "所以", "然后", "因为", "如果", "已经", "还有", "没有", "知道", "觉得", "感觉", "需要", "看看",
          "试试", "处理", "解决", "完成", "开始", "继续", "修改", "确认", "检查", "测试", "版本", "文档",
          "项目", "功能", "时候", "意思", "情况", "方案", "结果", "问一下", "说一下", "看一下", "改一下"]
STYLES = ["微信聊天", "和同事讨论工作", "给 AI 编程助手下指令"]
PROMPT = """请列出 80 个在“{style}”时，人们会作为一个整体连着打出来的中文口语短语，每个都要包含“{hinge}”。
长度 2 到 6 个字，例如：还有个问题、没什么问题、怎么看、我怎么知道、应该是对的、你考虑一下、这到底是什么。
要常用、自然，不要成语、不要书面语、不要完整长句。只输出短语，用顿号“、”分隔，不要编号、不要解释。"""
CJK = re.compile(r"^[一-鿿]{2,6}$")


def main():
    out_dir = str(WORK_DIR / 'phrases')
    known = dictionary_words()
    jobs = [(h, s) for h in HINGES for s in STYLES]

    def run(job):
        hinge, style = job
        try:
            text, usage = deepseek_call(PROMPT.format(hinge=hinge, style=style), 180)
        except Exception as e:
            print("fail", hinge, style, type(e).__name__, flush=True)
            return hinge, [], 0
        items = [x.strip() for x in re.split(r"[、,，\s]+", text) if x.strip()]
        return hinge, [x for x in items if CJK.match(x) and hinge in x], int(usage.get("completion_tokens", 0) or 0)

    seen, rows, tokens = set(), [], 0
    with ThreadPoolExecutor(12) as pool:
        for hinge, items, t in pool.map(run, jobs):
            tokens += t
            for x in items:
                if x not in seen and x not in known:
                    seen.add(x)
                    rows.append({"phrase": x, "reading": " ".join(lazy_pinyin(x, style=Style.NORMAL)), "hinge": hinge})
    with open(os.path.join(out_dir, "generated.jsonl"), "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"calls={len(jobs)} new phrases={len(rows)} output_tokens={tokens}")
    print("sample:", " ".join(r["phrase"] for r in rows[:100]))


if __name__ == "__main__":
    main()
