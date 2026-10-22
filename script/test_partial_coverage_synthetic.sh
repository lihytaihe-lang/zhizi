#!/usr/bin/env bash
# test_partial_coverage_synthetic.sh — partial-coverage synthetic-entry top1 test
#
# Synthetic preconditions (self-contained; do not depend on external task notes):
#   - Fresh empty user dir (mktemp -d only); import EXACTLY ONE public-source
#     synthetic userdb entry from quality/corpus/partial-coverage-synthetic-v1.json:
#     text=就可以做完, pinyin="jiu ke yi zuo wan", entryLine snapshot with c=1.
#   - PRE-DECLARED functional gate (Arm S): for queries "jiukyizuow" and
#     "jiukeyizuow", page-1 candidate #1 (pages[0].candidates[0]) MUST equal
#     就可以做完. No set-equality, no rank-distribution assertions.
#   - Arm E (empty library) is observation ONLY: no absence assertion
#     (three pages unseen != not generated).
#
# Scope boundary (do not over-claim):
#   - c=1 is synthetic-entry EXISTENCE only; NOT evidence of real commit-gesture
#     learning success; does not locate the old gate's root cause.
#   - The frozen gate quality/corpus/partial-coverage-top1-v1.json is NOT touched,
#     NOT run, and remains unresolved. This test is not wired into any aggregate
#     regression and does not claim the original gate is fixed.
#
# Dependencies (explicitly prechecked, nothing assumed preinstalled):
#   python3, shasum; existing binaries (no build here):
#   AgentIMEUserDBHarness, AgentIMEUserDBCtl, script/artifact_checksum.sh
#
# Env overrides (testing only; must point at existing files/dirs, never a real
# user library; overrides are flagged as toolOverrides=true in the report):
#   AIM_SYNTHETIC_BUNDLE       default: <root>/build/native/release/Agent Input Method.inputmethod
#   AIM_SYNTHETIC_PROBE_BIN    default: <root>/native/macos/.build/debug/AgentIMEUserDBHarness
#   AIM_SYNTHETIC_CTL_BIN      default: <root>/native/macos/.build/debug/AgentIMEUserDBCtl
#   AIM_SYNTHETIC_TMP_PARENT   mktemp parent for ALL temp dirs/reports (default ${TMPDIR:-/tmp});
#                              must NOT be an existing user-library path
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="${BASH_SOURCE[0]}"
CORPUS="$ROOT/quality/corpus/partial-coverage-synthetic-v1.json"
BUNDLE="${AIM_SYNTHETIC_BUNDLE:-$ROOT/build/native/release/Agent Input Method.inputmethod}"
SHARED="$BUNDLE/Contents/Resources/Rime"
PROBE="${AIM_SYNTHETIC_PROBE_BIN:-$ROOT/native/macos/.build/debug/AgentIMEUserDBHarness}"
CTL="${AIM_SYNTHETIC_CTL_BIN:-$ROOT/native/macos/.build/debug/AgentIMEUserDBCtl}"
PAGES=3
TMP_PARENT="${AIM_SYNTHETIC_TMP_PARENT:-${TMPDIR:-/tmp}}"
TOOL_OVERRIDES=$([ -n "${AIM_SYNTHETIC_PROBE_BIN:-}" ] || [ -n "${AIM_SYNTHETIC_CTL_BIN:-}" ] && echo true || echo false)
WORK="$(mktemp -d "$TMP_PARENT/agent-ime-synthetic.XXXXXX")"
REPORT="$WORK/synthetic-report.json"
STATUS=FAIL
FAILED_CASES=""
PASSED_CASES=""
CTRL_NOTE="not-completed"
REASON=""

