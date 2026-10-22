"""Names of everyday things, category by category, to fill the gaps of the long tail (柴犬 and 白兔 were in no dictionary - owner 2026-10-17).

  ime_context_model/.venv/bin/python scripts/lexicon/gen_category_words.py --out <dir> [--calls 3] [--workers 6] [--limit N categories]

For every one of the fine categories below DeepSeek lists the names `--calls` times (independent calls, so a name that comes up again
is a name people really use).  A name is a candidate when it is in no shipped dictionary, every syllable of its reading is a valid reading of
its character, and it came up in at least two calls over all categories.  Three review passes (reshuffled batches) then drop what
two of them call invented, obscure or ancient.  Output: <out>/new_words.tsv in the format export_new_words.py reads
(text, reading, year = category group, domain = category, wiki, synth, deleted_votes).
"""
import argparse
import json
import os
import random
import re
import sys
import time
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "context_model", "zhizi"))
from build_general_lexicon import existing_entries  # noqa: E402
from gen_new_words import HAN, presence, reading_ok  # noqa: E402
from synth_generate import deepseek_call  # noqa: E402

# Added 2026-10-17: 是的, 对吧, 对的 were in no dictionary (owner: typing `sd` gave 说到, not 是的).  Dictionaries split 「是的」 into 是 / 的 and the
# spoken layer skipped two-character items, so the commonest colloquial replies and 「X的 / X吧」 compounds were missing.  Weights for this group
# come from calibrate_weights.py (corpus frequency on the shipped scale), not from the flat category weight.
COLLOQUIAL = {
    "口语": ["单独成句的回复和应答（是的、对的、好的、行吧、没问题、收到、懂了、明白）", "句末语气词组合（对吧、是吧、好吧、行吧、对不对、是不是、好不好）",
             "「的」字短语和口语里的 X 的（是的、对的、假的、真的、新的、旧的、大的、小的、我的、你的）", "口头禅和填充语（那个、就是说、怎么说呢、你知道吧、说白了）",
             "客气话和礼貌用语（谢谢、不客气、麻烦了、辛苦了、打扰了、抱歉）", "感叹和反应（哇塞、我去、天哪、不会吧、真的假的、太棒了）",
             "表示同意、反对、犹豫、拒绝的短语", "聊天里的开场和结束语（在吗、忙吗、晚安、回聊、先这样）"],
}
GROUPS = {
    "动物": ["宠物狗的品种", "宠物猫的品种", "常见宠物（兔、仓鼠、鸟、鱼、龟等）及其品种和毛色叫法", "农场和家养动物", "野生哺乳动物", "鸟类", "鱼类和海洋生物", "昆虫和小虫子", "爬行动物和两栖动物", "恐龙和史前生物", "动物园里的动物", "卡通和玩偶里常见的动物形象叫法"],
    "植物": ["花卉", "树木", "蔬菜", "水果", "坚果和干果", "菌菇", "中药材和草药", "盆栽和多肉植物", "粮食作物和杂粮", "香料和调味植物"],
    "食物": ["中餐家常菜", "各地小吃和早餐", "面食和主食", "火锅和烧烤食材", "甜品和糕点", "饮品、奶茶和咖啡的名称", "零食和糖果", "肉类和海鲜食材", "调味品和酱料", "西餐和日料、韩餐的菜名", "水果和蔬菜的做法与吃法", "酒类和酒的品种", "茶叶品种"],
    "物品": ["厨房用具和餐具", "家具和家居用品", "家用电器", "衣服和裤子的款式", "鞋帽、包和配饰", "首饰和珠宝", "化妆品和护肤品", "文具和办公用品", "工具和五金", "玩具和游戏机", "运动器材和装备", "乐器", "数码产品和配件", "电脑硬件和外设", "手机功能和应用里的按钮、菜单名称", "日用品和洗护用品", "母婴用品", "交通工具和车辆部件", "旅行用品和户外装备", "宠物用品", "医疗器械和药品剂型", "布料和材质"],
    "人": ["职业和工种", "亲属称呼和人际关系称呼", "互联网和职场里的人物角色称呼", "各种爱好者和某某党某某族的叫法", "学校里的身份、职务和学科", "公司里的岗位和部门", "医生的科室和医护岗位", "法律和政务里的角色和机构"],
    "身体与健康": ["人体部位和器官", "常见症状和不适", "常见疾病和病名", "常用药品的名称", "体检和检查项目", "运动损伤和康复", "营养成分和保健品", "心理状态和情绪词"],
    "地方": ["中国的省市和地级市", "中国的县和区（常被提到的）", "中国的著名景点和山水", "世界各国和首都", "世界著名城市和景点", "城市里的地点：商场、医院、学校、车站、公园的常见类别名", "住宅小区、房屋类型和装修相关的地方名称", "街道、道路和交通设施的叫法", "中国的民族和方言", "自然地理：地形、气象、天体和自然现象"],
    "生活": ["家务和家居动作的短语", "烹饪方法和厨房动作", "购物和快递相关的词", "银行、支付和理财相关的词", "租房买房装修相关的词", "出行、交通和旅行相关的词", "节日、习俗和日期的叫法", "婚礼、生日、丧葬等人生礼仪的词", "宠物饲养和护理相关的词", "育儿和教育相关的词", "健身和减肥相关的词", "穿搭和美妆相关的词"],
    "学习与工作": ["各学科的常用术语（数学、物理、化学、生物）", "语文、历史、地理、政治的常用词", "编程和软件开发常用词", "办公软件和文档处理常用词", "人工智能和数据常用词", "职场会议和项目管理常用词", "财务会计和税务常用词", "法律和合同常用词", "医学和护理常用词", "建筑、机械和工程常用词", "外贸、物流和电商常用词", "考试、升学和留学常用词"],
    "文化娱乐": ["电影和电视剧类型与行业词", "音乐类型、乐队和演唱会相关词", "游戏类型和游戏里的常用词", "动漫、漫画和二次元常用词", "体育项目和赛事", "球类运动的术语", "桌游、棋牌和麻将的词", "摄影、绘画和手工的词", "网络小说和短视频直播的词", "古诗词和历史上的常见称呼（现代人仍常提）", "神话传说和民间故事里的名字", "星座、生肖和命理相关词", "中国传统文化和节气", "综艺和娱乐圈常用词"],
    "社会": ["新闻里常见的机构和政策词", "政府部门和公共服务", "交通出行的规则和证件手续", "社保、医保、公积金和税务", "教育体制相关词", "互联网平台和常见的 App 名称", "常见品牌和连锁店（餐饮、零售、服饰）", "常见的汽车品牌和车型叫法", "手机、电脑和电器的品牌与系列", "著名企业和公司（国内外）", "国际组织、条约和国际时事常用词", "军事和国防常用词（新闻里常见的）"],
}
PROMPT = """请尽量完整地列出「{category}」里常见的名称或说法，最多 150 个。要求：现代中国人日常会提到、会在输入法里打出来；包括常见的品种、型号、别称和口语叫法；不要编造；不要太冷僻或古旧；
纯汉字，2 到 6 个字（含英文字母和数字的不要）。每行一个，格式：词=拼音（音节用空格隔开，不标声调，ü 写作 v），不要编号、不要解释。"""
JUDGE = """下面每行是“词<TAB>类别”，是输入法候选的当代常用名称。请找出应当删除的：编造的、不存在的、几乎没人用的、生僻的、古旧或文言的、意思不通的、只是普通短句或片段而不是一个词的。
现在日常常用的名称和说法要保留。严格按格式输出，没有就写“无”，不要解释：
删除：词1、词2、……

{items}"""


