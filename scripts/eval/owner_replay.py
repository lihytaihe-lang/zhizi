# -*- coding: utf-8 -*-
"""Owner-record replay yardstick (zhixu block 0, EXECUTION-PLAN-0.2.0 A1-A4; local only,
owner data never leaves this machine).

  python3 scripts/eval/owner_replay.py sample --events <events.jsonl> --since 2026-10-19 \
      --hit-ratio 7 --seed 1 --out <dir>
  python3 scripts/eval/owner_replay.py compare --cases <dir> --old <out-old.jsonl> \
      --new <out-new.jsonl> [--report <md>]
  python3 scripts/eval/owner_replay.py redo --events <events.jsonl> --since 2026-10-19
  python3 scripts/eval/owner_replay.py rates --events <events.jsonl> --a-since 2026-10-27 \
      --a-until 2026-10-29T08:33 --b-since 2026-10-29T08:33
  python3 scripts/eval/owner_replay.py variance --a <out-a.jsonl> --b <out-b.jsonl> --cases <dir>
  python3 scripts/eval/owner_replay.py curve --input <sequential-out.jsonl>

`sample` freezes the replay set from a COPY of the owner's events.jsonl: every miss
(selectedIndex >= 1, segment-spelled wholes marked `segments`) plus a seeded 1-in-N sample
of the hits, so `compare` can weigh the sampled hits back to the full population
(LOG-REVIEW-2026-10-26 section 4).  cases.jsonl keeps eval.rs' fields (id/bucket/context/
keys/target; it indexes a serde_json::Value and ignores the extras kind/weight) and
manifest.json carries the file hash, sample parameters and the owner's 500 most-committed
words for the "no frequent word may break" check (G1).  scripts/eval/owner_replay.sh wraps
two commits through run_eval.sh into `compare`.
`sample` preserves event-file order and segment choices for sequential eval replay.
`rates` reads the events file directly and compares two time windows of it (minute-precise
via YYYY-MM-DDTHH:MM): commit count, first-choice rate, the non-first share split into
first-page and paged picks (`maxPage`), the deleted-after-commit share and the redo share,
all judged with `sample`'s criteria, then B's change against A.
`curve` groups that replay's output in file order, 500 cases at a time, and prints the
unweighted first-choice hit rate (including cases whose target could not be selected).
"""
import argparse
import hashlib
import json
import os
import random
import re
import unicodedata
from collections import Counter
from datetime import datetime

PINYIN = re.compile(r"^[a-z]+$")


def read_events(path):
    """Parsed events plus the count of non-blank lines and of unparseable ones."""
    rows, lines, bad = [], 0, 0
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            if not line.strip():
                continue
            lines += 1
            try:
                rows.append(json.loads(line))
            except ValueError:
                bad += 1
    return rows, lines, bad


def load_jsonl(path):
    with open(path, encoding="utf-8") as fh:
        return [json.loads(line) for line in fh if line.strip()]


def since_ms(text):
    # Local time; a bare date means local midnight, matching how the owner's day boundaries behave.
    fmt = "%Y-%m-%dT%H:%M" if "T" in text else "%Y-%m-%d"
    return int(datetime.strptime(text, fmt).timestamp() * 1000)


def window_commits(rows, since, until=None):
    """Committed candidates inside the [since, until) local-time window (see `since_ms`)."""
    cut, stop = since_ms(since), since_ms(until) if until else None
    return [r for r in rows
            if r.get("outcome") == "committed_candidate"
            and type(r.get("recordedAtMs")) is int and r["recordedAtMs"] >= cut
            and (stop is None or r["recordedAtMs"] < stop)]


def judged(row):
    """The picks `sample` builds cases from: readable pinyin, committed text and index."""
    return (PINYIN.match(row.get("pinyin") or "") is not None and bool(row.get("selectedText"))
            and type(row.get("selectedIndex")) is int)