note() { printf '%s=%s\n' "$1" "$2" >>"$WORK/obs.txt"; }
emit_report() {
  if ! command -v python3 >/dev/null 2>&1; then
    echo "FATAL: python3 unavailable; report NOT generated; WORK=$WORK" >&2
    return 1
  fi
  python3 - "$REPORT" "$STATUS" "$FAILED_CASES" "$PASSED_CASES" "$CTRL_NOTE" "$REASON" \
    "$TOOL_OVERRIDES" "$RUNNER" "$CORPUS" "$SHARED" "$PROBE" "$CTL" "$WORK/obs.txt" <<'PY'
import hashlib, json, os, sys
(out, status, failed, passed, ctrl, reason, tool, runner,
 corpus, shared, probe, ctl, obs_path) = sys.argv[1:14]
obs = {}
if os.path.exists(obs_path):
    for line in open(obs_path, encoding="utf-8"):
        k, _, v = line.rstrip("\n").partition("=")
        obs[k] = v
def sha(p):
    if not os.path.isfile(p):
        return None
    h = hashlib.sha256()
    try:
        with open(p, "rb") as f:
            for b in iter(lambda: f.read(1 << 20), b""):
                h.update(b)
    except OSError:
        return None
    return h.hexdigest()
def dirsha(p):
    if not os.path.isdir(p):
        return None
    h = hashlib.sha256()
    for root, _, files in sorted(os.walk(p)):
        for name in sorted(files):
            fp = os.path.join(root, name)
            h.update((sha(fp) or "missing").encode())
            h.update(os.path.relpath(fp, p).encode())
    return h.hexdigest()
report = {
    "kind": "partial-coverage-synthetic-v1-report",
    "status": status,
    "failedCases": failed.split(),
    "passedCases": passed.split(),
    "failureReason": reason or None,
    "controlObservation": ctrl,
    "toolOverrides": tool == "true",
    "armS_gate": "pages[0].candidates[0] == expectedTop1 for both cases (synthetic-entry existence only)",
    "armE_note": "observation only; no absence assertion",
    "candidateDiagnostics": (json.load(open(os.path.join(os.path.dirname(out), "candidate-diagnostics.json"), encoding="utf-8"))
                             if os.path.exists(os.path.join(os.path.dirname(out), "candidate-diagnostics.json")) else None),
    "oldGate": "partial-coverage-top1-v1: NOT RUN — remains unresolved",
    "provenance": {
        "corpusSha256": sha(corpus),
        "runnerSha256": sha(runner),
        "sharedDirListingSha256": dirsha(shared),
        "probeBinSha256": sha(probe),
        "ctlBinSha256": sha(ctl),
    },
    "observations": obs,
}
with open(out, "w", encoding="utf-8") as f:
    json.dump(report, f, ensure_ascii=False, indent=2)
print(os.path.abspath(out))
PY
}
fail_run() { # $1 reason, $2 exit code
  STATUS=FAIL
  REASON="$1"
  note failureReason "$REASON"
  emit_report || true
  echo "SYNTHETIC-TOP1: FAIL ($REASON)$( [ -n "$FAILED_CASES" ] && printf ' failed:%s' "$FAILED_CASES" )"
  echo "CONTROL-OBSERVATION: $CTRL_NOTE"
  echo "OLD-GATE partial-coverage-top1-v1: NOT RUN — remains unresolved"
  exit "${2:-1}"
}

# ---- precheck: explicit dependencies (nothing assumed preinstalled) --------
command -v python3 >/dev/null 2>&1 || fail_run "python3 missing from PATH (explicit dependency, not assumed bundled)" 1
command -v shasum >/dev/null 2>&1 || fail_run "shasum missing from PATH" 1
[ -x "$PROBE" ] || fail_run "probe binary missing/not executable: $PROBE" 1
[ -x "$CTL" ] || fail_run "ctl binary missing/not executable: $CTL" 1
[ -d "$SHARED" ] || fail_run "shared Rime dir missing: $SHARED" 1
{ [ -d "$BUNDLE" ] && [ ! -L "$BUNDLE" ]; } || fail_run "bundle missing or symlink: $BUNDLE" 1
[ -f "$ROOT/script/artifact_checksum.sh" ] || fail_run "artifact_checksum.sh missing" 1
ARTIFACT_SHA="$(bash "$ROOT/script/artifact_checksum.sh" "$BUNDLE")" || fail_run "artifact_checksum.sh failed" 1
note artifactSha256 "$ARTIFACT_SHA"

# corpus precheck: exact contract shape; tamper => refuse to run
python3 - "$CORPUS" <<'PY' || fail_run "corpus precheck failed (missing/tampered)" 1
import json, sys
c = json.load(open(sys.argv[1], encoding="utf-8"))
assert c["kind"] == "partial-coverage-synthetic-v1"
assert c["formatVersion"] == 1
e = c["syntheticEntry"]
assert e["id"] == "synthetic-v1-jiukeyizuowan"
assert e["text"] == "就可以做完"
assert e["pinyin"] == "jiu ke yi zuo wan"
assert e["entryLine"] == "#@/db_name\tpinyin_simp\n#@/db_type\tuserdb\njiu ke yi zuo wan\t就可以做完\tc=1"
assert c["probePages"] == 3
assert c["cases"] == [
    {"id": "synth-partial-jiukyizuow", "query": "jiukyizuow", "expectedTop1": "就可以做完"},
    {"id": "synth-full-jiukeyizuow",  "query": "jiukeyizuow", "expectedTop1": "就可以做完"},
]
PY

