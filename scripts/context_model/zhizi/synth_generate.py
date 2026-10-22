"""Generate colloquial / work-chat / CN-EN mixed Chinese text for pretraining.

Backends are the owner's own CLIs (QoderWork Qwen3.8-Flash, opencode GLM-5.3-flash),
run with tools disabled and stdin closed. Each call asks for ~60 lines in one
(scenario, style) cell; cells are sampled so the corpus covers the owner's real
typing mix rather than whatever the model likes to write.

Output: synth/<run>/<backend>.jsonl, one record per call with the cell and the
cleaned lines. Failures are logged and counted, never retried in a tight loop.
"""

import argparse
import json
import os
import random
import re
import subprocess
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORK_DIR = Path(os.environ.get("ZHIZI_WORK", ROOT / "ime_context_model"))

QODER = "/Applications/QoderWork CN.app/Contents/Resources/bin/qoderclicn"
BACKENDS = {
    "qwen": lambda p: [QODER, "-p", "-m", "Qwen3.8-Flash", "--tools", "", "--", p],
    "glm": lambda p: ["opencode", "run", "--pure", "-m", "zhipuai-coding-plan/glm-5.3-flash", p],
}

JD_ENV = str(ROOT.parent / 'JD_jiedandata/.env')  # DeepSeek key lives here; read, never printed


def deepseek_env():
    env = {}
    for line in open(JD_ENV, encoding="utf-8"):
        if "=" in line and not line.lstrip().startswith("#"):
            k, v = line.rstrip("\n").split("=", 1)
            env[k.strip()] = v.strip().strip('"').strip("'")
    return env


