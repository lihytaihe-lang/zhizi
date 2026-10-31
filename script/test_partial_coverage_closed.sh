#!/usr/bin/env bash
# test_partial_coverage_closed.sh — closed-state partial-coverage top1 gate
#
# Closed state (machine-independent, owner-lexicon-free): a fresh empty user
# dir from mktemp plus the synthetic entry declared in
# quality/corpus/partial-coverage-synthetic-v1.json (restore path proven by
# script/test_partial_coverage_synthetic.sh). The owner Rime dir under
# ~/Library/Application Support and AGENT_IME_OWNER_RIME_DIR are never read.
#
# For every corpus case NOT registered in KNOWN_OPEN, page-1 candidate #1 must
# be in the case's allowed list (same top1 semantics as AgentIMEReplayHarness:
# NFKC + trim normalized text). KNOWN_OPEN lists cases that cannot pass
# deterministically in the closed state; each is printed as
# `known-open: <id> <reason>` and does not affect the exit code. Every corpus
# case is either asserted or registered, so nothing is silently dropped or
# relaxed. Measured 2026-10-23: all 9 cases pass in the closed state (three
# consecutive identical runs), so the list is empty.
#
# Scope (do not over-claim): this gate proves that WITH the declared synthetic
# entry the core ranks the covered composition first in a closed state. It
# does NOT prove real commit-gesture learning writes work, and it does NOT
# fix, replace or relax partial_coverage_top1_replay, which replays the
# owner's real lexicon and stays as-is.
#
# Env overrides (testing only; must point at binaries built from the same
# sources, never at a real user library):
#   AIM_CLOSED_PROBE_BIN / AIM_CLOSED_CTL_BIN  when one is set, both must be;
#                                              no build happens in that mode
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
SHARED="$BUNDLE/Contents/Resources/Rime"
TOP1_CORPUS="$ROOT_DIR/quality/corpus/partial-coverage-top1-v1.json"
SYNTHETIC_CORPUS="$ROOT_DIR/quality/corpus/partial-coverage-synthetic-v1.json"
FROZEN_TOP1_SHA="f13db46c21d8b945b0cd69c507d14163eb8d85e821d4f06dac46c39712fc4c2d"
FROZEN_SYNTHETIC_SHA="9593cd497beb9289de859f1b42537f93c7d308ed935104792d34b8698f74e2dd"
PAGES=3

# Cases that cannot deterministically pass in the closed state, as "id|reason".
KNOWN_OPEN=()

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release input bundle: $BUNDLE" >&2; exit 2; }
[[ -d "$SHARED" ]] || { echo "missing shared Rime dir: $SHARED" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "python3 missing from PATH" >&2; exit 2; }
command -v shasum >/dev/null 2>&1 || { echo "shasum missing from PATH" >&2; exit 2; }

corpus_sha() { shasum -a 256 "$1" | awk '{print $1}'; }
die() { echo "$1" >&2; exit "${2:-1}"; }
ACTUAL_TOP1_SHA="$(corpus_sha "$TOP1_CORPUS")"
ACTUAL_SYNTHETIC_SHA="$(corpus_sha "$SYNTHETIC_CORPUS")"
[[ "$ACTUAL_TOP1_SHA" == "$FROZEN_TOP1_SHA" ]] || {
  echo "closed-gate top1 corpus checksum mismatch: expected $FROZEN_TOP1_SHA, got $ACTUAL_TOP1_SHA" >&2
  exit 6
}
[[ "$ACTUAL_SYNTHETIC_SHA" == "$FROZEN_SYNTHETIC_SHA" ]] || {
  echo "closed-gate synthetic corpus checksum mismatch: expected $FROZEN_SYNTHETIC_SHA, got $ACTUAL_SYNTHETIC_SHA" >&2
  exit 6
}

# Corpus contract: the shape the owner replay gate freezes, plus this gate's
# own requirement that every case carries a non-empty allowed list.
python3 - "$TOP1_CORPUS" "$SYNTHETIC_CORPUS" <<'PY' || die "closed-gate corpus contract precheck failed (missing/tampered)" 6
import json, sys
top1 = json.load(open(sys.argv[1], encoding="utf-8"))
syn = json.load(open(sys.argv[2], encoding="utf-8"))
assert top1["kind"] == "partial-coverage-top1-regression" and top1["formatVersion"] == 1
cases = top1["cases"]
assert len(cases) == 9 and all(c["mustPassTop1"] is True for c in cases)
assert all(isinstance(c.get("allowed"), list) and c["allowed"] for c in cases)
assert len([c for c in cases if c.get("expectedSet") is not None]) == 3
assert len([c for c in cases if c["category"] == "strict-prefix-abbreviation-regression"]) == 1
assert len([c for c in cases if c["category"] == "registered-ascii-control"]) == 3
assert syn["kind"] == "partial-coverage-synthetic-v1" and syn["formatVersion"] == 1
e = syn["syntheticEntry"]
assert e["text"] == "就可以做完" and e["pinyin"] == "jiu ke yi zuo wan"
assert e["entryLine"] == "#@/db_name\tpinyin_simp\n#@/db_type\tuserdb\njiu ke yi zuo wan\t就可以做完\tc=1"
assert syn["probePages"] == 3
PY

