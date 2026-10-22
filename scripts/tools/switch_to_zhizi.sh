#!/usr/bin/env bash
# Switch the system input source to 顺手·知字 and read it back. A stop-gap for the case where the input menu stops
# listing 知字 although the system still has it enabled and selectable (seen 2026-10-11 and 2026-10-13).
# Never disables, unregisters or reinstalls anything.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TIS="$ROOT_DIR/.build-deps/tis"
[[ -x "$TIS" ]] || { mkdir -p "$ROOT_DIR/.build-deps"; swiftc -O "$ROOT_DIR/scripts/tools/tis.swift" -o "$TIS"; }
ID=com.hongye.facai.inputmethod.zhizi.pinyin
"$TIS" select "$ID"
sleep 1
CURRENT="$("$TIS")"
echo "current input source: $CURRENT"
[[ "$CURRENT" == "$ID" ]]
