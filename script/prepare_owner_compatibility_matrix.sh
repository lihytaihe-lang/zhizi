#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT="${1:-$ROOT_DIR/docs/current/evidence/v0.8/CHUNK8/artifacts/owner-compatibility-environment.md}"
mkdir -p "$(dirname "$OUTPUT")"

version_of() {
  local path="$1"
  if [[ -d "$path" && ! -L "$path" ]]; then
    /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$path/Contents/Info.plist" 2>/dev/null || echo unknown
  else
    echo not-installed
  fi
}

{
  echo '# Owner compatibility environment (preparation only)'
  echo
  echo "Generated: $(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo
  echo '| App | Detected version | Human scenarios | Result |'
  echo '|---|---:|---|---|'
  echo "| TextEdit | $(version_of /System/Applications/TextEdit.app) | marked text, caret, first-row digits 1-5, second-row digits 6-0, shortcut, secure transition | PENDING_OWNER |"
  echo "| Safari | $(version_of /Applications/Safari.app) | contenteditable, address bar, password field, candidate position | PENDING_OWNER |"
  echo "| Google Chrome | $(version_of '/Applications/Google Chrome.app') | textarea, contenteditable, password field, shortcut | PENDING_OWNER |"
  echo "| Visual Studio Code | $(version_of '/Applications/Visual Studio Code.app') | editor marked text, terminal secure boundary, candidate position | PENDING_OWNER |"
  echo "| Xcode | $(version_of /Applications/Xcode.app) | source editor marked text, completion coexistence, candidate position | PENDING_OWNER |"
  echo "| Terminal | $(version_of /System/Applications/Utilities/Terminal.app) | shell marked text, secure input fail-closed, shortcut | PENDING_OWNER |"
  echo
  echo 'Codex prepared versions and scenarios only. No human result or P0 verdict is claimed.'
} > "$OUTPUT"

echo "compatibility_matrix_prepared=$OUTPUT human_results=0 status=PENDING_OWNER"
