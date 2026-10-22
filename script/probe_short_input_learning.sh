#!/usr/bin/env bash
# Short-input A/B probe: the same engine and the same dictionaries, with only
# the user database swapped.
#
# This exists because the owner's short-input ranking was reported as degrading
# with use, and the only evidence on record was three hand-copied examples. A
# claim about learning damage has to be reproducible before any learning
# behaviour is changed, so this freezes the comparison instead.
#
# The owner's real user database is NEVER handed to the harness and NEVER
# written to: it is copied to a private scratch directory first, because
# librime writes to whatever user directory it is given and a probe that
# pollutes the thing it is measuring is worthless.
#
# usage: probe_short_input_learning.sh [output-directory] [probe-corpus]
# Set AIM_SWIFTPM_SANDBOX_WORKAROUND=1 only in restricted execution
# environments that cannot use SwiftPM's default sandbox/cache locations.
# Unset/default preserves the repository's normal Swift build invocation.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

BUNDLE="$ROOT_DIR/build/native/release/Agent Input Method.inputmethod"
CORPUS="${2:-$ROOT_DIR/quality/corpus/short-input-learning-probe-v1.json}"
LIVE_USER_RIME="$HOME/Library/Application Support/AgentInputMethod/Rime"
OUTPUT_DIR="${1:-$ROOT_DIR/build/probe/short-input-learning}"
REPETITIONS=5

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing release bundle: $BUNDLE" >&2; exit 2; }
[[ -f "$CORPUS" ]] || { echo "missing probe corpus: $CORPUS" >&2; exit 2; }
[[ -d "$LIVE_USER_RIME" ]] || { echo "missing owner user rime: $LIVE_USER_RIME" >&2; exit 2; }

if [[ "${AIM_SWIFTPM_SANDBOX_WORKAROUND:-0}" == "1" ]]; then
  SWIFT_MODULE_CACHE="${TMPDIR:-/tmp}/agent-ime-short-input-swift-module-cache"
  SWIFT_SCRATCH_PATH="${TMPDIR:-/tmp}/agent-ime-short-input-swift-build"
  mkdir -p "$SWIFT_MODULE_CACHE" "$SWIFT_SCRATCH_PATH"
  touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
  CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" \
    SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" \
    AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --disable-sandbox --scratch-path "$SWIFT_SCRATCH_PATH" \
    --product AgentIMEReplayHarness >/dev/null
  BIN_DIR="$(CLANG_MODULE_CACHE_PATH="$SWIFT_MODULE_CACHE" \
    SWIFTPM_MODULECACHE_OVERRIDE="$SWIFT_MODULE_CACHE" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --disable-sandbox --scratch-path "$SWIFT_SCRATCH_PATH" \
    --show-bin-path)"
else
  touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
  AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --product AgentIMEReplayHarness >/dev/null
  BIN_DIR="$(swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --show-bin-path)"
fi
HARNESS="$BIN_DIR/AgentIMEReplayHarness"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-short-input-probe.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
mkdir -p "$OUTPUT_DIR"

# Arm A: a copy of the owner's live user database, taken once so both the
# entry listing and the replay describe the same state.
mkdir -p "$TEMP_ROOT/owner-user"
cp -R "$LIVE_USER_RIME"/. "$TEMP_ROOT/owner-user/"
chmod -R u+rwX "$TEMP_ROOT/owner-user"

# Compare logical entries, not raw bytes. librime compacts its LevelDB on every
# open, so a file checksum always differs afterwards and would make the
# integrity guard useless. What must not change is the learned entry set.
USERDB_CTL="$BUNDLE/Contents/MacOS/AgentIMEUserDBCtl"
entry_digest() {
  DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$USERDB_CTL" list \
    "$BUNDLE/Contents/Resources/Rime" "$1" |
    jq -S -c '[.items[] | {pinyin, text, entryLine}] | sort' | shasum -a 256 | awk '{print $1}'
}
OWNER_ENTRY_DIGEST="$(entry_digest "$TEMP_ROOT/owner-user")"
OWNER_ENTRY_COUNT="$(
  DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$USERDB_CTL" list \
    "$BUNDLE/Contents/Resources/Rime" "$TEMP_ROOT/owner-user" | jq '.items | length'
)"

