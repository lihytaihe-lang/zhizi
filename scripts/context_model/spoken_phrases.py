"""Spoken-phrase candidates for the phrase layer (MODEL-V3-PLAN / NEXT-ORDER: "口语短语层").

The gap is not words but spoken combinations (你看下, 我刚刚, 捞手机): 78% of the owner's phrases are single
dictionary entries, the missing ~20% are mostly such combinations. This asks the owner's own CLIs (the same
backends as ime_context_model/src/synth_generate.py, tools disabled, empty working directory) for short spoken
combinations per (scenario, pattern) cell, and counts how many independent calls produced each phrase: the
count is the frequency proxy. Only scenario prompts leave the machine; no owner data is read or sent.

  python spoken_phrases.py --run v1 --calls 40 --workers deepseek:4
Output: <root>/phrases/spoken-<run>/<backend>.jsonl (one record per call) and phrases.tsv (phrase, calls, backends).
"""
import argparse
import collections
import json
import os
import random
import re
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

WORK = str(WORK_DIR)
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "zhizi"))
from synth_generate import deepseek_call  # noqa: E402
QODER = "/Applications/QoderWork CN.app/Contents/Resources/bin/qoderclicn"
# "deepseek" is the API backend (terms 4.2(3) allow training other models and building derivative products from its output); the key is
# read by zhizi/synth_generate.py from the owner's JD project .env and never printed.  GLM and Qwen outputs are not allowed for the release.
BACKENDS = {
    "qwen": lambda p: [QODER, "-p", "-m", "Qwen3.8-Flash", "--tools", "", "--", p],
    "glm": lambda p: ["opencode", "run", "--pure", "-m", "zhipuai-coding-plan/glm-5.3-flash", p],
}
SCENARIOS = [
    "微信私聊里朋友之间约饭、约时间、催对方回消息", "家人之间说买东西、做饭、接送孩子、看病",
    "跟同事说工作进度、请对方看文件、改东西、对时间", "给 AI 助手下指令：让它看、改、查、跑、整理、解释",
    "网购、退换货、跟客服和快递沟通", "手机、电脑、软件出了问题时说的话",
    "用 AI 编程工具改代码时对它说的话：看下报错、改一版、跑一下测试、提交、回滚、解释一下",
    "讨论软件产品的功能、体验、版本计划、用户反馈", "和客户沟通报价、交付时间、合同和展会安排", "数据标注、数据集交付、质检和进度同步",
    "独立游戏开发、关卡、美术外包、上架", "小红书和公众号运营、选题、文案、涨粉",
    "吐槽、抱怨、安慰、开玩笑", "出行、订票、打车、问路", "炒股、基金、聊行情", "游戏、追剧、刷视频时聊的话",
]
# Added 2026-10-14 from the A-class ruler (owner: 前一页 / 前一个 were missing): interface and everyday-operation vocabulary.
SCENARIOS_OPS = [
    "软件界面操作：点击、翻页、选择、确认、取消、返回、刷新、展开、收起", "浏览器和网页：打开网页、收藏、下载、登录、刷新、关闭标签页",
    "文档和表格编辑：复制、粘贴、撤销、保存、导出、查找替换、排版", "文件管理：新建文件夹、重命名、移动、删除、压缩、解压、打包",
    "聊天软件操作：发送、撤回、转发、@某人、拉群、发语音、发红包", "手机设置和App：打开、关闭、授权、更新、卸载、清缓存、连WiFi",
    "会议和协作：共享屏幕、静音、举手、记录、对齐、同步、排期", "学习和考试：复习、背书、做题、交作业、查成绩、刷题",
    "健康和就医：挂号、开药、复查、忌口、休息、锻炼", "家庭和亲子：接送、作业、吃饭、洗澡、睡觉、出门",
]
# The three weakest topics of ruler A on 2026-10-14 (games 57%, home and kitchen 66%, study 68%).
SCENARIOS_TOPICS = [
    "打游戏：开黑、上分、副本、装备、技能、组队、掉线、卡顿、充值、抽卡", "追剧和听歌：追更、弹幕、倍速、单曲循环、歌词、综艺、番剧",
    "做饭和买菜：炒菜、焖饭、腌肉、切丝、调味、火候、外卖、买菜、冰箱", "收拾家务：洗衣、晾衣、拖地、倒垃圾、叠被子、收纳、换洗",
    "上课和作业：听讲、笔记、预习、复习、背单词、刷题、交作业、课代表", "考试和成绩：模拟考、查分、错题本、押题、补考、绩点、排名",
]
# Added 2026-10-15 (v9) for the next-weakest ruler-A topics: travel, moods and venting, tech talk, games (more angles), social life, office, health, shopping, money.
SCENARIOS_V9 = [
    "出行和旅行：订机票高铁票、订酒店民宿、行程安排、景点排队、托运行李、退改签、办签证、自驾租车",
    "情绪和吐槽：加班、被甲方折磨、破防、无语、emo、摆烂、想辞职、安慰朋友、自嘲",
    "程序员日常口语：部署上线、回滚、报错、拉代码、合并分支、写文档、排期、需求变更、线上事故、复盘",
    "手游端游主机游戏：新赛季、版本更新、氪金、肝活动、单排双排、翻车、carry、直播、电竞比赛",
    "朋友聚会和恋爱社交：约会、表白、吵架和好、同学群、相亲、邻居、随礼、请客、AA、生日",
    "办公室日常：请假、报销、周报、面试、述职、对表、拉群、开会、加薪、离职交接",
    "健康运动减肥睡眠：健身、跑步、控糖、失眠、体检、拉伸、晨起、熬夜、养生、吃药",
    "直播购物和售后：直播间抢购、优惠券、满减、预售、凑单、退款、差评、补发、七天无理由",
    "钱和日常财务：房贷、社保公积金、报税、定投、记账、还信用卡、转账、红包、分期、理财到期",
    "厨房做菜细节：焯水、爆香、收汁、勾芡、腌制、焖煮、控油、备菜、解冻、剩菜",
    "孩子和家庭琐事：幼儿园、兴趣班、家长群、作业检查、体温、疫苗、周末安排、老人看病",
    "学习进修和考证：网课、刷题、背书、笔记、考研、考公、雅思、论文、答辩、导师",
]
# Added 2026-10-17 (v10) from a bucket audit of ruler A on the DeepSeek layer (games 58%, home and kitchen 67%, travel 71%, tech 74%, study 74%
# first-choice): finer angles inside those buckets plus renting/buying a home and pets.  The example words here are not taken from the ruler.
SCENARIOS_V10 = [
    "团战竞技手游端游（王者、吃鸡、LOL）：开团、集合、绕后、补刀、复活、蹲草、收割、残血、空投、报点",
    "抽卡和养成类游戏：十连、保底、体力、日常任务、副本、满级、强化、礼包、公会、签到",
    "单机游戏和Steam：打折、愿望单、通关、存档、速通、卡关、联机、读档、攻略、手柄",
    "炒菜和灶台细节：翻面、出锅、撒盐、大火小火、盛盘、爆炒、焖熟、刷锅、少放油、勾点水淀粉",
    "家务口头指令：把碗洗了、拖下地、晾衣服、扔垃圾、擦桌子、收拾屋子、烧水、关灯、倒杯水",
    "开车和导航：变道、限速、堵车、加油、停车位、红绿灯、右转、绕路、违章、倒车入库",
    "地铁公交高铁和打车：换乘、刷码、检票、晚点、候车、师傅、尾号、到站、行李、接驾",
    "程序员互相吐槽：祖传代码、改需求、背锅、内存泄漏、线上炸了、熬夜上线、重构、加班、提bug",
    "技术协作口语：对接口、跑通了、重启服务、查日志、发版、灰度、回归测试、提测、联合排查、压测",
    "课堂和复习：划重点、抄笔记、点名、小测、答疑、课代表、错题、预习、自习、打卡",
    "朋友闲聊的网络梗和口头禅：绝绝子、笑死、离谱、扎心、真香、整活、磕到了、太真实了、麻了",
    "夸人、安慰和心疼人：做得真好、辛苦了、别太累、你最棒、心疼你、真贴心、太懂事了、抱抱你",
    "手机和软件操作口令：全选、下拉刷新、双指缩放、滑动解锁、清空记录、绑定手机、验证码、授权、静音",
    "办公沟通：对口径、拉通、抄送、盖章、回执、催一下、归档、开发票、签字、跟进",
    "身体不适和就医：拉肚子、扭到腰、嗓子疼、量体温、挂号、拍片、打针、复诊、过敏、咳嗽",
    "账单和信用卡：账单日、逾期、额度、对账、提现、利息、免息期、分期手续费、绑卡、限额",
    "租房买房和装修：看房、砍价、押金、中介、装修、签约、物业费、水电表、搬家、验房",
    "宠物和遛娃：遛狗、铲屎、猫粮、疫苗、洗澡、驱虫、哄睡、喂奶、绘本、推车",
]
PATTERNS_OPS = [
    "序数和方位词短语（如：前一页、后一个、上一步、下一项、第一个、最后一页、往上翻）",
    "点选类动宾（如：点击确定、选中全部、勾选这个、拖到这里、双击打开、长按保存）",
]
PATTERNS = [
    "动词 + 看下/一下/一会儿/试试（如：你看下、我查一下、先放着）", "副词 + 动词（如：我刚刚、还没来得及、差点忘了）",
    "动词 + 结果补语（如：打开、搞定、弄好、捞出来、翻出来）", "口语固定搭配和语气短语（如：怎么说呢、对了对了、不至于吧）",
    "带宾语的短动宾（如：捞手机、占座、薅羊毛、蹲一个、掐你一个）", "年轻人和网络口语常用说法（不要太冷僻、不要脏话）",
]
NUMBERING = re.compile(r"^\s*(\d+[.、)）]|[-*•·])\s*")
HAN_ONLY = re.compile(r"^[一-鿿]{2,6}$")


