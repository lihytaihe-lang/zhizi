#!/usr/bin/env bash
set -euo pipefail

BUNDLE="${1:-}"
[[ -d "$BUNDLE" ]] || { echo "usage: $0 <input-method-bundle>" >&2; exit 2; }
[[ -z "$(find "$BUNDLE" -type l -print -quit)" ]] || { echo "bundle contains symlinks" >&2; exit 3; }

(
  cd "$BUNDLE"
  find . -type f -print0 | while IFS= read -r -d '' name; do
    case "$name" in
      *'\'*|*$'\r'*|*$'\n'*)
        echo "bundle contains a filename with backslash, newline, or carriage return" >&2
        exit 3
        ;;
    esac
  done
  find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256
) | shasum -a 256 | awk '{print $1}'
