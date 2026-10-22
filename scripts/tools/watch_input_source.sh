#!/usr/bin/env bash
# Read-only timeline of the system input-source state, to catch what changes when the menu stops listing 知字
# (2026-10-11, 2026-10-13). Every 2 s it appends one line when anything below changes: the current source, whether the
# two sources are enabled / selected per TIS, the PIDs of the processes that own the menu, and a checksum of the two
# preference files. Changes nothing. Stop it with `kill <pid>` (the pid is printed).
set -uo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EVAL_OUT="${ZHIZI_EVAL_OUT:-$ROOT_DIR/eval-runs}"
OUT="${1:-$EVAL_OUT/inputsource-watch.log}"
TIS="$ROOT_DIR/.build-deps/tis"
[[ -x "$TIS" ]] || { mkdir -p "$ROOT_DIR/.build-deps"; swiftc -O "$ROOT_DIR/scripts/tools/tis.swift" -o "$TIS"; }
LIST="$ROOT_DIR/.build-deps/tislist"
[[ -x "$LIST" ]] || swiftc -O "$ROOT_DIR/scripts/tools/tislist.swift" -o "$LIST"
echo "watching, pid $$, writing to $OUT" >&2
last=""
while true; do
  state="current=$("$TIS") | $("$LIST" | tr '\n' ';') | pids=$(pgrep -x TextInputSwitcher | tr '\n' ','),$(pgrep -x TextInputMenuAgent | tr '\n' ','),$(pgrep -x imklaunchagent | tr '\n' ','),$(pgrep -f 'Input Methods/Agent Input Method.app' | tr '\n' ','),$(pgrep -x SogouInput | tr '\n' ',') | prefs=$(cat ~/Library/Preferences/com.apple.HIToolbox.plist ~/Library/Preferences/com.apple.inputsources.plist 2>/dev/null | shasum | cut -c1-8)"
  if [[ "$state" != "$last" ]]; then echo "$(date '+%F %T') $state" >> "$OUT"; last="$state"; fi
  sleep 2
done
