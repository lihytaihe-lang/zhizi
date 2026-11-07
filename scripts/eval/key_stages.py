#!/usr/bin/env python3
"""Per-stage key latency from the local diagnostics log (Q12).

    scripts/eval/key_stages.py [--pid <processInstanceID prefix>] [--kind text] [log ...]

Default logs: ~/Library/Logs/AgentInputMethod/events-*.jsonl.  Without --pid the newest
process instance with stage records is used.  Stage names come from KeyStageTimings:
setup, preceding, process, model, quality, insert, marked, caret, panel.  `process` wraps
the FFI core call (coreElapsed), the model call (`model`) and the Swift-side work in
CoreSession.process, so `process - core - model` is reported as `swift_rest`.
Numbers only; no text content is read.  Local diagnosis, not a gate.
"""
import argparse
import glob
import json
import os
import sys

STAGES = ["setup", "preceding", "process", "model", "quality", "insert", "marked", "caret", "panel"]


def pct(values, p):
    if not values:
        return float("nan")
    values = sorted(values)
    return values[min(len(values) - 1, int(len(values) * p))] / 1e6


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pid", default="")
    parser.add_argument("--kind", default="text")
    parser.add_argument("logs", nargs="*")
    args = parser.parse_args()
    logs = args.logs or sorted(glob.glob(os.path.expanduser("~/Library/Logs/AgentInputMethod/events-*.jsonl")))
    rows = []
    for path in logs:
        with open(path, errors="replace") as handle:
            for line in handle:
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if record.get("stages"):
                    rows.append(record)
    if not rows:
        sys.exit("no stage records found")
    pid = args.pid
    if not pid:
        counts = {}
        for record in rows:
            counts[record["processInstanceID"]] = counts.get(record["processInstanceID"], 0) + 1
        # newest instance that has a meaningful sample
        last = {}
        for record in rows:
            last[record["processInstanceID"]] = record["timestamp"]
        pid = max((p for p in counts if counts[p] >= 50), key=lambda p: last[p], default=max(counts, key=counts.get))
    rows = [r for r in rows if r["processInstanceID"].startswith(pid) and r["eventKind"] == args.kind]
    print(f"process={pid[:8]} kind={args.kind} n={len(rows)}")
    if not rows:
        return
    print(f"  {'total':<10} p50 {pct([r['totalElapsedNanoseconds'] for r in rows], .5):7.2f}  p90 {pct([r['totalElapsedNanoseconds'] for r in rows], .9):7.2f}  p99 {pct([r['totalElapsedNanoseconds'] for r in rows], .99):7.2f}")
    print(f"  {'core':<10} p50 {pct([r['coreElapsedNanoseconds'] for r in rows], .5):7.2f}  p90 {pct([r['coreElapsedNanoseconds'] for r in rows], .9):7.2f}  p99 {pct([r['coreElapsedNanoseconds'] for r in rows], .99):7.2f}")
    for name in STAGES:
        values = [r["stages"].get(name, 0) for r in rows]
        print(f"  {name:<10} p50 {pct(values, .5):7.2f}  p90 {pct(values, .9):7.2f}  p99 {pct(values, .99):7.2f}")
    rest = [max(0, r["stages"]["process"] - r["coreElapsedNanoseconds"] - r["stages"]["model"]) for r in rows]
    print(f"  {'swift_rest':<10} p50 {pct(rest, .5):7.2f}  p90 {pct(rest, .9):7.2f}  p99 {pct(rest, .99):7.2f}   (process - core - model)")
    with_model = [r for r in rows if r["stages"]["model"] > 0]
    without = [r for r in rows if r["stages"]["model"] == 0]
    print(f"  model ran on {len(with_model)}/{len(rows)} keys; total p50 with model {pct([r['totalElapsedNanoseconds'] for r in with_model], .5):.2f} ms, without {pct([r['totalElapsedNanoseconds'] for r in without], .5):.2f} ms")


if __name__ == "__main__":
    main()