def prompt(scenario, pattern, n):
    return (f"请列出 {n} 条中文口语短语，场景：{scenario}。类型：{pattern}。\n"
            "要求：每条是真人打字时会连着输入的 2 到 6 个汉字的片段，不是完整长句；不含标点、数字、英文；"
            "尽量是高频常见的，别写书面语；每条一行；不要编号、不要解释、不要标题；不要出现任何人名。")


def clean(text):
    out = []
    for line in text.splitlines():
        line = NUMBERING.sub("", line.strip()).strip("，。！？、,.!? ")
        if HAN_ONLY.match(line):
            out.append(line)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--calls", type=int, required=True)
    ap.add_argument("--workers", default="deepseek:4",
                    help="backend:count pairs (default deepseek:4); glm/qwen output terms forbid release data - internal experiments only")
    ap.add_argument("--lines", type=int, default=80)
    ap.add_argument("--timeout", type=int, default=240)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--topics", action="store_true", help="only the weak ruler-A topics (games, kitchen, study)")
    ap.add_argument("--v9", action="store_true", help="the twelve v9 scenarios (2026-10-15)")
    ap.add_argument("--v10", action="store_true", help="the eighteen v10 scenarios (2026-10-17)")
    ap.add_argument("--ops", action="store_true", help="only the interface / everyday-operation scenarios and patterns (2026-10-14)")
    args = ap.parse_args()
    out_dir = os.path.join(WORK, "phrases", f"spoken-{args.run}")
    work_dir = os.path.join(out_dir, "cwd")
    os.makedirs(work_dir, exist_ok=True)
    workers = {k: int(v) for k, v in (p.split(":") for p in args.workers.split(","))}
    names = [b for b, n in workers.items() for _ in range(n)]
    rng = random.Random(args.seed)
    scenarios, patterns = (SCENARIOS_OPS, PATTERNS + PATTERNS_OPS) if args.ops else (SCENARIOS, PATTERNS)
    if args.topics:
        scenarios, patterns = SCENARIOS_TOPICS, PATTERNS
    if args.v9:
        scenarios, patterns = SCENARIOS_V9, PATTERNS
    if args.v10:
        scenarios, patterns = SCENARIOS_V10, PATTERNS
    cells = [(sc, pt) for sc in scenarios for pt in patterns]
    rng.shuffle(cells)
    jobs = [(names[i % len(names)],) + cells[i % len(cells)] for i in range(args.calls)]
    lock = threading.Lock()
    streak = {b: 0 for b in workers}
    stats = {b: {"ok": 0, "fail": 0, "phrases": 0} for b in workers}
    files = {b: open(os.path.join(out_dir, f"{b}.jsonl"), "a", encoding="utf-8") for b in workers}

    def run(job):
        backend, sc, pt = job
        if streak[backend] >= 5:
            return
        try:
            if backend == "deepseek":
                for attempt in range(4):  # a dropped connection is not a failed call: retry before counting it
                    try:
                        text, _usage = deepseek_call(prompt(sc, pt, args.lines), args.timeout)
                        break
                    except Exception:
                        if attempt == 3:
                            raise
                        time.sleep(5 * (attempt + 1))
                lines = clean(text)
            else:
                r = subprocess.run(BACKENDS[backend](prompt(sc, pt, args.lines)), cwd=work_dir, stdin=subprocess.DEVNULL,
                                   capture_output=True, text=True, timeout=args.timeout)
                lines = clean(r.stdout) if r.returncode == 0 else []
        except Exception:  # timeout / missing binary: counted as a failure
            lines = []
        with lock:
            if lines:
                stats[backend]["ok"] += 1
                stats[backend]["phrases"] += len(lines)
                streak[backend] = 0
                files[backend].write(json.dumps({"scenario": sc, "pattern": pt, "lines": lines}, ensure_ascii=False) + "\n")
                files[backend].flush()
            else:
                stats[backend]["fail"] += 1
                streak[backend] += 1
            print(json.dumps(stats, ensure_ascii=False), flush=True)

    pools = {b: ThreadPoolExecutor(max_workers=n) for b, n in workers.items()}
    for f in [pools[j[0]].submit(run, j) for j in jobs]:
        f.result()
    seen = collections.defaultdict(lambda: [0, set()])
    for b in workers:
        for line in open(os.path.join(out_dir, f"{b}.jsonl"), encoding="utf-8"):
            for phrase in set(json.loads(line)["lines"]):
                seen[phrase][0] += 1
                seen[phrase][1].add(b)
    with open(os.path.join(out_dir, "phrases.tsv"), "w", encoding="utf-8") as fh:
        for phrase, (n, bs) in sorted(seen.items(), key=lambda kv: (-kv[1][0], kv[0])):
            fh.write(f"{phrase}\t{n}\t{','.join(sorted(bs))}\n")
    print(json.dumps({"final": stats, "distinct": len(seen)}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
