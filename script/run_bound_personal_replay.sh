#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$#" -ne 5 ]]; then
  echo "usage: $0 <input-bundle> <immutable-user-rime-snapshot> <golden.json> <candidate-lexicon.json> <report.json>" >&2
  exit 2
fi

BUNDLE="$1"
USERDB_SNAPSHOT="$2"
GOLDEN="$3"
LEXICON="$4"
REPORT="$5"
HARNESS="$BUNDLE/Contents/MacOS/AgentIMEPersonalReplayHarness"
INFO_PLIST="$BUNDLE/Contents/Info.plist"

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "invalid input bundle" >&2; exit 3; }
[[ -d "$USERDB_SNAPSHOT" && ! -L "$USERDB_SNAPSHOT" ]] || { echo "invalid UserDB snapshot" >&2; exit 3; }
[[ -f "$GOLDEN" && ! -L "$GOLDEN" ]] || { echo "invalid golden set" >&2; exit 3; }
[[ -f "$LEXICON" && ! -L "$LEXICON" ]] || { echo "invalid candidate lexicon" >&2; exit 3; }
[[ -x "$HARNESS" ]] || { echo "missing personal replay harness" >&2; exit 3; }
[[ -z "$(find "$USERDB_SNAPSHOT" -type l -print -quit)" ]] || { echo "UserDB snapshot contains symlinks" >&2; exit 3; }

ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
USERDB_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$USERDB_SNAPSHOT")"
GOLDEN_SHA="$(shasum -a 256 "$GOLDEN" | awk '{print $1}')"
LEXICON_SHA="$(shasum -a 256 "$LEXICON" | awk '{print $1}')"
PRODUCT_COMMIT="$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEProductCommit' "$INFO_PLIST")"
PRODUCT_INPUTS_SHA="$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEProductInputsSHA256' "$INFO_PLIST")"

[[ "$PRODUCT_COMMIT" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid bundle ProductCommit" >&2; exit 3; }
[[ "$PRODUCT_INPUTS_SHA" =~ ^[0-9a-f]{64}$ ]] || { echo "invalid bundle ProductInputsSHA256" >&2; exit 3; }
EXPECTED_INPUTS_SHA="$("$ROOT_DIR/script/rc_gate_support.sh" fingerprint "$ROOT_DIR" "$PRODUCT_COMMIT")"
[[ "$PRODUCT_INPUTS_SHA" == "$EXPECTED_INPUTS_SHA" ]] || {
  echo "bundle ProductInputsSHA256 does not match ProductCommit" >&2
  exit 3
}

REPORT_PARENT="$(dirname "$REPORT")"
mkdir -p "$REPORT_PARENT"
REPORT_PARENT="$(cd "$REPORT_PARENT" && pwd -P)"
REPORT="$REPORT_PARENT/$(basename "$REPORT")"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-bound-personal-replay.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
mkdir -p "$TEMP_ROOT/user-rime"
cp -R "$USERDB_SNAPSHOT"/. "$TEMP_ROOT/user-rime/"

DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$HARNESS" \
  "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/user-rime" "$GOLDEN" "$LEXICON" \
  "$REPORT" 5 "$ARTIFACT_SHA" "$PRODUCT_COMMIT" "$PRODUCT_INPUTS_SHA" "$USERDB_SHA"

jq -e \
  --arg artifact "$ARTIFACT_SHA" \
  --arg productCommit "$PRODUCT_COMMIT" \
  --arg productInputs "$PRODUCT_INPUTS_SHA" \
  --arg userdb "$USERDB_SHA" \
  --arg golden "$GOLDEN_SHA" \
  --arg candidate "$LEXICON_SHA" '
  .schemaVersion == 2 and
  .artifactChecksum == $artifact and
  .productCommit == $productCommit and
  .productInputsSHA256 == $productInputs and
  .userDBChecksum == $userdb and
  .goldenChecksum == $golden and
  .candidateChecksum == $candidate and
  .redactionState == "redacted-no-input-or-candidate-text" and
  (.generatedAt | type == "string" and length > 0) and
  (.privacyRule | contains("no pinyin or candidate text"))
' "$REPORT" >/dev/null
[[ "$(stat -f '%Lp' "$REPORT")" == "600" ]] || { echo "report permissions are not 0600" >&2; exit 4; }

jq -r --arg report "$REPORT" --arg checksum "$(shasum -a 256 "$REPORT" | awk '{print $1}')" '
  "bound_personal_replay=PASS report_status=\(.status) samples=\(.sampleCount) artifact_checksum=\(.artifactChecksum) report_checksum=\($checksum) report=\($report)"
' "$REPORT"