probe_once() { # userdir query outjson
  local userdir="$1" query="$2" out="$3" pages="${4:-$PAGES}"
  DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT/native/core/target/release" \
    "$PROBE" --process-deep-candidate-probe "$SHARED" "$userdir" "$query" "$out" "$pages" \
    >"$WORK/probe-$(basename "$userdir")-$query.log" 2>&1
}

check_probe_shape() { # outjson query ; strict anti-contradiction checks
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
errs = []
if d.get("query") != sys.argv[2]:
    errs.append("query mismatch")
pages = d.get("pages")
if not isinstance(pages, list) or not pages:
    errs.append("pages empty")
else:
    if not (1 <= len(pages) <= 3):
        errs.append("pages count %d not in 1..3" % len(pages))
    if [p.get("pageNumber") for p in pages] != list(range(len(pages))):
        errs.append("pageNumber not sequential from 0")
    if pages[0].get("pageNumber") != 0:
        errs.append("first page pageNumber != 0")
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
if errs:
    sys.exit("probe shape invalid: " + "; ".join(errs))
PY
}

top1_of() { # outjson -> page-1 candidate #1
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1],encoding="utf-8"))["pages"][0]["candidates"][0])' "$1"
}
top5_of() { # outjson -> "/"-joined top5
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1],encoding="utf-8")); print("/".join(d["allCandidates"][:5]))' "$1"
}
case_fields() { # index key
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1],encoding="utf-8"))["cases"][int(sys.argv[2])][sys.argv[3]])' "$CORPUS" "$1" "$2"
}

# ---- Arm E: empty-library control observation (no absence assertion) -------
USER_E="$(mktemp -d "$WORK/user-E.XXXXXX")"
for CASE in 1 2; do
  QUERY="$(case_fields "$((CASE-1))" query)"
  CID="$(case_fields "$((CASE-1))" id)"
  OUT="$WORK/arm-e-$QUERY.json"
  probe_once "$USER_E" "$QUERY" "$OUT" || fail_run "arm E probe failed: $QUERY (WORK=$WORK)" 1
  [ -s "$OUT" ] || fail_run "arm E probe output missing: $QUERY" 1
  check_probe_shape "$OUT" "$QUERY" || fail_run "arm E probe shape invalid: $QUERY" 1
  note "armE_$CID" "top1=$(top1_of "$OUT");top5=$(top5_of "$OUT")"
  note "armE_${CID}_complete" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1],encoding="utf-8"))["complete"])' "$OUT")"
done
CTRL_NOTE="recorded (no absence assertion)"

# ---- Arm S: synthetic state (restore exactly one entry, then gate) ---------
USER_S="$(mktemp -d "$WORK/user-S.XXXXXX")"
python3 - "$CORPUS" >"$WORK/restore-input.json" <<'PY'
import json, sys
c = json.load(open(sys.argv[1], encoding="utf-8"))
e = c["syntheticEntry"]
print(json.dumps([{"id": e["id"], "text": e["text"], "pinyin": e["pinyin"], "entryLine": e["entryLine"]}], ensure_ascii=False))
PY
"$CTL" restore "$SHARED" "$USER_S" <"$WORK/restore-input.json" >"$WORK/restore-echo.json" 2>"$WORK/restore.err" \
  || fail_run "restore failed (WORK=$WORK, see restore.err)" 1
python3 - "$WORK/restore-echo.json" <<'PY' || fail_run "restore echo != exactly 1 item" 1
import json, sys
assert len(json.load(open(sys.argv[1], encoding="utf-8")).get("items", [])) == 1
PY
"$CTL" list "$SHARED" "$USER_S" >"$WORK/list.json" 2>"$WORK/list.err" \
  || fail_run "list failed (WORK=$WORK, see list.err)" 1
python3 - "$WORK/list.json" "$CORPUS" <<'PY' || fail_run "list readback != exactly one expected entry (count/fields/entryLine==1)" 1
import json, sys
items = json.load(open(sys.argv[1], encoding="utf-8")).get("items", [])
e = json.load(open(sys.argv[2], encoding="utf-8"))["syntheticEntry"]
assert len(items) == 1, "entry count %d != 1" % len(items)
it = items[0]
assert it.get("text") == e["text"] and it.get("pinyin") == e["pinyin"], "field mismatch"
assert it.get("entryLine") == "1", "entryLine count %r != 1" % it.get("entryLine")
PY