if [[ -n "${AIM_CLOSED_PROBE_BIN:-}${AIM_CLOSED_CTL_BIN:-}" ]]; then
  PROBE="${AIM_CLOSED_PROBE_BIN:?set AIM_CLOSED_PROBE_BIN together with AIM_CLOSED_CTL_BIN}"
  CTL="${AIM_CLOSED_CTL_BIN:?set AIM_CLOSED_CTL_BIN together with AIM_CLOSED_PROBE_BIN}"
else
  touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
  AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --product AgentIMEUserDBHarness >/dev/null
  BIN_DIR="$(swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution --show-bin-path)"
  PROBE="$BIN_DIR/AgentIMEUserDBHarness"
  CTL="$BUNDLE/Contents/MacOS/AgentIMEUserDBCtl"
fi
[[ -x "$PROBE" ]] || { echo "probe binary missing/not executable: $PROBE" >&2; exit 2; }
[[ -x "$CTL" ]] || { echo "ctl binary missing/not executable: $CTL" >&2; exit 2; }

DYLD_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release"
if [[ -n "${AIM_CLOSED_PROBE_BIN:-}${AIM_CLOSED_CTL_BIN:-}" ]]; then
  DYLD_PATH="$DYLD_PATH:$(dirname "$PROBE")"
fi

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-partial-closed.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
USER_DIR="$TEMP_ROOT/user"
mkdir -p "$USER_DIR"

# Closed initial state: restore exactly the declared synthetic entry, then
# verify the readback sees exactly that one entry (same checks as the
# synthetic diagnostic gate).
python3 - "$SYNTHETIC_CORPUS" >"$TEMP_ROOT/restore-input.json" <<'PY'
import json, sys
e = json.load(open(sys.argv[1], encoding="utf-8"))["syntheticEntry"]
print(json.dumps([{"id": e["id"], "text": e["text"], "pinyin": e["pinyin"], "entryLine": e["entryLine"]}], ensure_ascii=False))
PY
"$CTL" restore "$SHARED" "$USER_DIR" <"$TEMP_ROOT/restore-input.json" >"$TEMP_ROOT/restore-echo.json" 2>"$TEMP_ROOT/restore.err" || {
  echo "closed-gate synthetic restore failed (see $TEMP_ROOT/restore.err)" >&2
  exit 1
}
"$CTL" list "$SHARED" "$USER_DIR" >"$TEMP_ROOT/list.json" 2>"$TEMP_ROOT/list.err" || {
  echo "closed-gate synthetic list failed (see $TEMP_ROOT/list.err)" >&2
  exit 1
}
python3 - "$TEMP_ROOT/restore-echo.json" "$TEMP_ROOT/list.json" "$SYNTHETIC_CORPUS" <<'PY' || die "closed-gate synthetic readback != exactly one expected entry" 1
import json, sys
echo_items = json.load(open(sys.argv[1], encoding="utf-8")).get("items", [])
items = json.load(open(sys.argv[2], encoding="utf-8")).get("items", [])
e = json.load(open(sys.argv[3], encoding="utf-8"))["syntheticEntry"]
assert len(echo_items) == 1 and len(items) == 1
assert items[0].get("text") == e["text"] and items[0].get("pinyin") == e["pinyin"]
assert items[0].get("entryLine") == "1"
PY

# KNOWN_OPEN bookkeeping: ids must be unique and exist in the corpus; corpus
# cases not registered here are asserted below (a new corpus case can never be
# silently skipped).
KNOWN_OPEN_FILE="$TEMP_ROOT/known-open.txt"
: >"$KNOWN_OPEN_FILE"
for ENTRY in ${KNOWN_OPEN[@]+"${KNOWN_OPEN[@]}"}; do
  printf '%s\n' "$ENTRY" >>"$KNOWN_OPEN_FILE"
