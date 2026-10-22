#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
SHARED_RIME="$BUNDLE/Contents/Resources/Rime"
LIVE_USER_RIME="$HOME/Library/Application Support/AgentInputMethod/Rime"
CORPUS="$ROOT_DIR/quality/corpus/dakle-owner-hit-v1.json"
SECONDARY_FIXTURE="$ROOT_DIR/quality/fixtures/learning-gesture-secondary-v1.json"
FROZEN_CORPUS_SHA="54a3b05df78a2e91b46bea6bf062dbacda1e9cf56be76692a0877223839b7baa"

[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || {
  echo "missing release input bundle: $BUNDLE" >&2
  exit 2
}
[[ -d "$LIVE_USER_RIME" && ! -L "$LIVE_USER_RIME" ]] || {
  echo "missing owner Rime directory: $LIVE_USER_RIME" >&2
  exit 2
}
ACTUAL_CORPUS_SHA="$(shasum -a 256 "$CORPUS" | awk '{print $1}')"
[[ "$ACTUAL_CORPUS_SHA" == "$FROZEN_CORPUS_SHA" ]] || {
  echo "dakle corpus checksum mismatch: expected $FROZEN_CORPUS_SHA, got $ACTUAL_CORPUS_SHA" >&2
  exit 6
}
jq -e '
  .kind == "dakle-owner-hit-regression" and
  (.cases | length) == 10 and
  ([.cases[] | select(.id == "owner-dakle" and .allowed == ["打开了"]
    and .mustPassTop1 == true)] | length) == 1 and
  ([.cases[] | select(.category == "same-class-observation")] | length) == 3 and
  ([.cases[] | select(.category == "working-control")] | length) == 6
' "$CORPUS" >/dev/null

LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib" \
  cargo build --manifest-path "$ROOT_DIR/native/core/agent-ime-core/Cargo.toml" \
  --release --features librime >/dev/null
touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEUserDBHarness >/dev/null
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEReplayHarness >/dev/null
BIN_DIR="$(swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --show-bin-path)"
USERDB_HARNESS="$BIN_DIR/AgentIMEUserDBHarness"
REPLAY_HARNESS="$BIN_DIR/AgentIMEReplayHarness"
USERDB_CTL="$BUNDLE/Contents/MacOS/AgentIMEUserDBCtl"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/agent-ime-dakle-owner-hit.XXXXXX")"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT

list_entries() {
  local user_directory="$1" output="$2"
  DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$USERDB_CTL" list \
    "$SHARED_RIME" "$user_directory" |
    jq -S '[.items[] | {pinyin, text, entryLine}] | sort_by(.pinyin, .text)' > "$output"
}
entry_digest() {
  jq -S -c '.' "$1" | shasum -a 256 | awk '{print $1}'
}

# Read the owner's live UserDB exactly once. The owner may be typing while the suite runs,
# so every arm below works from this snapshot and the live directory is never passed to
# a harness; the final check proves no harness wrote to its source snapshot.
OWNER_SOURCE="$TEMP_ROOT/owner-source"
mkdir -p "$OWNER_SOURCE" "$TEMP_ROOT/owner-before" "$TEMP_ROOT/owner-after"
cp -R "$LIVE_USER_RIME"/. "$OWNER_SOURCE/"
chmod -R a-w "$OWNER_SOURCE"
cp -R "$OWNER_SOURCE"/. "$TEMP_ROOT/owner-before/"
chmod -R u+w "$TEMP_ROOT/owner-before"
list_entries "$TEMP_ROOT/owner-before" "$TEMP_ROOT/owner-before.json"
OWNER_DIGEST_BEFORE="$(entry_digest "$TEMP_ROOT/owner-before.json")"
OWNER_COUNT_BEFORE="$(jq 'length' "$TEMP_ROOT/owner-before.json")"

REPORT="$TEMP_ROOT/owner-report.json"
ARTIFACT_SHA="$("$ROOT_DIR/script/artifact_checksum.sh" "$BUNDLE")"
DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks:$ROOT_DIR/native/core/target/release" \
  "$REPLAY_HARNESS" "$SHARED_RIME" "$TEMP_ROOT/owner-before" \
  "$CORPUS" "$REPORT" 5 "$ARTIFACT_SHA" >/dev/null
jq -e '
  .cases == 10 and .attemptedSessions == 50 and
  .createdSessions == 50 and .executedSessions == 50 and
  .mustPassTop1Cases == 1 and .mustPassTop1Hits == 1 and
  .stableCases == 10 and (.failures | length) == 0 and
  (.observations | map(select(.id == "owner-dakle")) | .[0].actual[0]) == "打开了"
' "$REPORT" >/dev/null
list_entries "$TEMP_ROOT/owner-before" "$TEMP_ROOT/query-after.json"
cmp -s "$TEMP_ROOT/owner-before.json" "$TEMP_ROOT/query-after.json" || {
  echo "dakle_owner_hit=FAIL query changed owner-copy logical entries" >&2
  exit 1
}

run_commit_arm() {
  local attempt root output result
  for attempt in 1 2 3; do
    root="$TEMP_ROOT/commit-attempt-$attempt"
    output="$TEMP_ROOT/commit-attempt-$attempt.json"
    mkdir -p "$root/Rime"
    cp -R "$OWNER_SOURCE"/. "$root/Rime/"
    chmod -R u+w "$root/Rime"
    list_entries "$root/Rime" "$TEMP_ROOT/commit-before-$attempt.json"
    set +e
    DYLD_LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib" \
      "$USERDB_HARNESS" --learning-gesture "$SHARED_RIME" "$root" \
      owner-verified-dakle "$output" 9400000000 "$SECONDARY_FIXTURE" >/dev/null
    result=$?
    set -e
    if [[ "$result" -eq 0 ]]; then
      COMMIT_ROOT="$root"
      COMMIT_OUTPUT="$output"
      COMMIT_BEFORE="$TEMP_ROOT/commit-before-$attempt.json"
      return 0
    fi
    if [[ "$result" -ne 139 ]]; then
      echo "dakle_owner_hit_harness=FAIL exit=$result" >&2
      return "$result"
    fi
    echo "dakle_owner_hit_harness_restart=$attempt signal=SIGSEGV isolated_root_discarded=true" >&2
    sleep "$attempt"
  done
  echo "dakle_owner_hit_harness=FAIL sigsegv_attempts=3" >&2
  return 139
}

COMMIT_ROOT=""
COMMIT_OUTPUT=""
COMMIT_BEFORE=""
run_commit_arm
list_entries "$COMMIT_ROOT/Rime" "$TEMP_ROOT/commit-after.json"
cmp -s "$COMMIT_BEFORE" "$TEMP_ROOT/commit-after.json" || {
  echo "dakle_owner_hit=FAIL local complete commit changed UserDB logical entries" >&2
  exit 1
}
jq -e '
  .gesture == "owner-verified-dakle" and
  .commitText == "打开了" and
  (.selectionProbe.observed == false or .selectionProbe.reinforce == false) and
  .userDataSyncAttempted == false and
  .detail.markedTextBefore == "打开了" and
  .detail.persistentLearningAuthorized == false
' "$COMMIT_OUTPUT" >/dev/null

cp -R "$OWNER_SOURCE"/. "$TEMP_ROOT/owner-after/"
chmod -R u+w "$TEMP_ROOT/owner-after"
list_entries "$TEMP_ROOT/owner-after" "$TEMP_ROOT/owner-after.json"
OWNER_DIGEST_AFTER="$(entry_digest "$TEMP_ROOT/owner-after.json")"
OWNER_COUNT_AFTER="$(jq 'length' "$TEMP_ROOT/owner-after.json")"
[[ "$OWNER_DIGEST_BEFORE" == "$OWNER_DIGEST_AFTER" ]] || {
  echo "dakle_owner_hit=FAIL ownerEntriesUnchanged=false" >&2
  exit 3
}

echo "dakle_owner_hit=PASS owner_top1=打开了 marked_text=打开了 commit=打开了 userdb_diff=0 owner_entries=$OWNER_COUNT_BEFORE/$OWNER_COUNT_AFTER ownerEntriesUnchanged=true cases=10 sessions=50 corpus_checksum=$ACTUAL_CORPUS_SHA artifact_checksum=$ARTIFACT_SHA"
jq -c '.observations[] | {id, query, top5: .actual[0:5]}' "$REPORT"
