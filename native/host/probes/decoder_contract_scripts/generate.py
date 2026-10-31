"""从仓库公开语料生成可重放的 Decoder 按键脚本。

只接受公开词典中唯一的注音；不使用候选排序、owner 记录或联网服务。
resolve_prefix 注释供真实 Rime 预处理，下一条 select 必须在验收前解析成
实际覆盖前半截的候选下标；未解析的脚本不能作为分段选词证据。
"""

import argparse
import hashlib
import json
from pathlib import Path
import random
import re


SEED = 20261029


def hanzi(text):
    return bool(text) and all("\u3400" <= char <= "\u9fff" for char in text)


def load_dictionary(path):
    readings = {}
    body = False
    for line in Path(path).read_text(encoding="utf-8-sig").splitlines():
        if line.strip() == "...":
            body = True
            continue
        if not body or not line.strip() or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) < 2 or not hanzi(fields[0]):
            continue
        syllables = fields[1].lower().replace("ü", "v").split()
        if len(syllables) != len(fields[0]) or not all(re.fullmatch("[a-z]+", s) for s in syllables):
            continue
        readings.setdefault(fields[0], set()).add(" ".join(syllables))
    return readings


def annotate(text, readings):
    if not 2 <= len(text) <= 8 or not hanzi(text):
        return None
    paths = [set() for _ in range(len(text) + 1)]
    paths[0].add(())
    for start in range(len(text)):
        for end in range(start + 1, len(text) + 1):
            for reading in sorted(readings.get(text[start:end], ())):
                for prefix in sorted(paths[start]):
                    paths[end].add(prefix + tuple(reading.split()))
                # Two prefixes at a boundary prove ambiguity for every shared suffix.
                paths[end] = set(sorted(paths[end])[:2])
    return " ".join(next(iter(paths[-1]))) if len(paths[-1]) == 1 else None


def fragments(sources):
    rows, seen = [], set()
    for source in sources:
        source = Path(source)
        for case in json.loads(source.read_text(encoding="utf-8"))["cases"]:
            for sentence in case["allowed"]:
                if not hanzi(sentence):
                    continue
                for start in range(len(sentence)):
                    for size in range(2, 9):
                        fragment = sentence[start:start + size]
                        if len(fragment) != size or fragment in seen:
                            continue
                        seen.add(fragment)
                        rows.append({"fragment": fragment, "source": str(source),
                                     "case_id": case["id"], "start": start,
                                     "length": size, "sentence": sentence})
    return rows


def choose_fragments(rows, readings, count, seed):
    if count <= 0:
        raise ValueError("count must be positive")
    buckets = {size: [] for size in range(2, 9)}
    rejected = 0
    for row in rows:
        reading = annotate(row["fragment"], readings)
        if reading is None:
            rejected += 1
        else:
            buckets[row["length"]].append({**row, "reading": reading})
    available = {size: len(bucket) for size, bucket in buckets.items()}
    rng = random.Random(seed)
    for bucket in buckets.values():
        rng.shuffle(bucket)
    chosen = []
    while len(chosen) < count and any(buckets.values()):
        for size, bucket in buckets.items():
            if bucket and len(chosen) < count:
                chosen.append(bucket.pop())
    return chosen, {"input_fragments": len(rows), "accepted": sum(available.values()),
                    "rejected_missing_or_ambiguous": rejected, "available_by_length": available}


def edit_script(reading, seed):
    rng = random.Random(seed)
    keys = reading.replace(" ", "")
    actions = ["clear"]
    edits = ["backspace", "left", "right", "pagedown", "pageup", "next", "prev"]
    # First expose the complete transcription, so the resolver can prove a partial
    # selection. Only then scramble editing actions; these may leave invalid input.
    for key in keys:
        actions.append(f"type {key}")
    actions.extend(["# resolve_prefix", "select 1 deliberate"])
    actions.extend(rng.sample(edits, len(edits)))
    actions.extend(["commit_raw", "clear"])
    return actions


def typing_script(reading):
    return [f"type {letter}" for letter in reading.replace(" ", "")]


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    repository = Path(__file__).resolve().parents[4]
    parser.add_argument("--dictionary", type=Path, required=True,
                        help="本地公开 pinyin_simp.dict.yaml；不下载")
    parser.add_argument("--output", type=Path, required=True, help="新建输出目录")
    parser.add_argument("--count", type=int, default=70)
    parser.add_argument("--seed", type=int, default=SEED)
    parser.add_argument("--typing-only", action="store_true", help="只逐字母输入；最后一步保留完整输入第一页")
    args = parser.parse_args(argv)
    sources = [repository / "quality/corpus/base-500.json",
               repository / "quality/corpus/generalization-smoke-20.json"]
    rows, stats = choose_fragments(fragments(sources), load_dictionary(args.dictionary),
                                  args.count, args.seed)
    if len(rows) != args.count:
        parser.error(f"only {len(rows)} unambiguous fragments for requested {args.count}")
    args.output.mkdir(parents=True, exist_ok=False)
    scripts = []
    for index, row in enumerate(rows):
        filename = f"public-{index + 1:03}.keys"
        actions = typing_script(row["reading"]) if args.typing_only else edit_script(row["reading"], args.seed + index)
        metadata = {**row, "source": str(Path(row["source"]).relative_to(repository)),
                    "seed": args.seed + index, "typing_only": args.typing_only}
        content = "# " + json.dumps(metadata, ensure_ascii=False, sort_keys=True) + "\n"
        path = args.output / filename
        path.write_text(content + "\n".join(actions) + "\n", encoding="utf-8")
        scripts.append({"file": filename, **metadata, "sha256": sha256(path),
                        "steps": sum(not line.startswith("#") for line in actions),
                        "actions": actions, "requires_prefix_resolution": not args.typing_only})
    manifest = {"format_version": 1, "seed": args.seed,
                "annotation": "unique dictionary segmentation; ambiguous/missing readings rejected",
                "sources": [{"path": str(path.relative_to(repository)), "sha256": sha256(path)}
                            for path in sources],
                "dictionary": {"filename": args.dictionary.name, "sha256": sha256(args.dictionary)},
                "selection": stats, "script_count": len(scripts),
                "step_count": sum(script["steps"] for script in scripts), "scripts": scripts}
    (args.output / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"scripts": len(scripts), "steps": manifest["step_count"],
                      "output": str(args.output), "unresolved_prefix_markers": 0 if args.typing_only else len(scripts)},
                     ensure_ascii=False))


if __name__ == "__main__":
    main()
