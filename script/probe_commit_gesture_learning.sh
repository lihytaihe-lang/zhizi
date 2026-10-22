#!/usr/bin/env bash
# Real-librime learning write matrix for commit gestures and the
# requires_regular_selection multi-segment exception.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

BUNDLE="$ROOT_DIR/build/native/release/Agent Input Method.inputmethod"
SHARED_RIME="$BUNDLE/Contents/Resources/Rime"
LIVE_USER_RIME="$HOME/Library/Application Support/AgentInputMethod/Rime"
CORPUS="$ROOT_DIR/quality/corpus/short-input-learning-probe-v1.json"
SECONDARY_FIXTURE="$ROOT_DIR/quality/fixtures/learning-gesture-secondary-v1.json"
OUTPUT_DIR="${1:-$ROOT_DIR/build/probe/commit-gesture-learning}"

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release bundle: $BUNDLE" >&2; exit 2; }
[[ -d "$LIVE_USER_RIME" && ! -L "$LIVE_USER_RIME" ]] || {
  echo "missing owner user rime: $LIVE_USER_RIME" >&2
  exit 2
}
[[ -f "$CORPUS" ]] || { echo "missing corpus: $CORPUS" >&2; exit 2; }
[[ -f "$SECONDARY_FIXTURE" ]] || { echo "missing fixture: $SECONDARY_FIXTURE" >&2; exit 2; }

LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib" \
  cargo build --manifest-path "$ROOT_DIR/native/core/agent-ime-core/Cargo.toml" \
  --release --features librime >/dev/null
touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEUserDBHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --show-bin-path)"
HARNESS="$BIN_DIR/AgentIMEUserDBHarness"
USERDB_CTL="$BUNDLE/Contents/MacOS/AgentIMEUserDBCtl"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-commit-gesture.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
mkdir -p "$OUTPUT_DIR" "$TEMP_ROOT/owner-before" "$TEMP_ROOT/owner-after"

list_entries() {
  local user_directory="$1" output="$2"
  mkdir -p "$user_directory"
  DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$USERDB_CTL" list \
    "$SHARED_RIME" "$user_directory" |
    jq -S '{items: [.items[] | {pinyin, text, entryLine}] | sort_by(.pinyin, .text)}' \
      > "$output"
}

entry_digest() {
  jq -S -c '.items' "$1" | shasum -a 256 | awk '{print $1}'
}

logical_diff() {
  local before="$1" after="$2" output="$3"
  jq -n --slurpfile before "$before" --slurpfile after "$after" '
    def keyed:
      map({key: (.pinyin + "\u0000" + .text), value: .}) | from_entries;
    ($before[0].items | keyed) as $b |
    ($after[0].items | keyed) as $a |
    {
      added: [
        ($a | keys[]) as $key |
        select($b[$key] == null) |
        $a[$key]
      ],
      removed: [
        ($b | keys[]) as $key |
        select($a[$key] == null) |
        $b[$key]
      ],
      valueChanged: [
        ($a | keys[]) as $key |
        select($b[$key] != null and $b[$key].entryLine != $a[$key].entryLine) |
        {
          pinyin: $a[$key].pinyin,
          text: $a[$key].text,
          before: $b[$key].entryLine,
          after: $a[$key].entryLine
        }
      ]
    } |
    . + {
      addedCount: (.added | length),
      removedCount: (.removed | length),
      valueChangedCount: (.valueChanged | length)
    }
  ' > "$output"
}

# The live owner directory is copied only. UserDBCtl and the harness receive
# private copies or isolated empty roots, never the live path.
cp -R "$LIVE_USER_RIME"/. "$TEMP_ROOT/owner-before/"
chmod -R u+rwX "$TEMP_ROOT/owner-before"
list_entries "$TEMP_ROOT/owner-before" "$TEMP_ROOT/owner-before.json"
OWNER_ENTRY_DIGEST_BEFORE="$(entry_digest "$TEMP_ROOT/owner-before.json")"
OWNER_ENTRY_COUNT="$(jq '.items | length' "$TEMP_ROOT/owner-before.json")"

