#!/usr/bin/env python3
"""Per-day and per-segment usage metrics from a COPY of the owner quality log.

    python3 -I scripts/eval/usage_segments.py <events.jsonl copy> [--exclude-day MM-DD ...]

Metrics per day: records, committed candidates, first-choice rate, deleted-after-commit rate,
redo-after-commit rate, abandoned/raw counts, and the "整句"-first rows with how often the owner
picked something else.  Segments are fixed install windows (see SEGMENTS); 11-01 is a transition day
(0.3.12 and 0.3.13 installed the same day) and is reported but left out of the segments.
Local only: input is owner data and nothing here is written back to it.
"""
import argparse, collections, datetime, json, sys

SEGMENTS = [("A 0.3.9-0.3.11", "10-28", "10-31"),
            ("B 0.3.13-0.3.15", "11-02", "11-03"),
            ("C 0.3.16", "11-04", "11-04"),
            ("D 0.3.17+", "11-05", "12-31")]


def load(path, skip):
    days = collections.defaultdict(collections.Counter)
    for line in open(path, encoding="utf-8"):
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if "recordedAtMs" not in r:
            continue
        day = datetime.datetime.fromtimestamp(r["recordedAtMs"] / 1000).strftime("%m-%d")
        if day in skip:
            continue
        c = days[day]
        out = r.get("outcome")
        if out is None:
            continue
        c["records"] += 1
        if out == "abandoned":
            c["abandoned"] += 1
        elif out == "committed_raw":
            c["raw"] += 1
        elif out == "committed_candidate":
            c["n"] += 1
            idx = r.get("selectedIndex", 0)
            c["first"] += idx == 0
            c["deleted"] += bool(r.get("deletedAfterCommit"))
            c["redo"] += bool(r.get("redoAfterCommit"))
            cm = r.get("candidateComments") or []
            if cm and "整句" in (cm[0] or ""):
                c["zj"] += 1
                c["zj_changed"] += idx != 0
    return days


PUNCT = set("，。！？、；：“”‘’（）《》【】…—,.!?;:\"'()[]<>-~· \n\t")
REDO_WINDOW_MS = 30000


def classify_deleted(path, skip):
    """Q6: split 'deleted after commit' into redo-same-reading / punctuation / reselect / other.
    A deleted commit is followed (same app, within REDO_WINDOW_MS) by the next committed or
    abandoned record; same pinyin => retyped the same reading (selected a different candidate =
    reselect, same candidate = plain retype); different pinyin => other."""
    recs = []
    for line in open(path, encoding="utf-8"):
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if "recordedAtMs" in r and r.get("outcome"):
            recs.append(r)
    recs.sort(key=lambda r: r["recordedAtMs"])
    days = collections.defaultdict(collections.Counter)
    for i, r in enumerate(recs):
        if r.get("outcome") != "committed_candidate" or not r.get("deletedAfterCommit"):
            continue
        day = datetime.datetime.fromtimestamp(r["recordedAtMs"] / 1000).strftime("%m-%d")
        if day in skip:
            continue
        c = days[day]
        c["deleted"] += 1
        text = r.get("committedText") or r.get("selectedText") or ""
        if text and all(ch in PUNCT for ch in text):
            c["punct"] += 1
            continue
        window = [x for x in recs[i + 1:i + 12]
                  if x.get("appHash") == r.get("appHash")
                  and x["recordedAtMs"] - r["recordedAtMs"] <= REDO_WINDOW_MS]
        nxt = next((x for x in window if x.get("pinyin") == r.get("pinyin")), None)
        if not window:
            c["no_follow"] += 1
        elif nxt is not None:
            same = (nxt.get("committedText") or nxt.get("selectedText")) == text
            c["retype_same_text" if same else "reselect"] += 1
        else:
            c["other_reading"] += 1
    return days


def q6_table(days):
    cols = ["deleted", "punct", "reselect", "retype_same_text", "other_reading", "no_follow"]
    print("\nQ6 deleted-after-commit split (count; share of deleted non-punct):")
    print("%-16s %7s %6s %9s %9s %9s %9s" % ("", "deleted", "punct", "reselect", "retype", "other", "nofollow"))
    def line(label, c):
        core = c["deleted"] - c["punct"]
        f = lambda k: "%4d %3.0f%%" % (c[k], 100 * c[k] / core) if core else "   -    "
        print("%-16s %7d %6d %s %s %s %s" % (label, c["deleted"], c["punct"], f("reselect"),
              f("retype_same_text"), f("other_reading"), f("no_follow")))
    for name, lo, hi in SEGMENTS:
        tot = collections.Counter()
        for d, c in days.items():
            if lo <= d <= hi:
                tot.update(c)
        line(name, tot)


def pct(a, b):
    return "%5.1f%%" % (100 * a / b) if b else "   -  "


def row(label, c):
    n = c["n"]
    return "%-16s %6d %6d %s %s %s %5d %5d %5d %5d" % (
        label, c["records"], n, pct(c["first"], n), pct(c["deleted"], n), pct(c["redo"], n),
        c["abandoned"], c["raw"], c["zj"], c["zj_changed"])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("log")
    ap.add_argument("--exclude-day", action="append", default=[])
    a = ap.parse_args()
    days = load(a.log, set(a.exclude_day))
    head = "%-16s %6s %6s %7s %7s %7s %5s %5s %5s %5s" % (
        "", "rec", "commit", "first", "deleted", "redo", "aband", "raw", "zj1", "zj-chg")
    print(head)
    for d in sorted(days)[-14:]:
        print(row(d, days[d]))
    print()
    print(head)
    for name, lo, hi in SEGMENTS:
        tot = collections.Counter()
        for d, c in days.items():
            if lo <= d <= hi:
                tot.update(c)
        print(row(name, tot))
    q6_table(classify_deleted(a.log, set(a.exclude_day)))
    print("\n11-01 = transition day (0.3.12 -> 0.3.13), not in any segment.")


if __name__ == "__main__":
    sys.exit(main())
