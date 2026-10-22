"""Second opinion on the readings of spoken phrases that contain a heteronym character.

pypinyin picks one reading per phrase; where a character has several readings (了 le/liao, 行 xing/hang, 长 chang/zhang)
the pick can be wrong, and a wrong reading means the phrase is only found by typing the wrong syllables. This sends the
phrases (text only, nothing else) to the GLM-5.3-flash CLI in batches and asks for the everyday spoken reading, then
writes the phrases where the two disagree.

  python check_readings.py <agent_spoken.dict.yaml> <out disagreements.tsv> [--workers 2]
"""
import argparse
import os
import re
import subprocess
import sys
import threading
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "zhizi"))
from pypinyin import Style, pinyin  # noqa: E402

TONELESS = re.compile(r"[^a-z ]")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dictionary")
    ap.add_argument("out")
    ap.add_argument("--workers", type=int, default=2)
    ap.add_argument("--batch", type=int, default=60)
    ap.add_argument("--backend", default="glm", choices=["glm", "deepseek"], help="deepseek: the only backend whose terms allow it for the release")
    args = ap.parse_args()
    rows = []
    for line in open(args.dictionary, encoding="utf-8").read().split("\n...\n", 1)[1].splitlines():
        parts = line.split("\t")
        if len(parts) >= 3:
            rows.append((parts[0], parts[1]))
    poly = [(t, r) for t, r in rows if any(len(alts) > 1 for alts in pinyin(t, style=Style.NORMAL, heteronym=True))]
    print(f"{len(rows)} phrases, {len(poly)} with a heteronym", flush=True)
    batches = [poly[i:i + args.batch] for i in range(0, len(poly), args.batch)]
    lock, bad, answered = threading.Lock(), [], [0]

    def ask(batch):
        prompt = ("下面每行是一个中文口语短语。请给出它在日常口语中最自然的读音（拼音，不带声调，音节之间一个空格），"
                  "按『短语<TAB>拼音』输出，每行一条，不要解释、不要编号。\n" + "\n".join(t for t, _ in batch))
        try:
            if args.backend == "deepseek":
                from synth_generate import deepseek_call
                out = deepseek_call(prompt, 300)[0]
            else:
                out = subprocess.run(["opencode", "run", "--pure", "-m", "zhipuai-coding-plan/glm-5.3-flash", prompt],
                                     cwd="/tmp", stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=300).stdout
        except Exception:
            return
        said = {}
        for line in out.splitlines():
            if "\t" in line:
                text, reading = line.split("\t", 1)
                said[text.strip()] = TONELESS.sub("", reading.strip().lower()).strip()
        with lock:
            for t, r in batch:
                if t in said:
                    answered[0] += 1
                    if said[t] != r:
                        bad.append((t, r, said[t]))

    with ThreadPoolExecutor(args.workers) as pool:
        list(pool.map(ask, batches))
    with open(args.out, "w", encoding="utf-8") as fh:
        for t, ours, theirs in sorted(bad):
            fh.write(f"{t}\t{ours}\t{theirs}\n")
    print(f"answered {answered[0]} of {len(poly)}; disagreements {len(bad)}", flush=True)


if __name__ == "__main__":
    main()
