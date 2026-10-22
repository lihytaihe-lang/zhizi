#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMP_ROOT="$(mktemp -d /tmp/agent-ime-userdb-word-cleanup.XXXXXX)"
trap 'rm -rf "$TEMP_ROOT"' EXIT

"$ROOT_DIR/script/bootstrap_native.sh" >/dev/null
CARGO_NET_OFFLINE=true cargo build --locked --offline \
  --manifest-path "$ROOT_DIR/native/core/Cargo.toml" --release --features librime >/dev/null
touch "$ROOT_DIR/native/macos/Sources/AgentIMECoreBridge/bridge.c"
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEUserDBHarness >/dev/null
AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --product AgentIMEUserDBCtl >/dev/null
BIN="$(AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
  --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
  --show-bin-path)"
export DYLD_LIBRARY_PATH="$ROOT_DIR/.build-deps/librime/1.16.1/lib:$ROOT_DIR/native/core/target/release"

SHARED="$TEMP_ROOT/shared"
RUNTIME="$TEMP_ROOT/runtime"
USER="$RUNTIME/Rime"
mkdir -p "$SHARED" "$USER"
cp "$ROOT_DIR"/native/resources/Rime/* "$SHARED/"
cp "$ROOT_DIR/.build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml" "$SHARED/"

TRAIN="$TEMP_ROOT/train.json"
AFTER_DELETE="$TEMP_ROOT/after-delete.json"
AFTER_UNDO="$TEMP_ROOT/after-undo.json"
RELEARN="$TEMP_ROOT/relearn.json"
"$BIN/AgentIMEUserDBHarness" --word-cleanup-train "$SHARED" "$RUNTIME" "$TRAIN"
jq -e '.promotedCandidates[0] == "时" and .trainingSelections == 30' "$TRAIN" >/dev/null

BEFORE_LIST="$($BIN/AgentIMEUserDBCtl list "$SHARED" "$USER")"
TARGET="$(printf '%s' "$BEFORE_LIST" | jq -c '.items[] | select(.text == "时" and .pinyin == "shi")')"
[[ -n "$TARGET" ]] || { echo "trained target missing from UserDB list" >&2; exit 1; }
DELETE_REQUEST="$(printf '%s' "$TARGET" | jq -c '[{id:"word-cleanup-target",text,pinyin,entryLine:null}]')"
PARTIAL_REQUEST="$(printf '%s' "$TARGET" | jq -c '[{id:"word-cleanup-target",text,pinyin,entryLine:null},{id:"missing-target",text:"不存在的学习词",pinyin:"shi",entryLine:null}]')"
if printf '%s' "$PARTIAL_REQUEST" | "$BIN/AgentIMEUserDBCtl" delete "$SHARED" "$USER" >/dev/null 2>&1; then
  echo "partial multi-delete unexpectedly succeeded" >&2
  exit 1
fi
[[ "$($BIN/AgentIMEUserDBCtl list "$SHARED" "$USER" | jq '[.items[] | select(.text == "时" and .pinyin == "shi")] | length')" -eq 1 ]]
DELETED="$(printf '%s' "$DELETE_REQUEST" | $BIN/AgentIMEUserDBCtl delete "$SHARED" "$USER")"
[[ "$(printf '%s' "$DELETED" | jq '.items | length')" -eq 1 ]]
[[ "$($BIN/AgentIMEUserDBCtl list "$SHARED" "$USER" | jq '[.items[] | select(.text == "时" and .pinyin == "shi")] | length')" -eq 0 ]]

"$BIN/AgentIMEUserDBHarness" --word-cleanup-inspect "$SHARED" "$RUNTIME" "$AFTER_DELETE"
jq -e '.candidates[0] != "时"' "$AFTER_DELETE" >/dev/null
[[ "$(jq -cS '.controlNi' "$TRAIN")" == "$(jq -cS '.controlNi' "$AFTER_DELETE")" ]]
[[ "$(jq -cS '.controlSuoyi' "$TRAIN")" == "$(jq -cS '.controlSuoyi' "$AFTER_DELETE")" ]]

RESTORE_REQUEST="$(printf '%s' "$DELETED" | jq -c '[.items[0]]')"
printf '%s' "$RESTORE_REQUEST" | "$BIN/AgentIMEUserDBCtl" restore "$SHARED" "$USER" >/dev/null
"$BIN/AgentIMEUserDBHarness" --word-cleanup-inspect "$SHARED" "$RUNTIME" "$AFTER_UNDO"
jq -e '.candidates[0] == "时"' "$AFTER_UNDO" >/dev/null

RESTORED_LIST="$($BIN/AgentIMEUserDBCtl list "$SHARED" "$USER")"
RESTORED_TARGET="$(printf '%s' "$RESTORED_LIST" | jq -c '.items[] | select(.text == "时" and .pinyin == "shi")')"
DELETE_AGAIN="$(printf '%s' "$RESTORED_TARGET" | jq -c '[{id:"word-cleanup-target",text,pinyin,entryLine:null}]')"
printf '%s' "$DELETE_AGAIN" | "$BIN/AgentIMEUserDBCtl" delete "$SHARED" "$USER" >/dev/null
"$BIN/AgentIMEUserDBHarness" --word-cleanup-select "$SHARED" "$RUNTIME" "$RELEARN" 9000000000
jq -e '.commitText == "时" and .selectionEventID != "0"' "$RELEARN" >/dev/null
[[ "$($BIN/AgentIMEUserDBCtl list "$SHARED" "$USER" | jq '[.items[] | select(.text == "时" and .pinyin == "shi")] | length')" -eq 1 ]]

# A learned reading that cannot be typed back into a candidate (`shen m` splits
# as an abbreviation) is still deletable and undoable.
UNTYPEABLE="$(printf '%s' "$DELETED" | jq -c '[.items[0] | {id:"untypeable",text:"神呒",pinyin:"shen m",entryLine:(.entryLine | sub("shi[ ]*\t时"; "shen m \t神呒"))}]')"
printf '%s' "$UNTYPEABLE" | "$BIN/AgentIMEUserDBCtl" restore "$SHARED" "$USER" >/dev/null
[[ "$($BIN/AgentIMEUserDBCtl list "$SHARED" "$USER" | jq '[.items[] | select(.text == "神呒" and .pinyin == "shen m")] | length')" -eq 1 ]]
UNTYPEABLE_DELETED="$(printf '%s' "$UNTYPEABLE" | jq -c '[.[0] | .entryLine = null]' | "$BIN/AgentIMEUserDBCtl" delete "$SHARED" "$USER")"
[[ "$($BIN/AgentIMEUserDBCtl list "$SHARED" "$USER" | jq '[.items[] | select(.text == "神呒")] | length')" -eq 0 ]]
printf '%s' "$UNTYPEABLE_DELETED" | jq -c '.items' | "$BIN/AgentIMEUserDBCtl" restore "$SHARED" "$USER" >/dev/null
[[ "$($BIN/AgentIMEUserDBCtl list "$SHARED" "$USER" | jq '[.items[] | select(.text == "神呒" and .pinyin == "shen m")] | length')" -eq 1 ]]

printf 'userdb_word_cleanup=PASS real_librime=1 target=时 query=shi training=30 partial_batch_rollback=1 delete_top1_cleared=1 hr6_controls_unchanged=2 undo_top1_restored=1 relearn_allowed=1 untypeable_delete_undo=1\n'
printf 'promoted_candidates=%s\n' "$(jq -c '.promotedCandidates' "$TRAIN")"
printf 'after_delete_candidates=%s\n' "$(jq -c '.candidates' "$AFTER_DELETE")"
printf 'after_undo_candidates=%s\n' "$(jq -c '.candidates' "$AFTER_UNDO")"
printf 'control_ni=%s\n' "$(jq -c '.controlNi' "$AFTER_DELETE")"
printf 'control_suoyi=%s\n' "$(jq -c '.controlSuoyi' "$AFTER_DELETE")"