def bucket_of(keys):
    return "short" if len(keys) <= 3 else "mid" if len(keys) <= 6 else "long"


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def cmd_sample(args):
    rows, lines, bad = read_events(args.events)
    # --until (exclusive) cuts a training segment off before the evaluation sample starts
    committed = window_commits(rows, args.since, getattr(args, "until", None))
    top_words = Counter(r["committedText"] for r in committed if r.get("committedText"))
    hits, misses = [], []  # positions into `committed`
    for i, r in enumerate(committed):
        if not judged(r):
            continue
        index = r["selectedIndex"]
        if index == 0:
            hits.append(i)
        elif index >= 1:
            misses.append(i)
    rng = random.Random(args.seed)
    kept_hits = set()
    for start in range(0, len(hits), args.hit_ratio):
        kept_hits.add(hits[start + rng.randrange(min(args.hit_ratio, len(hits) - start))])
    cases, counts = [], {"hit": 0, "miss": 0, "segments": 0}
    miss_set = set(misses)
    for i, r in enumerate(committed):
        if i in miss_set:
            kind = "segments" if r.get("segments") else "miss"
        elif i in kept_hits:
            kind = "hit"
        else:
            continue
        counts[kind] += 1
        cases.append({"id": f"{kind}-{counts[kind]:04d}", "bucket": bucket_of(r["pinyin"]),
                      "context": (r.get("precedingText") or "")[-32:], "keys": r["pinyin"],
                      "target": r["selectedText"], "kind": kind,
                      "weight": args.hit_ratio if kind == "hit" else 1,
                      # deleted or retyped right after the commit: the owner did not want what went up
                      "deleted": bool((type(r.get("deletedAfterCommit")) is int and r["deletedAfterCommit"] > 0)
                                      or r.get("redoAfterCommit"))})
        if r.get("segments"):
            cases[-1]["segments"] = r["segments"]
    os.makedirs(args.out, exist_ok=True)
    cases_path = os.path.join(args.out, "cases.jsonl")
    with open(cases_path, "w", encoding="utf-8") as fh:
        for case in cases:
            fh.write(json.dumps(case, ensure_ascii=False) + "\n")
    os.chmod(cases_path, 0o600)
    manifest = {"eventsSha256": sha256_of(args.events), "lines": lines, "badLines": bad,
                "since": args.since, "until": getattr(args, "until", None), "hitRatio": args.hit_ratio, "seed": args.seed,
                "kindCounts": counts,
                "topWords": [[word, n] for word, n in top_words.most_common(500)]}
    manifest_path = os.path.join(args.out, "manifest.json")
    with open(manifest_path, "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, ensure_ascii=False, indent=1)
    os.chmod(manifest_path, 0o600)
    print(len(cases), "cases", counts, "| lines", lines, "bad", bad)
    return manifest


def assess(case, was, now, frequent, stats):
    """Classify one case and file it; returns the verdict for grouping."""
    target = case["target"]
    old_first = (was.get("page") or [""])[0]
    new_first = (now.get("page") or [""])[0]
    case = dict(case, oldFirst=old_first, newFirst=new_first)
    # Typing in segments, the owner picks a first piece and goes on: losing that piece from the first slot costs a
    # step even though neither first equals the whole text, so it is counted apart.
    if (old_first and target.startswith(old_first) and old_first != target
            and not (new_first and target.startswith(new_first))):
        stats["prefix_lost"].append(case)
    if old_first == target and new_first == target:
        stats["both"] += 1
        return "both"
    if old_first == target:
        stats["broke"].append(case)
        if len(new_first) == len(target):
            stats["silent"].append(case)
        if target in frequent:
            stats["hot"].append(case)
        return "broke"
    if new_first == target:
        stats["fixed"].append(case)
        return "fixed"
    stats["neither"] += 1
    return "neither"


def group_rows(pairs, key):
    groups = {}
    for case, verdict in pairs:
        g = groups.setdefault(case[key], {"cases": 0, "fixed": 0, "broke": 0, "both": 0, "neither": 0,
                                          "fixedWeighted": 0, "brokeWeighted": 0})
        g["cases"] += 1
        g[verdict] += 1
        if verdict == "fixed":
            g["fixedWeighted"] += case["weight"]
        elif verdict == "broke":
            g["brokeWeighted"] += case["weight"]
    return groups


def cmd_compare(args):
    cases = {c["id"]: c for c in load_jsonl(os.path.join(args.cases, "cases.jsonl"))}
    with open(os.path.join(args.cases, "manifest.json"), encoding="utf-8") as fh:
        manifest = json.load(fh)
    frequent = {word for word, _ in manifest.get("topWords", [])[:500]}
    old = {r["id"]: r for r in load_jsonl(args.old)}
    new = {r["id"]: r for r in load_jsonl(args.new)}
    stats = {"fixed": [], "broke": [], "silent": [], "hot": [], "prefix_lost": [],
             "both": 0, "neither": 0, "missing": []}
    verdicts = []
    stats["deleted"] = []
    for cid, case in cases.items():
        if case.get("deleted") and case["kind"] == "hit":
            # a "hit" the owner deleted or retyped is no evidence its first was right: listed, not judged
            if cid in old and cid in new and (old[cid].get("page") or [""])[0] != (new[cid].get("page") or [""])[0]:
                stats["deleted"].append(dict(case, oldFirst=old[cid]["page"][0] if old[cid].get("page") else "",
                                             newFirst=new[cid]["page"][0] if new[cid].get("page") else ""))
            continue
        if cid not in old or cid not in new:
            stats["missing"].append(cid)
            continue
        verdicts.append((case, assess(case, old[cid], new[cid], frequent, stats)))
    stats["fixedWeighted"] = sum(c["weight"] for c in stats["fixed"])
    stats["brokeWeighted"] = sum(c["weight"] for c in stats["broke"])
    stats["pass"] = stats["fixedWeighted"] >= 3 * stats["brokeWeighted"] and not stats["hot"]
    stats["byBucket"] = group_rows(verdicts, "bucket")
    stats["byKind"] = group_rows(verdicts, "kind")
    print(f"fixed {len(stats['fixed'])} (weighted {stats['fixedWeighted']}), "
          f"broke {len(stats['broke'])} (weighted {stats['brokeWeighted']}), "
          f"both {stats['both']}, neither {stats['neither']}, "
          f"silent {len(stats['silent'])}, hot {len(stats['hot'])}, missing {len(stats['missing'])}, "
          f"deleted hits changed (not judged) {len(stats['deleted'])}, "
          f"first piece lost {len(stats['prefix_lost'])}")
    for case in stats["prefix_lost"][:20]:
        print(f"  PIECE-LOST {case['id']} {case['keys']} -> {case['target']} (was {case['oldFirst']}, now {case['newFirst']})")
    verdict = "PASS" if stats["pass"] else "FAIL"
    print(f"G1 {verdict}: fixed weighted {stats['fixedWeighted']} vs 3 x broke weighted "
          f"{3 * stats['brokeWeighted']}; high-frequency breaks {len(stats['hot'])}")
    for case in stats["hot"]:
        print(f"  HOT {case['id']} {case['keys']} -> {case['target']} (now {case['newFirst']})")
    for title, groups in (("bucket", stats["byBucket"]), ("kind", stats["byKind"])):
        print(f"by {title}:")
        print(f"{title}\tcases\tfixed\tbroke\tboth\tneither\tfixedW\tbrokeW")
        for name, g in sorted(groups.items()):
            print(f"{name}\t{g['cases']}\t{g['fixed']}\t{g['broke']}\t{g['both']}\t{g['neither']}"
                  f"\t{g['fixedWeighted']}\t{g['brokeWeighted']}")
    if args.report:
        write_report(args.report, stats)
    return stats


def md_group_table(groups, title):
    head = (f"| {title} | 条数 | 修好 | 弄坏 | 都对 | 都错 | 修好加权 | 弄坏加权 |\n"
            "|---|---:|---:|---:|---:|---:|---:|---:|\n")
    return head + "".join(
        f"| {name} | {g['cases']} | {g['fixed']} | {g['broke']} | {g['both']} | {g['neither']}"
        f" | {g['fixedWeighted']} | {g['brokeWeighted']} |\n"
        for name, g in sorted(groups.items()))


def write_report(path, stats):
    verdict = "PASS" if stats["pass"] else "FAIL"
    lines = ["# owner 重放对比（G1）", "",
             f"- 判定：**{verdict}**。门：修好加权 ≥ 3 × 弄坏加权，且高频词弄坏为 0。",
             f"- 修好 {len(stats['fixed'])} 条（加权 {stats['fixedWeighted']}），"
             f"弄坏 {len(stats['broke'])} 条（加权 {stats['brokeWeighted']}）；"
             f"都对 {stats['both']}，都错 {stats['neither']}，"
             f"静默错字 {len(stats['silent'])}，高频词弄坏 {len(stats['hot'])}，"
             f"两边输出缺失 {len(stats['missing'])}条。",
             "  - 修好/弄坏按 weight 加权（命中是抽样，weight 换算回全量）；静默错字 = "
             "旧首选对、新首选错且与目标等长（看起来像对的）。",
             "", "## 按输入长度（bucket）", "", md_group_table(stats["byBucket"], "bucket"),
             "", "## 按样本类别（kind）", "", md_group_table(stats["byKind"], "kind")]
    for title, rows in (("高频词弄坏（目标在 owner 最常上屏 500 词内）", stats["hot"]),
                        ("静默错字（旧对、新错、等长）", stats["silent"])):
        lines += ["", f"## {title}", "",
                  "| id | bucket | kind | keys | 目标 | 旧首选 | 新首选 | weight |",
                  "|---|---|---|---|---|---|---|---:|"]
        lines += [f"| {c['id']} | {c['bucket']} | {c['kind']} | {c['keys']} | {c['target']}"
                  f" | {c['oldFirst']} | {c['newFirst']} | {c['weight']} |" for c in rows]
    lines += ["", "## 弄坏清单（前 200 条，按 weight 降序）", "",
              "| id | bucket | kind | keys | 目标 | 旧首选 | 新首选 | weight | 静默 | 高频 |",
              "|---|---|---|---|---|---|---|---:|---|---|"]
    broke = sorted(stats["broke"], key=lambda c: (-c["weight"], c["id"]))[:200]
    silent_ids = {c["id"] for c in stats["silent"]}
    hot_ids = {c["id"] for c in stats["hot"]}
    lines += [f"| {c['id']} | {c['bucket']} | {c['kind']} | {c['keys']} | {c['target']}"
              f" | {c['oldFirst']} | {c['newFirst']} | {c['weight']}"
              f" | {'是' if c['id'] in silent_ids else '—'} | {'是' if c['id'] in hot_ids else '—'} |"
              for c in broke]
    if len(stats["broke"]) > 200:
        lines.append(f"\n（共 {len(stats['broke'])} 条弄坏，只列前 200。）")
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("".join(line + "\n" for line in lines))
    print("report:", path)


def classify_redo(this, nxt):
    """Queue Q6 classes.  A punctuation-only commit counts as punct_only first (deleting a
    comma is housekeeping, not a mispick); among the rest, the next event decides: same
    reading re-committed with a different word is repick, same reading otherwise is
    retype_same_reading.  Precedence is ours; the task text lists retype before repick,
    which read literally would make repick unreachable."""
    text = this.get("committedText") or this.get("selectedText") or ""
    if text and all(unicodedata.category(c).startswith("P") for c in text):
        return "punct_only"
    if nxt is not None and nxt.get("pinyin") == this.get("pinyin"):
        if nxt.get("outcome") == "committed_candidate" and nxt.get("selectedText") != this.get("selectedText"):
            return "repick"
        return "retype_same_reading"
    return "other"


def cmd_redo(args):
    rows, _, _ = read_events(args.events)
    cut = since_ms(args.since)
    counts = Counter()
    for i, r in enumerate(rows):
        index = r.get("selectedIndex")
        if r.get("outcome") != "committed_candidate" or type(index) is not int or index != 0:
            continue
        if type(r.get("recordedAtMs")) is int and r["recordedAtMs"] < cut:
            continue
        deleted = r.get("deletedAfterCommit")
        if not (type(deleted) is int and deleted > 0) and not r.get("redoAfterCommit"):
            continue
        counts[classify_redo(r, rows[i + 1] if i + 1 < len(rows) else None)] += 1
    total = sum(counts.values())
    for kind in ("retype_same_reading", "punct_only", "repick", "other"):
        n = counts.get(kind, 0)
        share = f"{100 * n / total:.1f}%" if total else "-"
        print(f"{kind}\t{n}\t{share}")
    print("total", total)
    return dict(counts)


def window_rates(commits):
    """Counts and rates over one window's judged picks; criteria shared with `sample`."""
    counts = dict.fromkeys(("first", "nonfirst_first_page", "nonfirst_paged", "deleted", "redo"), 0)
    for r in commits:
        # `candidates` and `selectedIndex` describe the last page shown, so a non-first
        # pick is counted as paged when the composition ever left page 0 (`maxPage`).
        if r["selectedIndex"] == 0:
            counts["first"] += 1
        elif type(r.get("maxPage")) is int and r["maxPage"] > 0:
            counts["nonfirst_paged"] += 1
        else:
            counts["nonfirst_first_page"] += 1
        # deleted or retyped right after the commit: the owner did not want what went up
        if (type(r.get("deletedAfterCommit")) is int and r["deletedAfterCommit"] > 0) or r.get("redoAfterCommit"):
            counts["deleted"] += 1
        if r.get("redoAfterCommit"):
            counts["redo"] += 1
    rates = {"commits": len(commits)}
    for key, n in counts.items():
        rates[key] = n
        rates[key + "Rate"] = n / len(commits) if commits else 0.0
    return rates


RATE_ROWS = (("commits", "上屏条数"), ("firstRate", "首选上屏率"),
             ("nonfirst_first_pageRate", "非首选·第一页"), ("nonfirst_pagedRate", "非首选·翻页后"),
             ("deletedRate", "上屏后删除"), ("redoRate", "重打率"))


def cmd_rates(args):
    rows, _, _ = read_events(args.events)
    result = {side: window_rates([r for r in window_commits(rows, getattr(args, side + "_since"),
                                                            getattr(args, side + "_until", None))
                                  if judged(r)])
              for side in ("a", "b")}
    print("指标\tA\tB")
    for key, label in RATE_ROWS:
        cells = [str(result[side][key]) if key == "commits" else f"{100 * result[side][key]:.1f}%"
                 for side in ("a", "b")]
        print(f"{label}\t" + "\t".join(cells))
    changes = [f"{label} {result['b'][key] - result['a'][key]:+d} 条" if key == "commits"
               else f"{label} {100 * (result['b'][key] - result['a'][key]):+.1f}pp"
               for key, label in RATE_ROWS]
    print("B 相对 A 的变化：" + "，".join(changes))
    return result


def cmd_variance(args):
    a = {r["id"]: r for r in load_jsonl(args.a)}
    b = {r["id"]: r for r in load_jsonl(args.b)}
    known = {c["id"] for c in load_jsonl(os.path.join(args.cases, "cases.jsonl"))}
    per = {}
    for cid in known:
        if cid not in a or cid not in b:
            continue
        bucket = a[cid].get("bucket") or "all"
        row = per.setdefault(bucket, [0, 0, 0])
        row[0] += 1
        row[1] += int(bool(a[cid].get("first")))
        row[2] += int(bool(b[cid].get("first")))
    print("bucket\tcases\trate-a\trate-b\t|diff|")
    result = {}
    for bucket in sorted(per):
        n, fa, fb = per[bucket]
        rate_a, rate_b = fa / n, fb / n
        result[bucket] = (n, rate_a, rate_b)
        print(f"{bucket}\t{n}\t{rate_a:.3f}\t{rate_b:.3f}\t{abs(rate_a - rate_b):.3f}")
    return result


def cmd_curve(args):
    rows = load_jsonl(args.input)
    result = []
    print("range\tcases\tfirst\tfirst-rate")
    for start in range(0, len(rows), 500):
        group = rows[start:start + 500]
        first = sum(row["first"] is True for row in group)
        rate = first / len(group)
        end = start + len(group)
        result.append({"start": start + 1, "end": end, "cases": len(group),
                       "first": first, "rate": rate})
        print(f"{start + 1}-{end}\t{len(group)}\t{first}\t{100 * rate:.1f}%")
    return result


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="command", required=True)
    s = sub.add_parser("sample", help="freeze the replay case set from a copy of events.jsonl")
    s.add_argument("--events", required=True)
    s.add_argument("--since", required=True, help="YYYY-MM-DD or YYYY-MM-DDTHH:MM (local)")
    s.add_argument("--until", help="exclusive end, same formats (local); default: no end")
    s.add_argument("--hit-ratio", type=int, default=7, help="keep 1 of every N first-choice hits")
    s.add_argument("--seed", type=int, default=1)
    s.add_argument("--out", required=True)
    s.set_defaults(func=cmd_sample)
    c = sub.add_parser("compare", help="judge two run_eval.sh outputs against the frozen set")
    c.add_argument("--cases", required=True)
    c.add_argument("--old", required=True)
    c.add_argument("--new", required=True)
    c.add_argument("--report", help="write a Chinese markdown report here")
    c.set_defaults(func=cmd_compare)
    r = sub.add_parser("redo", help="classify deleted first-choice commits (queue Q6)")
    r.add_argument("--events", required=True)
    r.add_argument("--since", required=True, help="YYYY-MM-DD or YYYY-MM-DDTHH:MM (local)")
    r.set_defaults(func=cmd_redo)
    rate = sub.add_parser("rates", help="typing metrics of two time windows, B against A")
    rate.add_argument("--events", required=True)
    rate.add_argument("--a-since", required=True, help="YYYY-MM-DD or YYYY-MM-DDTHH:MM (local)")
    rate.add_argument("--a-until", help="exclusive end, same formats (local); default: no end")
    rate.add_argument("--b-since", required=True, help="YYYY-MM-DD or YYYY-MM-DDTHH:MM (local)")
    rate.add_argument("--b-until", help="exclusive end, same formats (local); default: no end")
    rate.set_defaults(func=cmd_rates)
    v = sub.add_parser("variance", help="per-bucket first-choice rate difference of two runs")
    v.add_argument("--a", required=True)
    v.add_argument("--b", required=True)
    v.add_argument("--cases", required=True)
    v.set_defaults(func=cmd_variance)
    curve = sub.add_parser("curve", help="first-choice hit rate per 500 sequential replay cases")
    curve.add_argument("--input", required=True, help="sequential eval output JSONL, in replay order")
    curve.set_defaults(func=cmd_curve)
    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
