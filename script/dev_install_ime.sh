#!/usr/bin/env bash
# Development install of the freshly built input method into ~/Library/Input Methods, the way it has been done by hand
# since 1493 (the formal script install_input_method.sh only accepts frozen release packages).
#
#   script/dev_install_ime.sh --yes        # only after the owner has said "装"
#
# Stop the old process by PID, move the old bundle into DevelopmentInstalls/ime-backup-<time>, ditto the build in,
# lsregister -f, then PROVE it took: select the input source and read it back. It never disables, unregisters or
# reinstalls the input source (2026-10-11: that made the menu entry vanish and only a reboot fixed it).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD="$ROOT_DIR/build/native/release/Agent Input Method.inputmethod"
TARGET="$HOME/Library/Input Methods/Agent Input Method.app"
SUPPORT="$HOME/Library/Application Support/AgentInputMethod"
SOURCE_ID="com.hongye.facai.inputmethod.zhizi.pinyin"
[[ "${1:-}" == "--yes" ]] || { echo "refusing: run with --yes after the owner said 装" >&2; exit 2; }
[[ -d "$BUILD" ]] || { echo "no build at $BUILD" >&2; exit 2; }

codesign --verify --deep --strict "$BUILD"
"$ROOT_DIR/script/check_core_network_boundary.sh" "$BUILD" | tail -n 1
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$BUILD/Contents/Info.plist")"
EVENTS="$SUPPORT/Quality/events.jsonl"
BEFORE_LINES="$(wc -l < "$EVENTS" 2>/dev/null || echo 0)"
echo "installing build $VERSION (recorder lines: $BEFORE_LINES)"

TIS="$ROOT_DIR/.build-deps/tis"
if [[ ! -x "$TIS" ]]; then
  mkdir -p "$ROOT_DIR/.build-deps"
  swiftc -O "$ROOT_DIR/scripts/tools/tis.swift" -o "$TIS"
fi

BACKUP="$SUPPORT/DevelopmentInstalls/ime-backup-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP"
OLD_PIDS="$(pgrep -f "$TARGET/Contents/MacOS/AgentInputMethod" || true)"
for pid in $OLD_PIDS; do kill "$pid"; done          # by PID, never a broad pkill
sleep 1
[[ -d "$TARGET" ]] && mv "$TARGET" "$BACKUP/Agent Input Method.app"
ditto "$BUILD" "$TARGET"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$TARGET"

[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$TARGET/Contents/Info.plist")" == "$VERSION" ]]
diff -rq "$BUILD" "$TARGET" && echo "installed bundle identical to the build"
codesign --verify --deep --strict "$TARGET"
# A new bundle identity (2026-10-13: com.hongye.facai.inputmethod.zhizi) is a new input source to the system: register and
# enable it once. This does not touch the old com.agentinputlayer source, which stays as a stale entry until the owner removes it.
"$TARGET/Contents/MacOS/AgentIMEInputSourceCtl" register || true
"$TARGET/Contents/MacOS/AgentIMEInputSourceCtl" enable || true
"$TARGET/Contents/MacOS/AgentIMEInputSourceCtl" status || true

# The only proof that counts: the system really switches to the input source and reads it back.
"$TIS" select "$SOURCE_ID"
sleep 2
CURRENT="$("$TIS")"
if [[ "$CURRENT" != "$SOURCE_ID" ]]; then
  echo "NOT SWITCHED: current input source is $CURRENT. If this is the first run under the new identity, add it once in System Settings > Keyboard > Input Sources > + > Chinese (Simplified) > 知字拼音; otherwise do not disable or reinstall; tell the owner." >&2
  exit 3
fi
NEW_PID="$(pgrep -f "$TARGET/Contents/MacOS/AgentInputMethod" | head -1 || true)"
echo "install OK: build $VERSION, input source $CURRENT, process ${NEW_PID:-none yet}, backup $BACKUP"
echo "recorder lines now: $(wc -l < "$EVENTS" 2>/dev/null || echo 0) (was $BEFORE_LINES); they must keep growing while the owner types"