def call(prompt, timeout):
    for attempt in range(4):
        try:
            return deepseek_call(prompt, timeout)
        except Exception:
            time.sleep(5 * (attempt + 1))
    return "", {}


def generate(categories, calls, workers):
    jobs = [(group, category, k) for group, category in categories for k in range(calls)]

    def run(job):
        group, category, k = job
        text, usage = call(PROMPT.format(category=category), 360)
        rows = []
        for line in text.splitlines():
            if "=" in line:
                w, p = line.split("=", 1)
                rows.append((w.strip().strip("0123456789.、-* "), " ".join(p.strip().lower().replace("ü", "v").split())))
        return job, rows

    out = []
    with ThreadPoolExecutor(workers) as pool:
        for n, (job, rows) in enumerate(pool.map(run, jobs), 1):
            out += [(w, p, job[0], job[1], job[2]) for w, p in rows]
            if n % 20 == 0:
                print(f"generated {n}/{len(jobs)} calls, {len(out)} rows", flush=True)
    return out


def judge(rows, passes, workers, batch=120):
    deleted = Counter()
    for k in range(passes):
        order = rows[:]
        random.Random(300 + k).shuffle(order)
        batches = [order[i:i + batch] for i in range(0, len(order), batch)]

        def run(b):
            text, _ = call(JUDGE.format(items="\n".join(f"{r['text']}\t{r['domain']}" for r in b)), 300)
            if not text:
                return set()  # a failed batch deletes nothing: the words were produced by two independent calls already
            out = set()
            for line in text.splitlines():
                if line.strip().startswith("删除"):
                    body = re.split(r"[：:]", line, 1)[-1]
                    out |= {x.strip() for x in re.split(r"[、,，]", body) if x.strip() and x.strip() != "无"}
            return out

        with ThreadPoolExecutor(workers) as pool:
            for part in pool.map(run, batches):
                for w in part:
                    deleted[w] += 1
        print(f"judge pass {k + 1} done", flush=True)
    return deleted


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--calls", type=int, default=3)
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--limit", type=int, default=0, help="only the first N categories (trial)")
    ap.add_argument("--min-calls", type=int, default=2)
    ap.add_argument("--sure-calls", type=int, default=3, help="a name that came up in this many independent calls skips the review (it "
                    "dropped 狗窝, 德牧, 上坟, 鞋套 and 白兔, all real: two passes out of three call short compounds 'ordinary phrases')")
    ap.add_argument("--groups", default="", help="comma-separated group names to run (default: every group of GROUPS); 口语 is in COLLOQUIAL")
    ap.add_argument("--from-raw", action="store_true", help="reuse <out>/raw.json instead of generating again")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    pool = {**GROUPS, **COLLOQUIAL}
    chosen = [g for g in args.groups.split(",") if g] or list(GROUPS)
    categories = [(g, c) for g in chosen for c in pool[g]]
    if args.limit:
        categories = categories[:args.limit]
    print(f"{len(categories)} categories x {args.calls} calls", flush=True)
    if args.from_raw:
        raw = [tuple(r) for r in json.load(open(os.path.join(args.out, "raw.json")))]
    else:
        raw = generate(categories, args.calls, args.workers)
        json.dump(raw, open(os.path.join(args.out, "raw.json"), "w"), ensure_ascii=False)

    shipped = set(existing_entries())
    stats, calls_of, reading_votes, where = Counter(), defaultdict(set), defaultdict(Counter), {}
    for w, p, group, category, k in raw:
        if not HAN.match(w) or len(w) > 6:
            stats["not_han_2_6"] += 1
        elif w in shipped:
            stats["already_shipped"] += 1
        elif not reading_ok(w, p):
            stats["bad_reading"] += 1
        else:
            calls_of[w].add((category, k))
            reading_votes[w][p] += 1
            where.setdefault(w, (group, category))
    rows = [{"text": w, "reading": reading_votes[w].most_common(1)[0][0], "year": where[w][0], "domain": where[w][1], "calls": len(calls_of[w])}
            for w in calls_of if len(calls_of[w]) >= args.min_calls]
    print(f"candidates={len(rows)} (of {len(calls_of)} new names; {dict(stats)})", flush=True)
    wiki, synth = presence({r["text"] for r in rows})
    deleted = judge(rows, 3, args.workers)
    kept = [r for r in rows if deleted[r["text"]] < 2 or r["calls"] >= args.sure_calls]
    with open(os.path.join(args.out, "new_words.tsv"), "w", encoding="utf-8") as fh:
        fh.write("text\treading\tyear\tdomain\twiki\tsynth\tdeleted_votes\n")
        for r in sorted(kept, key=lambda r: (r["domain"], r["text"])):
            fh.write(f"{r['text']}\t{r['reading']}\t{r['year']}\t{r['domain']}\t{wiki[r['text']]}\t{synth[r['text']]}\t{deleted[r['text']]}\n")
    summary = {"categories": len(categories), "raw": len(raw), "new_names": len(calls_of), "candidates": len(rows), "kept": len(kept),
               "filter": stats, "by_group": Counter(r["year"] for r in kept)}
    json.dump(summary, open(os.path.join(args.out, "summary.json"), "w"), ensure_ascii=False, indent=1)
    print(json.dumps(summary, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