for CASE in 1 2; do
  QUERY="$(case_fields "$((CASE-1))" query)"
  CID="$(case_fields "$((CASE-1))" id)"
  EXP="$(case_fields "$((CASE-1))" expectedTop1)"
  OUT="$WORK/arm-s-$QUERY.json"
  probe_once "$USER_S" "$QUERY" "$OUT" || fail_run "arm S probe failed: $QUERY" 1
  [ -s "$OUT" ] || fail_run "arm S probe output missing: $QUERY" 1
  check_probe_shape "$OUT" "$QUERY" || fail_run "arm S probe shape invalid: $QUERY" 1
  TOP1="$(top1_of "$OUT")"
  note "armS_$CID" "top1=$TOP1 expectedTop1=$EXP"
  if [ "$TOP1" = "$EXP" ]; then
    PASSED_CASES="$PASSED_CASES $CID"
  else
    FAILED_CASES="$FAILED_CASES $CID"
    echo "ASSERT-FAIL: $CID top1=$TOP1 expectedTop1=$EXP" >&2
  fi
done

# Candidate-set diagnostics are separate from the synthetic Top-1 gate.
# Scan beyond three pages using the same existing probe; never infer absence
# from a bounded scan unless the probe reports the last page was reached.
for ARM in e s; do
  if [ "$ARM" = e ]; then USER_DIR="$USER_E"; else USER_DIR="$USER_S"; fi
  for CASE in 0 1; do
    QUERY="$(case_fields "$CASE" query)"
    probe_once "$USER_DIR" "$QUERY" "$WORK/deep-$ARM-$QUERY.json" 256 \
      || fail_run "deep diagnostic probe failed: $ARM/$QUERY" 1
  done
done
python3 - "$WORK" "$CORPUS" "$ROOT/quality/corpus/partial-coverage-top1-v1.json" <<'PYDIAG' \
  || fail_run "candidate diagnostics failed" 1
import json, pathlib, sys, unicodedata
work = pathlib.Path(sys.argv[1])
corpus = json.load(open(sys.argv[2], encoding="utf-8"))
frozen = json.load(open(sys.argv[3], encoding="utf-8"))
expected = {c["query"]: c["expectedSet"] for c in frozen["cases"] if "expectedSet" in c}
norm = lambda s: unicodedata.normalize("NFKC", s).strip()
rows = []
for arm in ("e", "s"):
    for case in corpus["cases"]:
        q = case["query"]
        short = json.loads((work / f"arm-{arm}-{q}.json").read_text())
        deep = json.loads((work / f"deep-{arm}-{q}.json").read_text())
        assert deep["query"] == q and deep["pages"]
        assert deep["allCandidates"] == [x for p in deep["pages"] for x in p["candidates"]]
        assert deep["pages"][:len(short["pages"])] == short["pages"], "deep/short prefix drift"
        first = [norm(x) for x in short["pages"][0]["candidates"][:5]]
        all3 = [norm(x) for x in short["allCandidates"]]
        all_deep = [norm(x) for x in deep["allCandidates"]]
        exp = expected[q]
        rows.append(dict(arm=arm, query=q, top1Pass=first[0] == case["expectedTop1"],
            observedFirstPageTop5=first, expectedSet=exp,
            missingFromTop5=sorted(set(exp)-set(first)), addedToTop5=sorted(set(first)-set(exp)),
            rawTextTop5SetMatches=set(first)==set(exp),
            ranks={x: dict(first3PagesRank=all3.index(x)+1 if x in all3 else None,
                           deepRank=all_deep.index(x)+1 if x in all_deep else None)
                   for x in exp}, deepPages=len(deep["pages"]), deepComplete=deep["complete"],
            rankNote="1-based flattened displayed rank; null with deepComplete=false means unobserved, not absent"))
result = dict(note="Raw displayed text comparison; existing probe lacks reading keys required by formal quality dedup. Original gate NOT run.", cases=rows)
(work / "candidate-diagnostics.json").write_text(json.dumps(result, ensure_ascii=False, indent=2)+"\n")
PYDIAG

if [ -z "${FAILED_CASES// /}" ] && [ -n "${PASSED_CASES// /}" ]; then STATUS=PASS; fi
emit_report
echo "SYNTHETIC-TOP1: $STATUS$PASSED_CASES$( [ -n "$FAILED_CASES" ] && printf ' failed:%s' "$FAILED_CASES" )"
echo "CONTROL-OBSERVATION: $CTRL_NOTE"
echo "OLD-GATE partial-coverage-top1-v1: NOT RUN — remains unresolved"
[ "$STATUS" = "PASS" ]
