# -*- coding: utf-8 -*-
"""Test cases from the testers' uploaded quality records (local only; for EVALUATION, never for training - owner 2026-10-17).

  ime_context_model/.venv/bin/python scripts/eval/build_friends_set.py --data tmp/zhizi-data --out eval-runs/friends.jsonl [--exclude-device ID ...]

--data is the folder `rsync` pulls from the log server (one folder per device, one JSONL per day).  One test case per committed
Chinese candidate: keys = the letters typed, target = the committed text, context = the text before the caret (last 32 characters).
Buckets are anonymous (friend-1, friend-2 ... in device-folder order); commits the user deleted and retyped right away go to
`<bucket>-redo` because their committed text is probably not what was meant.  Rows with mail / URL / long digit strings, secure-field
markers or no usable context marker are skipped.  Nothing is printed except counts; the output file stays on this machine.
"""
import argparse
import glob
import json
import os
import re

HAN = re.compile(r"^[一-鿿]{1,30}$")
SENSITIVE = re.compile(r"@|https?:|www\.|\d{6,}|[A-Za-z0-9_\-]{20,}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--exclude-device", nargs="*", default=[], help="device-id prefixes to leave out (the owner's own device)")
    args = ap.parse_args()
    cases, seen, counts = [], set(), {}
    devices = [d for d in sorted(glob.glob(os.path.join(args.data, "*/"))) if not any(os.path.basename(d.rstrip("/")).startswith(x) for x in args.exclude_device)]
    for n, directory in enumerate(devices, 1):
        bucket = f"friend-{n}"
        for path in sorted(glob.glob(os.path.join(directory, "*.jsonl"))):
            for line in open(path, encoding="utf-8"):
                try:
                    e = json.loads(line)
                except ValueError:
                    continue
                if e.get("outcome") != "committed_candidate":
                    continue
                text = e.get("committedText") or e.get("selectedText") or ""
                letters = re.sub(r"[^a-z]", "", (e.get("pinyin") or "").lower())
                context = e.get("precedingText") or ""
                if not HAN.match(text) or len(letters) < 1 or "[已隐藏]" in context or SENSITIVE.search(context + text + letters):
                    continue
                context = context[-32:]
                key = (bucket, letters, text, context)
                if key in seen:
                    continue
                seen.add(key)
                b = bucket + ("-redo" if e.get("redoAfterCommit") else "")
                counts[b] = counts.get(b, 0) + 1
                cases.append({"id": f"{b}-{counts[b]:04d}", "bucket": b, "source": "commit", "context": context, "keys": letters, "target": text})
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as fh:
        for c in cases:
            fh.write(json.dumps(c, ensure_ascii=False) + "\n")
    os.chmod(args.out, 0o600)
    print(len(cases), "cases", counts)


if __name__ == "__main__":
    main()