# Arm B: an empty user directory, so librime starts from the shipped
# dictionaries with nothing learned.
mkdir -p "$TEMP_ROOT/clean-user"

ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
PRODUCT_COMMIT="$(/usr/libexec/PlistBuddy -c 'Print :AgentIMEProductCommit' "$BUNDLE/Contents/Info.plist")"

run_arm() {
  local user_directory="$1" report="$2"
  DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" \
    "$HARNESS" "$BUNDLE/Contents/Resources/Rime" "$user_directory" \
    "$CORPUS" "$report" "$REPETITIONS" "$ARTIFACT_SHA" >/dev/null
}

run_arm "$TEMP_ROOT/owner-user" "$OUTPUT_DIR/owner.json"
run_arm "$TEMP_ROOT/clean-user" "$OUTPUT_DIR/clean.json"

# The learned entry set must be unchanged by the replay. If replaying taught the
# database anything, the comparison measured a moving target and the run is void.
OWNER_ENTRY_DIGEST_AFTER="$(entry_digest "$TEMP_ROOT/owner-user")"

jq -n \
  --slurpfile owner "$OUTPUT_DIR/owner.json" \
  --slurpfile clean "$OUTPUT_DIR/clean.json" \
  --arg artifact "$ARTIFACT_SHA" \
  --arg commit "$PRODUCT_COMMIT" \
  --arg userdb "$OWNER_ENTRY_DIGEST" \
  --arg userdbAfter "$OWNER_ENTRY_DIGEST_AFTER" \
  --arg entryCount "$OWNER_ENTRY_COUNT" \
  --arg repetitions "$REPETITIONS" '
  ($owner[0].observations | map({key: .id, value: .}) | from_entries) as $ownerById |
  ($clean[0].observations | map({key: .id, value: .}) | from_entries) as $cleanById |
  {
    kind: "short-input-learning-probe",
    artifactChecksum: $artifact,
    productCommit: $commit,
    ownerEntryDigestBefore: $userdb,
    ownerEntryDigestAfter: $userdbAfter,
    ownerEntryCount: ($entryCount | tonumber),
    ownerEntriesUnchanged: ($userdb == $userdbAfter),
    repetitions: ($repetitions | tonumber),
    cases: ($cleanById | keys | length),
    comparisons: [
      $cleanById | keys[] as $id |
      {
        id: $id,
        query: $cleanById[$id].query,
        clean: $cleanById[$id].actual,
        owner: $ownerById[$id].actual,
        top1Differs: ($cleanById[$id].actual[0] != $ownerById[$id].actual[0]),
        orderDiffers: ($cleanById[$id].actual != $ownerById[$id].actual)
      }
    ]
  } |
  . + {
    top1Differences: ([.comparisons[] | select(.top1Differs)] | length),
    orderDifferences: ([.comparisons[] | select(.orderDiffers)] | length)
  }
' > "$OUTPUT_DIR/comparison.json"

jq -r '
  "short_input_learning_probe=DONE cases=\(.cases) top1_differs=\(.top1Differences) order_differs=\(.orderDifferences) owner_entries=\(.ownerEntryCount) owner_entries_unchanged=\(.ownerEntriesUnchanged) artifact=\(.artifactChecksum)",
  "",
  (["query", "clean (factory)", "owner (learned)", "top1?"] | @tsv),
  (.comparisons[] | [.query, (.clean[0:5] | join(" ")), (.owner[0:5] | join(" ")), (if .top1Differs then "DIFFERS" else "same" end)] | @tsv)
' "$OUTPUT_DIR/comparison.json"
