#!/usr/bin/env bash
# One release version (VERSION at the repo root) for everything a user or a gate sees: the Mac input method's Info.plist, the management
# app (package.json, package-lock.json, tauri.conf.json, src-tauri/Cargo.toml and Cargo.lock) and the Swift constant the test harnesses report.
#   script/sync_version.sh            write VERSION into those files
#   script/sync_version.sh --check    exit 1 and list every file that differs (used by the build scripts); writes nothing
# Internal library crates (native/core, native/ipc, native/host, Windows crates) keep their own versions on purpose.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[[ $# -eq 0 || "$1" == "--check" ]] || { echo "usage: $0 [--check]" >&2; exit 2; }
exec python3 "$ROOT_DIR/scripts/tools/sync_version.py" --root "$ROOT_DIR" "$@"
