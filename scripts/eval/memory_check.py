#!/usr/bin/env python3
"""Memory trend of the installed input method from its local diagnostics (weekly review).

    scripts/eval/memory_check.py [--footprint-max 350] [--resident-max 600] [log ...]

Reads the `memory footprintMB=… residentMB=…` events (every 30 minutes, at start and before
sleep).  For each process instance prints samples, first/last/max and the growth over the
instance's life after the first hour (model load and first typing are expected to add
~100 MB early).  Exit 1 if the newest instance exceeds a ceiling or grows more than
--growth-max MB after the first hour.  Numbers only.  Ceilings: owner 11-01 measurement
(0.3.9, 8 h of typing: footprint flat at 225 MB, resident 154-170 MB) with headroom.
"""
import argparse
import glob
import json
import os
import re
import sys
from datetime import datetime

PATTERN = re.compile(r"footprintMB=(\d+) residentMB=(\d+)")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--footprint-max", type=int, default=350)
    parser.add_argument("--resident-max", type=int, default=600)
    parser.add_argument("--growth-max", type=int, default=60)
    parser.add_argument("logs", nargs="*")
    args = parser.parse_args()
    logs = args.logs or sorted(glob.glob(os.path.expanduser("~/Library/Logs/AgentInputMethod/events-*.jsonl")))
    instances = {}
    for path in logs:
        with open(path, errors="replace") as handle:
            for line in handle:
                if '"memory footprintMB' not in line:
                    continue
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                match = PATTERN.search(record.get("eventKind", ""))
                if match:
                    when = datetime.strptime(record["timestamp"], "%Y-%m-%dT%H:%M:%SZ")
                    instances.setdefault(record["processInstanceID"], []).append(
                        (when, int(match.group(1)), int(match.group(2))))
    if not instances:
        sys.exit("no memory samples found")
    ordered = sorted(instances.items(), key=lambda item: item[1][0][0])
    failed = False
    for index, (pid, samples) in enumerate(ordered):
        samples.sort()
        start = samples[0][0]
        late = [s for s in samples if (s[0] - start).total_seconds() >= 3600]
        growth = (late[-1][1] - late[0][1]) if len(late) >= 2 else 0
        peak_fp = max(s[1] for s in samples)
        peak_rs = max(s[2] for s in samples)
        hours = (samples[-1][0] - start).total_seconds() / 3600
        print(f"{pid[:8]} samples={len(samples)} span={hours:.1f}h footprint {samples[0][1]}->{samples[-1][1]} (max {peak_fp}) "
              f"resident {samples[0][2]}->{samples[-1][2]} (max {peak_rs}) growth_after_1h={growth} MB")
        if index == len(ordered) - 1:
            if peak_fp > args.footprint_max or peak_rs > args.resident_max or growth > args.growth_max:
                print(f"OVER: footprint<={args.footprint_max} resident<={args.resident_max} growth<={args.growth_max}")
                failed = True
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
