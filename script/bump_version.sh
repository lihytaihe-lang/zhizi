#!/usr/bin/env bash
# Next release version (VERSION at the repo root): one numbering for every build handed to anyone - the owner's own install and the
# friends' packages alike (owner 2026-10-16).  Run it before the build you are about to install or package; commit; then build.
#   script/bump_version.sh          0.1.1 -> 0.1.2
#   script/bump_version.sh minor    0.1.7 -> 0.2.0
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
current="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
[[ "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || { echo "bad VERSION: $current" >&2; exit 2; }
major=${BASH_REMATCH[1]}; minor=${BASH_REMATCH[2]}; patch=${BASH_REMATCH[3]}
if [[ "${1:-patch}" == "minor" ]]; then minor=$((minor + 1)); patch=0; else patch=$((patch + 1)); fi
next="$major.$minor.$patch"
printf '%s\n' "$next" > "$ROOT_DIR/VERSION"
"$ROOT_DIR/script/sync_version.sh" >/dev/null
echo "VERSION $current -> $next (add a CHANGELOG.md entry for $next, then commit before building)"
