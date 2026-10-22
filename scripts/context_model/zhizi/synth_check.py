"""Per-batch quality check. Exit 1 (stop the batch loop) when a batch looks broken."""
import json, os, sys

batch = sys.argv[1]
stats = json.load(open(os.path.join(batch, "stats.json")))
lines, seen_global = [], set()
for b in ("qwen", "glm", "deepseek"):
    p = os.path.join(batch, f"{b}.jsonl")
    if os.path.exists(p):
        lines += [x for l in open(p, encoding="utf-8") for x in json.loads(l)["lines"]]
root = os.path.dirname(batch)
for other in sorted(os.listdir(root)):  # duplicates against earlier batches
    od = os.path.join(root, other)
    if od == batch or not other.startswith("batch-") or not os.path.exists(os.path.join(od, "DONE")):
        continue
    for b in ("qwen", "glm", "deepseek"):
        p = os.path.join(od, f"{b}.jsonl")
        if os.path.exists(p):
            seen_global.update(x for l in open(p, encoding="utf-8") for x in json.loads(l)["lines"])
ok = sum(v["ok"] for v in stats.values()); fail = sum(v["fail"] for v in stats.values())
chars = sum(len(x) for x in lines)
report = {
    "batch": os.path.basename(batch), "chars": chars, "lines": len(lines), "calls_ok": ok, "calls_fail": fail,
    "fail_rate": round(fail / max(1, ok + fail), 3),
    "dup_within": round(1 - len(set(lines)) / max(1, len(lines)), 4),
    "dup_vs_earlier": round(sum(x in seen_global for x in set(lines)) / max(1, len(set(lines))), 4),
    "with_english": round(sum(any(c.isascii() and c.isalpha() for c in x) for x in lines) / max(1, len(lines)), 3),
    "avg_line_chars": round(chars / max(1, len(lines)), 1),
}
json.dump(report, open(os.path.join(batch, "check.json"), "w"), ensure_ascii=False, indent=2)
print(json.dumps(report, ensure_ascii=False))
bad = report["fail_rate"] > 0.2 or report["dup_within"] > 0.05 or report["dup_vs_earlier"] > 0.1 or chars < 500_000
sys.exit(1 if bad else 0)
