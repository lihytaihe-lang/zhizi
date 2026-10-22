"""Ruler A of VOCAB-COVERAGE-PLAN: everyday words and short phrases, collected independently of the spoken layer.

The spoken layer is built from Qwen3.8-Flash / GLM-5.3-flash generations; to avoid grading it on its own homework this asks a
different model (GLM-5.3, the full one) with differently worded category prompts, and the result is NEVER added to any
dictionary. Phrases are Han-only, 2-8 characters; each category call asks for 70 items.

  python vocab_ruler_a.py --out <repo>/ime_context_model/vocab/ruler-a.tsv [--calls-per-category 2]
Output TSV: phrase<TAB>category (deduplicated, in first-seen order).
"""
import argparse
import re
import subprocess
from concurrent.futures import ThreadPoolExecutor

CATEGORIES = {
    "界面和软件操作": "点击、翻页、设置、保存、导出、撤销、刷新、登录这类软件界面和操作里的常用词与短语（如：前一页、返回上一步、全部选中）",
    "办公和文档": "写文档、做表格、开会、发邮件、审批、排期、对表时的常用词和短语",
    "编程和技术日常": "程序员日常口头和打字里的词和短语（如：跑通了、回滚一下、加个断点），不含英文",
    "网购和快递": "下单、退款、物流、客服沟通里的常用词和短语",
    "出行和旅行": "打车、订票、导航、住宿、问路里的常用词和短语",
    "餐饮和家务": "点餐、做饭、买菜、收拾、洗衣这类日常生活里的词和短语",
    "社交闲聊": "朋友之间微信聊天的常用说法、语气短语、接话",
    "情绪和吐槽": "抱怨、安慰、开玩笑、夸人、表达无奈的常用说法",
    "健康和就医": "挂号、看病、吃药、休息、锻炼里的常用词和短语",
    "学习和考试": "上课、作业、复习、考试、成绩里的常用词和短语",
    "金融和理财": "转账、基金、股票、报销、账单里的常用词和短语",
    "游戏和娱乐": "打游戏、追剧、看视频、听歌里的常用词和短语",
}
HAN = re.compile(r"^[一-鿿]{2,8}$")
NUMBERING = re.compile(r"^\s*(\d+[.、)）]|[-*•·])\s*")


def ask(category, description, call):
    prompt = (f"请列出 70 条中文词语或短语，主题：{category}，{description}。\n"
              "要求：真人日常会输入的、常见的、不冷僻；每条 2 到 8 个汉字；不含标点、数字、英文；"
              f"这是第 {call + 1} 次，请换一批不同的写法；每行一条，不要编号，不要解释。")
    try:
        out = subprocess.run(["opencode", "run", "--pure", "-m", "zhipuai-coding-plan/glm-5.3", prompt], cwd="/tmp",
                             stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=900).stdout
    except subprocess.TimeoutExpired:
        return []  # one slow call must not lose the others
    lines = [NUMBERING.sub("", line.strip()).strip("，。！？、 ") for line in out.splitlines()]
    return [line for line in lines if HAN.match(line)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--calls-per-category", type=int, default=2)
    ap.add_argument("--workers", type=int, default=3)
    args = ap.parse_args()
    jobs = [(c, d, i) for c, d in CATEGORIES.items() for i in range(args.calls_per_category)]
    with ThreadPoolExecutor(args.workers) as pool:
        results = list(pool.map(lambda job: (job[0], ask(*job)), jobs))
    seen, rows = set(), []
    for category, phrases in results:
        for phrase in phrases:
            if phrase not in seen:
                seen.add(phrase)
                rows.append((phrase, category))
    with open(args.out, "w", encoding="utf-8") as fh:
        for phrase, category in rows:
            fh.write(f"{phrase}\t{category}\n")
    print(f"{len(rows)} phrases from {len(jobs)} calls")


if __name__ == "__main__":
    main()
