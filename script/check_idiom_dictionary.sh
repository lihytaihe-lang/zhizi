#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PYTHON="${PYPINYIN_PYTHON:-$ROOT_DIR/.build-deps/pypinyin/0.55.0/venv/bin/python}"
[[ -x "$PYTHON" ]] || {
  echo "missing pinned pypinyin 0.55.0 build tool: $PYTHON" >&2
  exit 2
}
"$PYTHON" "$ROOT_DIR/scripts/generate_idiom_dictionary.py" --check
"$PYTHON" "$ROOT_DIR/scripts/test_generate_dictionary.py"
