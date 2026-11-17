#!/usr/bin/env bash
# A missing real model, all-timeout run, mismatched page or wrong digit commit is a failure.
# Public synthetic cases only; no owner typing records are read and preferences are not changed.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:?usage: test_zhiju_delivery.sh <release-bundle> <release-harness> <report-dir>}"
HARNESS="${2:?missing release harness}"
REPORT_DIR="${3:?missing report directory}"
[[ $# -eq 3 && -d "$BUNDLE" && -x "$HARNESS" ]] || exit 2
mkdir -p "$REPORT_DIR"
REPORT_DIR="$(cd "$REPORT_DIR" && pwd)"
(
  cd "$BUNDLE/Contents/Resources/ZhijuModel"
  shasum -a 256 -c "$ROOT_DIR/native/resources/ZhijuModel/SHA256SUMS"
) > "$REPORT_DIR/model-checksums.log"
for mode in off on; do
  "$HARNESS" --zhiju-delivery "$BUNDLE/Contents/Resources/Rime" \
    "$ROOT_DIR/quality/corpus/zhiju-delivery-v1.json" "$REPORT_DIR/$mode.json" "$mode"
  jq -e --arg version "$(cat "$ROOT_DIR/VERSION")" \
    '.passed == true and .modelReady == true and .artifactVersion == $version and
     (.rows | length) == 8 and all(.rows[]; .pageMatchesCore and .digitSelectionMatchesDisplay)' \
    "$REPORT_DIR/$mode.json" >/dev/null
done
jq -e 'all(.rows[]; .zhijuOutcome != "applied")' "$REPORT_DIR/off.json" >/dev/null
jq -e 'any(.rows[]; .zhijuOutcome == "applied" and .comments[0] == "整句 · 知句")' "$REPORT_DIR/on.json" >/dev/null
"$HARNESS" --zhiju "$BUNDLE/Contents/Resources/Rime" "$REPORT_DIR/search.json"
jq -e '.status == "measured" and (.searchMilliseconds | length) == 20 and
       .p95Milliseconds <= 40 and .outcomes.applied > 0' "$REPORT_DIR/search.json" >/dev/null
"$HARNESS" --zhiju-delivery "$BUNDLE/Contents/Resources/Rime" \
  "$ROOT_DIR/quality/corpus/coverage-delivery-0.3.34.json" "$REPORT_DIR/coverage.json" off
jq -e '.passed == true and (.rows | length) == 17 and
  all(.rows[]; .pageMatchesCore and .digitSelectionMatchesDisplay)' "$REPORT_DIR/coverage.json" >/dev/null
python3 - "$REPORT_DIR/coverage.json" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))["rows"]
assert all(row["expected"] in row["displayedPage"][:5] for row in rows[:8])
assert all(row["selectedDigit"] == 5 and row["committed"] == row["displayedPage"][4] for row in rows[8:16])
assert rows[16]["selectionDisposition"] == "marked_segment"
PY
for mode in off on; do
  "$HARNESS" --zhiju-fifth-delivery "$BUNDLE/Contents/Resources/Rime" \
    "$ROOT_DIR/quality/corpus/fifth-selection-delivery-0.3.35.json" "$REPORT_DIR/fifth-$mode.json" "$mode"
done
python3 - "$REPORT_DIR/fifth-off.json" "$REPORT_DIR/fifth-on.json" <<'PY'
import json, sys
off, on = [json.load(open(path)) for path in sys.argv[1:]]
assert all(report["passed"] and len(report["rows"]) == 5 for report in (off, on))
assert on["requiredApplication"] == "fifth_offer_committed"
assert all(row["pageMatchesCore"] and row["digitSelectionMatchesDisplay"]
           for report in (off, on) for row in report["rows"])
assert on["rows"][3]["displayedPage"][0] == "生日快乐"
row = on["rows"][4]
assert row["displayedPage"][:4] == off["rows"][4]["displayedPage"][:4]
assert row["displayedPage"][4] == row["committed"] == "输入框里"
assert row["selectionMarkedText"] == "" and row["selectedDigit"] == 5
assert row["zhijuOutcome"] == "offered" and row["comments"][4] == "整句 · 知句备选"
assert all(row["zhijuOutcome"] != "offered" for row in off["rows"])
PY
"$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE" > "$REPORT_DIR/bundle.sha256"
echo "zhiju_delivery=PASS (real model; controller page; digit commit; search p95 <= 40 ms) reports=$REPORT_DIR"
