"""Run pool/replay with isolated blank profiles and emit normalized ranking JSONL."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

if __package__:
    from . import report
else:
    import report


def load_cases(path):
    rows = []
    with Path(path).open(encoding="utf-8") as stream:
        for number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            try:
                row = json.loads(line)
            except ValueError as exc:
                raise ValueError(f"case line {number}: invalid JSON") from exc
            if not isinstance(row, dict):
                raise ValueError(f"case line {number}: expected object")
            # Reuse artifact validation with an empty synthetic result, without reading any model.
            case = {field: row.get(field) for field in ("id", *report.CASE_FIELDS)}
            if not isinstance(case["raw"], str) or not re.fullmatch("[a-z]{2,}", case["raw"]):
                raise ValueError(f"case line {number}: raw must contain at least two lowercase ASCII letters")
            rows.append(case)
    if not rows:
        raise ValueError("cases must not be empty")
    report.validate_records([dict(row, candidates=[], rank_all=-1, model_ready=False, model_ms=0) for row in rows])
    return rows


def case_key(row):
    return row["raw"], row["context"], row["target"]


def write_jsonl(path, rows):
    with path.open("w", encoding="utf-8") as stream:
        for row in rows:
            stream.write(json.dumps(row, ensure_ascii=False, allow_nan=False) + "\n")


def run_shard(mode, number, cases, args, env):
    directory = args.work_dir / f"{mode}-{number}"
    directory.mkdir()
    user = directory / "user"
    shutil.copytree(args.blank_user_dir, user)
    events = directory / "events.jsonl"
    output = directory / "output.jsonl"
    write_jsonl(events, [{"pinyin": row["raw"], "precedingText": row["context"],
                         "selectedText": row["target"]} for row in cases])
    binary = args.pool_bin if mode == "pool" else args.replay_bin
    with (directory / "process.log").open("wb") as log:
        done = subprocess.run([str(binary), str(args.shared_dir), str(user), str(events), str(output)],
                              env=env, stdout=log, stderr=subprocess.STDOUT)
    if done.returncode:
        raise ValueError(f"{mode} shard {number} exited {done.returncode}; inspect its process.log")
    expected = {case_key(row) for row in cases}
    results = {}
    with output.open(encoding="utf-8") as stream:
        for line_number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            try:
                row = json.loads(line)
                key = row["raw"], row["context"], row["chosen"]
                if key not in expected or key in results:
                    raise ValueError("unexpected or duplicate result")
                # A stale binary missing the new fields must fail, not inflate the denominator.
                result = dict(row, id=str(line_number), bucket="full", target=row["chosen"], source="ext")
                report.validate_records([result])
            except (ValueError, KeyError, TypeError) as exc:
                raise ValueError(f"{mode} shard {number} line {line_number}: invalid engine result") from exc
            results[key] = row
    if set(results) != expected:
        raise ValueError(f"{mode} shard {number}: missing engine results")
    return results


def run_bench(args):
    if not 1 <= args.jobs <= 3 or args.wait_ms < 0:
        raise ValueError("jobs must be 1..3 and wait-ms must be nonnegative")
    rows = load_cases(args.cases)
    weights = report.load_weights(args.weights)
    for directory in (args.shared_dir, args.blank_user_dir):
        if not directory.is_dir():
            raise ValueError("shared-dir and blank-user-dir must be existing directories")
    # Reject a run directory inside either source tree before recursively copying a profile.
    work = args.work_dir.resolve()
    for directory in (args.shared_dir, args.blank_user_dir):
        if work == directory.resolve() or directory.resolve() in work.parents:
            raise ValueError("work-dir must be outside shared-dir and blank-user-dir")
    env = os.environ.copy()
    env["ZHIZI_MIN_KEYS"] = "2"
    env["ZHIZI_WAIT_MS"] = str(args.wait_ms)
    for option, variable in ((args.ort, "ZHIZI_ORT"), (args.model, "ZHIZI_MODEL"), (args.itos, "ZHIZI_ITOS")):
        if option is not None:
            env[variable] = option
    if args.mode != "pool" and any(not env.get(key) for key in ("ZHIZI_ORT", "ZHIZI_MODEL", "ZHIZI_ITOS")):
        raise ValueError("replay requires --ort/--model/--itos or ZHIZI_ORT/ZHIZI_MODEL/ZHIZI_ITOS")
    # Keep existing engine de-duplication from dropping independent labeled cases.
    unique = list({case_key(row): row for row in rows}.values())
    count = min(args.jobs, len(unique))
    shards = [unique[index::count] for index in range(count)]
    args.work_dir = work
    args.work_dir.mkdir(parents=True, exist_ok=False)
    modes = ("pool", "replay") if args.mode == "both" else (args.mode,)
    reports = {}
    exit_code = 0
    for mode in modes:
        child_env = env.copy()
        if mode == "pool":
            for key in ("ZHIZI_ORT", "ZHIZI_MODEL", "ZHIZI_ITOS"):
                child_env.pop(key, None)
        with ThreadPoolExecutor(max_workers=count) as executor:
            futures = [executor.submit(run_shard, mode, number, shard, args, child_env)
                       for number, shard in enumerate(shards)]
            results = {}
            for future in futures:
                results.update(future.result())
        artifact = [dict(results[case_key(row)], **row) for row in rows]
        summary = report.summarize(artifact, weights)
        partial = args.work_dir / f"{mode}.partial.jsonl"
        write_jsonl(partial, artifact)
        partial.replace(args.work_dir / f"{mode}.jsonl")
        text = report.render_report(summary, require_model=mode == "replay")
        (args.work_dir / f"{mode}.report.txt").write_text(text, encoding="utf-8")
        reports[mode] = text
        exit_code = max(exit_code, report.report_exit_code(summary, require_model=mode == "replay"))
    # Model health must be the very first line even when both modes ran.
    for mode in reversed(modes):
        print(reports[mode], end="")
    return exit_code


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--shared-dir", type=Path, required=True)
    parser.add_argument("--blank-user-dir", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True, help="new directory for this run")
    parser.add_argument("--mode", choices=("pool", "replay", "both"), default="both")
    parser.add_argument("--jobs", type=int, choices=(1, 2, 3), default=1)
    parser.add_argument("--wait-ms", type=int, default=os.environ.get("ZHIZI_WAIT_MS", "3000"))
    binaries = Path(__file__).resolve().parents[3] / "native/host/target/release/examples"
    parser.add_argument("--pool-bin", type=Path, default=binaries / "pool")
    parser.add_argument("--replay-bin", type=Path, default=binaries / "replay")
    parser.add_argument("--ort", help="ORT library path; otherwise ZHIZI_ORT")
    parser.add_argument("--model", help="model path; otherwise ZHIZI_MODEL")
    parser.add_argument("--itos", help="vocabulary path; otherwise ZHIZI_ITOS")
    parser.add_argument("--weights", type=Path, default=Path(__file__).with_name("weights.json"))
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        return run_bench(args)
    except (OSError, ValueError) as exc:
        print(f"评测运行失败：{exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
