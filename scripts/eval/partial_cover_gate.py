#!/usr/bin/env python3
"""Partial-cover gate on the Zhixu path (replaces the Rime-userdb-based gate for G8).

    scripts/eval/partial_cover_gate.py [--sample <cases.jsonl>] [--baseline <json>] [--write-baseline] [label]

Runs scripts/eval/run_eval.sh in `live` mode (a copy of the owner's Rime user directory,
which holds agent_zhixu_learning_v1.json) with ZHIZI_ZHIXU_WHOLE=1, ZHIZI_LEARNING=zhixu and
ZHIZI_EVAL_REQUIRE_DECODER=zhixu, so the decoder under test is Zhixu with the owner's own
learning, never Rime.  Two parts:

  A. the nine frozen cases of quality/corpus/partial-coverage-top1-v1.json: the first
     candidate must be in `allowed`.
  B. the class, on the frozen owner sample (default eval-runs/owner-replay-v1/cases.jsonl):
     cases where the first candidate is shorter than the target, the target is on the first
     page and the first candidate is a prefix of the target ("half the input beat the
     whole").  The count must not exceed the recorded baseline, and the cases whose first
     candidate was right at baseline may not turn into this class (broken = 0).

Build first: (cd native/host && cargo build --release --offline --example eval).
Needs ZHIZI_SHARED if build/native/release has no bundle.  Local only: owner data.
"""
import argparse
import json
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CORPUS = os.path.join(ROOT, "quality/corpus/partial-coverage-top1-v1.json")


def run(label, cases_path, out_dir):
    env = dict(os.environ, ZHIZI_EVAL_OUT=out_dir, ZHIZI_ZHIXU_WHOLE="1", ZHIZI_LEARNING="zhixu",
               ZHIZI_EVAL_REQUIRE_DECODER="zhixu")
    records = os.path.expanduser("~/Library/Application Support/AgentInputMethod/Quality/events.jsonl")
    if os.path.isfile(records):
        env.setdefault("ZHIZI_OWNER_RECORDS", records)
    log = os.path.join(out_dir, f"run-{label}.log")
    with open(log, "w") as handle:
        status = subprocess.call([os.path.join(ROOT, "scripts/eval/run_eval.sh"), label, cases_path, "live"],
                                 env=env, stdout=handle, stderr=subprocess.STDOUT)
    if status != 0:
        sys.exit(f"eval failed ({status}); see {log}")
    with open(os.path.join(out_dir, f"out-{label}.jsonl")) as handle:
        return [json.loads(line) for line in handle if line.strip()]


def is_half(row):
    page = row.get("page") or []
    target = row.get("target", "")
    first = page[0] if page else ""
    return bool(first) and first != target and target in page and len(first) < len(target) and target.startswith(first)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--sample", default=os.path.join(ROOT, "eval-runs/owner-replay-v1/cases.jsonl"))
    parser.add_argument("--baseline", default=os.path.join(ROOT, "eval-runs/partial-cover-baseline.json"))
    parser.add_argument("--write-baseline", action="store_true")
    parser.add_argument("label", nargs="?", default="partial-cover")
    args = parser.parse_args()
    out_dir = os.path.join(ROOT, "eval-runs/partial-cover")
    os.makedirs(out_dir, exist_ok=True)

    corpus = json.load(open(CORPUS))["cases"]
    frozen = os.path.join(out_dir, "frozen-cases.jsonl")
    with open(frozen, "w") as handle:
        for case in corpus:
            handle.write(json.dumps({"id": case["id"], "bucket": "frozen", "context": "", "keys": case["query"],
                                     "target": case["allowed"][0]}, ensure_ascii=False) + "\n")
    rows = run(args.label + "-frozen", frozen, out_dir)
    allowed = {case["id"]: case["allowed"] for case in corpus}
    failed_a = []
    for row in rows:
        first = (row.get("page") or [""])[0]
        if first not in allowed[row["id"]]:
            failed_a.append((row["id"], row["keys"], first))
    print(f"partial_cover_frozen={'PASS' if not failed_a else 'FAIL'} cases={len(rows)} failed={len(failed_a)}")
    for item in failed_a:
        print("  FAIL", *item)

    sample_rows = run(args.label + "-sample", args.sample, out_dir)
    half = {row["id"] for row in sample_rows if is_half(row)}
    firsts = {row["id"] for row in sample_rows if row.get("first")}
    print(f"partial_cover_class cases={len(sample_rows)} half_beats_whole={len(half)}")
    if args.write_baseline:
        json.dump({"half": sorted(half), "firsts": sorted(firsts), "cases": len(sample_rows)}, open(args.baseline, "w"))
        print(f"baseline written to {args.baseline}")
    elif os.path.isfile(args.baseline):
        base = json.load(open(args.baseline))
        broken = sorted(set(base["firsts"]) & half)
        fixed = sorted(set(base["half"]) - half)
        print(f"  vs baseline: count {len(base['half'])} -> {len(half)}, fixed={len(fixed)}, broken(was first, now half)={len(broken)}")
        if len(half) > len(base["half"]) or broken:
            print("partial_cover_class=FAIL")
            sys.exit(1)
        print("partial_cover_class=PASS")
    else:
        print("no baseline yet; rerun with --write-baseline on a known-good commit")
    sys.exit(1 if failed_a else 0)


if __name__ == "__main__":
    main()
