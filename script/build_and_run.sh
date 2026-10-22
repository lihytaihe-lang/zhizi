#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-run}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="agent-input-layer"
APP_BUNDLE="$ROOT_DIR/src-tauri/target/debug/bundle/macos/Agent Input Method.app"

pkill -x "$APP_NAME" >/dev/null 2>&1 || true
cd "$ROOT_DIR"
./script/build_native.sh debug
npm run tauri build -- --debug --bundles app

open_app() {
  /usr/bin/open -n "$APP_BUNDLE"
}

case "$MODE" in
  run)
    open_app
    ;;
  --debug|debug)
    lldb -- "$APP_BUNDLE/Contents/MacOS/$APP_NAME"
    ;;
  --logs|logs)
    open_app
    /usr/bin/log stream --info --style compact --predicate "process == \"$APP_NAME\""
    ;;
  --telemetry|telemetry)
    open_app
    /usr/bin/log stream --info --style compact --predicate 'subsystem == "com.agentinputlayer.desktop" OR subsystem == "com.agentinputlayer.inputmethod"'
    ;;
  --verify|verify)
    open_app
    for _ in 1 2 3 4 5; do
      pgrep -x "$APP_NAME" >/dev/null && exit 0
      sleep 1
    done
    echo "Agent Input Method manager process did not start" >&2
    exit 1
    ;;
  *)
    echo "usage: $0 [run|--debug|--logs|--telemetry|--verify]" >&2
    exit 2
    ;;
esac