done
python3 - "$TOP1_CORPUS" "$KNOWN_OPEN_FILE" <<'PY' || die "closed-gate KNOWN_OPEN bookkeeping mismatch (unknown or duplicate id)" 1
import json, sys
corpus_ids = {c["id"] for c in json.load(open(sys.argv[1], encoding="utf-8"))["cases"]}
open_ids = [line.split("|", 1)[0] for line in open(sys.argv[2], encoding="utf-8").read().splitlines() if line.strip()]
assert len(open_ids) == len(set(open_ids)), "duplicate KNOWN_OPEN id"
unknown = set(open_ids) - corpus_ids
assert not unknown, "KNOWN_OPEN id not in corpus: %s" % sorted(unknown)
PY

probe_once() { # query outjson
  DYLD_LIBRARY_PATH="$DYLD_PATH" \
    "$PROBE" --process-deep-candidate-probe "$SHARED" "$USER_DIR" "$1" "$2" "$PAGES" \
    >"$TEMP_ROOT/probe-$1.log" 2>&1
}

check_probe_shape() { # outjson query
  python3 - "$1" "$2" <<'PY' || { echo "closed-gate probe shape invalid: $2" >&2; exit 1; }
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
errs = []
if d.get("query") != sys.argv[2]:
    errs.append("query mismatch")
pages = d.get("pages")
if not isinstance(pages, list) or not pages:
    errs.append("pages empty")
else:
    if [p.get("pageNumber") for p in pages] != list(range(len(pages))):
        errs.append("pageNumber not sequential from 0")
    flat = []
    for p in pages:
        cand = p.get("candidates")
        if not isinstance(cand, list) or not cand or not all(isinstance(x, str) and x for x in cand):
            errs.append("bad candidates page %r" % p.get("pageNumber"))
            cand = []
        flat.extend(cand)
    if d.get("allCandidates") != flat:
        errs.append("allCandidates != flattened page candidates")
if not isinstance(d.get("complete"), bool):
    errs.append("complete not bool")
assert not errs, "; ".join(errs)
PY
}

FAILED=""
ASSERTED=0
while IFS=$'\t' read -r CASE_ID QUERY ALLOWED; do
  OPEN_REASON=""
  if [[ -s "$KNOWN_OPEN_FILE" ]]; then
    OPEN_REASON="$(python3 - "$KNOWN_OPEN_FILE" "$CASE_ID" <<'PY'
import sys
cid = sys.argv[2]
for line in open(sys.argv[1], encoding="utf-8").read().splitlines():
    if line.split("|", 1)[0] == cid:
        print(line.split("|", 1)[1])
        break
PY
)"
  fi
  if [[ -n "$OPEN_REASON" ]]; then
    echo "known-open: $CASE_ID $OPEN_REASON"
    continue
  fi
  ASSERTED=$((ASSERTED + 1))
  OUT="$TEMP_ROOT/probe-$QUERY.json"
  probe_once "$QUERY" "$OUT"
  check_probe_shape "$OUT" "$QUERY"
  VERDICT="$(python3 - "$OUT" "$ALLOWED" <<'PY'
import json, sys, unicodedata
norm = lambda s: unicodedata.normalize("NFKC", s).strip()
d = json.load(open(sys.argv[1], encoding="utf-8"))
top1 = norm(d["pages"][0]["candidates"][0])
allowed = [norm(x) for x in sys.argv[2].split("/")]
print("pass" if top1 in allowed else "fail:" + top1)
PY
)"
  if [[ "$VERDICT" == pass ]]; then
    echo "closed-case: $CASE_ID query=$QUERY top1=pass"
  else
    echo "ASSERT-FAIL: $CASE_ID query=$QUERY top1=${VERDICT#fail:} allowed=$ALLOWED" >&2
    FAILED="$FAILED $CASE_ID"
  fi
done < <(python3 - "$TOP1_CORPUS" <<'PY'
import json, sys
for c in json.load(open(sys.argv[1], encoding="utf-8"))["cases"]:
    print("\t".join([c["id"], c["query"], "/".join(c["allowed"])]))
PY
)

[[ "$ASSERTED" -gt 0 ]] || { echo "closed-gate asserted zero cases" >&2; exit 1; }
[[ -s "$KNOWN_OPEN_FILE" ]] || echo "known-open: (none)"
if [[ -n "${FAILED// /}" ]]; then
  echo "partial_coverage_closed=FAIL failed:$FAILED asserted=$ASSERTED known_open=$(grep -c . "$KNOWN_OPEN_FILE" || true) corpus_checksum=$ACTUAL_TOP1_SHA synthetic_checksum=$ACTUAL_SYNTHETIC_SHA"
  exit 1
fi
echo "partial_coverage_closed=PASS asserted=$ASSERTED known_open=$(grep -c . "$KNOWN_OPEN_FILE" || true) cases=9 corpus_checksum=$ACTUAL_TOP1_SHA synthetic_checksum=$ACTUAL_SYNTHETIC_SHA"