def deepseek_call(prompt, timeout):
    env = deepseek_env()
    body = {"model": env.get("DEEPSEEK_MODEL", "deepseek-flash"),
            "messages": [{"role": "user", "content": prompt}],
            "thinking": {"type": "disabled"}, "temperature": 1.0, "max_tokens": 4000}
    req = urllib.request.Request(env["DEEPSEEK_BASE_URL"].rstrip("/") + "/chat/completions",
                                 data=json.dumps(body).encode("utf-8"), method="POST",
                                 headers={"Authorization": f"Bearer {env['DEEPSEEK_API_KEY']}",
                                          "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        payload = json.load(r)
    return payload["choices"][0]["message"]["content"], payload.get("usage", {})


SCENARIOS = [
    "用 AI 编程工具（Claude、Codex、Kimi、Cursor）改项目代码、写 spec、跑测试、提 PR",
    "给 AI 助手下指令：让它审查项目、整理文档、改方案、解释报错、写提示词",
    "讨论一个输入法/桌面软件产品的功能、体验和版本计划",
    "和客户沟通展会展位、报价、交付时间、合同细节",
    "数据标注/数据集交付项目：进度、质检、模型对比、租 GPU",
    "独立游戏开发：Godot、关卡、美术外包、上架 Steam",
    "朋友之间聊股票、基金、仓位、行情",
    "朋友日常闲聊：吃饭、周末安排、孩子、宠物、吐槽",
    "家人之间聊家务、买东西、看病、接送",
    "小红书/公众号运营：选题、文案、数据、涨粉",
    "出差旅行：订票、酒店、行程变更",
    "团队协作：开会时间、分工、日报周报、请假",
]
STYLES = [
    ("微信私聊", "口语化，像真人边想边打字，可以有省略、语气词、不完整的句子"),
    ("微信群聊", "多人你一句我一句，口语化，偶尔接话、调侃"),
    ("飞书工作消息", "简洁的工作沟通，带点口语"),
    ("给 AI 的指令", "一个人对 AI 助手打字下指令或追问，口语、直接，常带具体文件名、英文术语"),
    ("会议纪要", "书面但不生硬，用字规范，的地得用法正确"),
    ("交付文档段落", "正式书面语，用字和标点规范，的地得用法严格正确"),
]
SPEAKER = re.compile(r"^[\u4e00-\u9fffA-Za-z]{1,4}[:：]\s*")  # group-chat labels like 老周：
NUMBERING = re.compile(r"^\s*(\d+[.、)）]|[-*•·]|[（(]?\d+[)）])\s*")


def build_prompt(scenario, style, hint, n):
    return (f"请写 {n} 条中文文本，场景：{scenario}。文体：{style}，{hint}。\n"
            "要求：内容具体、多样，不要重复句式；自然混用英文产品名和技术词（如果场景里会用到）；"
            "句子长短不一；每条一行；不要编号、不要引号、不要任何解释或标题；"
            "不要出现真实人名、电话、地址、身份证号等个人信息。")


def clean(text):
    out = []
    for line in text.splitlines():
        line = SPEAKER.sub("", NUMBERING.sub("", line).strip()).strip()
        if len(line) >= 4 and re.search(r"[一-鿿]", line) and not line.startswith(("[", ">")):
            out.append(line)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--calls", type=int, required=True, help="total calls across backends")
    ap.add_argument("--workers", default="deepseek:4",
                    help="backend:count pairs (default deepseek:4); glm/qwen output terms forbid release data - internal experiments only")
    ap.add_argument("--lines", type=int, default=60)
    ap.add_argument("--timeout", type=int, default=240)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--target-chars", type=int, default=0,
                    help="stop starting new calls once this many chars are written (0 = run all calls)")
    args = ap.parse_args()
    out_dir = os.path.join(str(WORK_DIR / 'synth'), args.run)
    work_dir = os.path.join(out_dir, "cwd")  # empty dir so the agents never touch a project
    os.makedirs(work_dir, exist_ok=True)
    workers = {k: int(v) for k, v in (p.split(":") for p in args.workers.split(","))}
    rng = random.Random(args.seed)
    jobs = []
    names = [b for b, n in workers.items() for _ in range(n)]
    for i in range(args.calls):
        sc = rng.choice(SCENARIOS)
        st, hint = rng.choice(STYLES)
        jobs.append((names[i % len(names)], sc, st, hint))
    lock = threading.Lock()
    stats = {b: {"ok": 0, "fail": 0, "chars": 0, "seconds": 0.0} for b in workers}
    files = {b: open(os.path.join(out_dir, f"{b}.jsonl"), "a", encoding="utf-8") for b in workers}

    streak = {b: 0 for b in workers}  # consecutive failures; 5 in a row disables the backend

    def run(job):
        backend, sc, st, hint = job
        if streak[backend] >= 5:
            return
        if args.target_chars and sum(v["chars"] for v in stats.values()) >= args.target_chars:
            return
        prompt = build_prompt(sc, st, hint, args.lines)
        t = time.time()
        usage = {}
        try:
            if backend == "deepseek":
                text, usage = deepseek_call(prompt, args.timeout)
                lines = clean(text)
                err = None if lines else "empty"
            else:
                r = subprocess.run(BACKENDS[backend](prompt), cwd=work_dir, stdin=subprocess.DEVNULL,
                                   capture_output=True, text=True, timeout=args.timeout)
                lines = clean(r.stdout) if r.returncode == 0 else []
                err = None if lines else (r.stderr[-300:] or r.stdout[-200:] or f"rc={r.returncode}")
        except subprocess.TimeoutExpired:
            lines, err = [], "timeout"
        except Exception as e:  # network/HTTP errors from the API backend
            lines, err = [], f"{type(e).__name__}: {str(e)[:200]}"
        dt = time.time() - t
        with lock:
            s = stats[backend]
            s["seconds"] += dt
            s["tokens_out"] = s.get("tokens_out", 0) + int(usage.get("completion_tokens", 0) or 0)
            if lines:
                s["ok"] += 1
                s["chars"] += sum(len(x) for x in lines)
                files[backend].write(json.dumps({"scenario": sc, "style": st, "lines": lines},
                                                ensure_ascii=False) + "\n")
                files[backend].flush()
                streak[backend] = 0
            else:
                s["fail"] += 1
                streak[backend] += 1
                print(f"[{backend}] fail: {err}", flush=True)
                if streak[backend] == 5:
                    print(f"[{backend}] 5 consecutive failures - backend disabled", flush=True)
            done = sum(v["ok"] + v["fail"] for v in stats.values())
            if done % 10 == 0:
                print(json.dumps({"done": done, **stats}, ensure_ascii=False), flush=True)

    pools = {b: ThreadPoolExecutor(max_workers=n) for b, n in workers.items()}
    futures = [pools[j[0]].submit(run, j) for j in jobs]
    for f in futures:
        f.result()
    print(json.dumps({"final": stats}, ensure_ascii=False), flush=True)
    with open(os.path.join(out_dir, "stats.json"), "w", encoding="utf-8") as f:
        json.dump(stats, f, ensure_ascii=False, indent=2)


if __name__ == "__main__":
    main()