GESTURES=(
  space-top1
  arrow-space
  digit-1
  digit-2
  digit-3
  digit-4
  digit-5
  digit-6
  mouse-top1
  mouse-nontop1
  punctuation
  focus-lost
  enter
  single-segment-nolearn
  multi-segment-prefix-control
  multi-segment-nolearn
)

TIMESTAMP=9200000000
for gesture in "${GESTURES[@]}"; do
  runtime="$TEMP_ROOT/runtime-$gesture"
  mkdir -p "$runtime/Rime"
  list_entries "$runtime/Rime" "$OUTPUT_DIR/$gesture.before.json"
  DYLD_LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib" \
    "$HARNESS" --learning-gesture "$SHARED_RIME" "$runtime" "$gesture" \
    "$OUTPUT_DIR/$gesture.result.json" "$TIMESTAMP" "$SECONDARY_FIXTURE"
  list_entries "$runtime/Rime" "$OUTPUT_DIR/$gesture.after.json"
  logical_diff "$OUTPUT_DIR/$gesture.before.json" "$OUTPUT_DIR/$gesture.after.json" \
    "$OUTPUT_DIR/$gesture.diff.json"
  TIMESTAMP=$((TIMESTAMP + 100))
done

# The multi-segment final Space is measured against a cloned clean-prefix
# control, so the deliberate prefix confirmation is excluded from its delta.
logical_diff "$OUTPUT_DIR/multi-segment-prefix-control.after.json" \
  "$OUTPUT_DIR/multi-segment-nolearn.after.json" \
  "$OUTPUT_DIR/multi-segment-nolearn.incremental.diff.json"

cp -R "$LIVE_USER_RIME"/. "$TEMP_ROOT/owner-after/"
chmod -R u+rwX "$TEMP_ROOT/owner-after"
list_entries "$TEMP_ROOT/owner-after" "$TEMP_ROOT/owner-after.json"
OWNER_ENTRY_DIGEST_AFTER="$(entry_digest "$TEMP_ROOT/owner-after.json")"
OWNER_ENTRY_COUNT_AFTER="$(jq '.items | length' "$TEMP_ROOT/owner-after.json")"

ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
PRODUCT_COMMIT="$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEProductCommit' \
  "$BUNDLE/Contents/Info.plist")"
PRODUCT_INPUTS_SHA="$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEProductInputsSHA256' \
  "$BUNDLE/Contents/Info.plist")"
CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"

jq -n \
  --arg artifact "$ARTIFACT_SHA" \
  --arg productCommit "$PRODUCT_COMMIT" \
  --arg productInputs "$PRODUCT_INPUTS_SHA" \
  --arg corpus "quality/corpus/short-input-learning-probe-v1.json" \
  --arg corpusChecksum "$CORPUS_SHA" \
  --arg ownerBefore "$OWNER_ENTRY_DIGEST_BEFORE" \
  --arg ownerAfter "$OWNER_ENTRY_DIGEST_AFTER" \
  --argjson ownerCount "$OWNER_ENTRY_COUNT" \
  --argjson ownerCountAfter "$OWNER_ENTRY_COUNT_AFTER" \
  --slurpfile space "$OUTPUT_DIR/space-top1.result.json" \
  --slurpfile spaceDiff "$OUTPUT_DIR/space-top1.diff.json" \
  --slurpfile arrow "$OUTPUT_DIR/arrow-space.result.json" \
  --slurpfile arrowDiff "$OUTPUT_DIR/arrow-space.diff.json" \
  --slurpfile d1 "$OUTPUT_DIR/digit-1.result.json" \
  --slurpfile d1Diff "$OUTPUT_DIR/digit-1.diff.json" \
  --slurpfile d2 "$OUTPUT_DIR/digit-2.result.json" \
  --slurpfile d2Diff "$OUTPUT_DIR/digit-2.diff.json" \
  --slurpfile d3 "$OUTPUT_DIR/digit-3.result.json" \
  --slurpfile d3Diff "$OUTPUT_DIR/digit-3.diff.json" \
  --slurpfile d4 "$OUTPUT_DIR/digit-4.result.json" \
  --slurpfile d4Diff "$OUTPUT_DIR/digit-4.diff.json" \
  --slurpfile d5 "$OUTPUT_DIR/digit-5.result.json" \
  --slurpfile d5Diff "$OUTPUT_DIR/digit-5.diff.json" \
  --slurpfile d6 "$OUTPUT_DIR/digit-6.result.json" \
  --slurpfile d6Diff "$OUTPUT_DIR/digit-6.diff.json" \
  --slurpfile mouse1 "$OUTPUT_DIR/mouse-top1.result.json" \
  --slurpfile mouse1Diff "$OUTPUT_DIR/mouse-top1.diff.json" \
  --slurpfile mouse2 "$OUTPUT_DIR/mouse-nontop1.result.json" \
  --slurpfile mouse2Diff "$OUTPUT_DIR/mouse-nontop1.diff.json" \
  --slurpfile punctuation "$OUTPUT_DIR/punctuation.result.json" \
  --slurpfile punctuationDiff "$OUTPUT_DIR/punctuation.diff.json" \
  --slurpfile focus "$OUTPUT_DIR/focus-lost.result.json" \
  --slurpfile focusDiff "$OUTPUT_DIR/focus-lost.diff.json" \
  --slurpfile enter "$OUTPUT_DIR/enter.result.json" \
  --slurpfile enterDiff "$OUTPUT_DIR/enter.diff.json" \
  --slurpfile single "$OUTPUT_DIR/single-segment-nolearn.result.json" \
  --slurpfile singleDiff "$OUTPUT_DIR/single-segment-nolearn.diff.json" \
  --slurpfile multi "$OUTPUT_DIR/multi-segment-nolearn.result.json" \
  --slurpfile multiDiff "$OUTPUT_DIR/multi-segment-nolearn.incremental.diff.json" '
  def row($result; $diff):
    {
      gesture: $result[0].gesture,
      commitText: $result[0].commitText,
      selectionProbe: $result[0].selectionProbe,
      diff: $diff[0]
    };
  {
    kind: "commit-gesture-learning-write-matrix",
    artifactChecksum: $artifact,
    productCommit: $productCommit,
    productInputsSHA256: $productInputs,
    corpusPath: $corpus,
    corpusChecksum: $corpusChecksum,
    ownerEntryCount: $ownerCount,
    ownerEntryCountAfter: $ownerCountAfter,
    ownerEntryDigestBefore: $ownerBefore,
    ownerEntryDigestAfter: $ownerAfter,
    ownerEntriesUnchanged: ($ownerBefore == $ownerAfter),
    gestures: [
      row($space; $spaceDiff),
      row($arrow; $arrowDiff),
      row($d1; $d1Diff),
      row($d2; $d2Diff),
      row($d3; $d3Diff),
      row($d4; $d4Diff),
      row($d5; $d5Diff),
      row($d6; $d6Diff),
      row($mouse1; $mouse1Diff),
      row($mouse2; $mouse2Diff),
      row($punctuation; $punctuationDiff),
      row($focus; $focusDiff),
      row($enter; $enterDiff)
    ],
    segmentPreview: [
      row($single; $singleDiff),
      row($multi; $multiDiff)
    ]
  }' > "$OUTPUT_DIR/matrix.json"

jq -e '.ownerEntriesUnchanged == true' "$OUTPUT_DIR/matrix.json" >/dev/null || {
  echo "ownerEntriesUnchanged=false; discard this run and rerun while the owner database is idle" >&2
  exit 3
}

jq -r '
  "commit_gesture_learning_probe=DONE owner_entries=\(.ownerEntryCount) owner_entries_unchanged=\(.ownerEntriesUnchanged) artifact=\(.artifactChecksum)",
  "",
  (["gesture", "reinforce", "requires_regular_selection", "added", "removed", "value_changed", "commit"] | @tsv),
  ((.gestures + .segmentPreview)[] |
    [
      .gesture,
      (if .selectionProbe.reinforce == null then "n/a" else .selectionProbe.reinforce end),
      (if .selectionProbe.requiresRegularSelection == null then "n/a" else .selectionProbe.requiresRegularSelection end),
      .diff.addedCount,
      .diff.removedCount,
      .diff.valueChangedCount,
      .commitText
    ] | @tsv)
' "$OUTPUT_DIR/matrix.json"
