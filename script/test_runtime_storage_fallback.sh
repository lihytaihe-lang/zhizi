#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
HARNESS="${2:-}"
[[ -d "$BUNDLE" && ! -L "$BUNDLE" ]] || { echo "missing input method bundle" >&2; exit 2; }
if [[ -z "$HARNESS" ]]; then
  BIN_DIR="$(AIM_CORE_LIBRARY_DIR="$ROOT_DIR/native/core/target/release" swift build \
    --package-path "$ROOT_DIR/native/macos" -c debug --disable-automatic-resolution \
    --show-bin-path)"
  HARNESS="$BIN_DIR/AgentIMECoreHarness"
fi
[[ -x "$HARNESS" ]] || { echo "missing core harness" >&2; exit 2; }

TEMP_ROOT="$(mktemp -d /tmp/agent-ime-storage-fallback-test.XXXXXX)"
trap 'chmod -R u+rwX "$TEMP_ROOT" 2>/dev/null || true; rm -rf "$TEMP_ROOT"' EXIT
echo "temporary_cleanup_scope=$TEMP_ROOT"

run_case() {
  local name="$1"
  local primary="$TEMP_ROOT/$name-primary"
  local fallback="$TEMP_ROOT/$name-fallback"
  local log="$TEMP_ROOT/$name.log"
  mkdir -p "$primary" "$fallback"
  chmod 700 "$primary" "$fallback"

  if [[ "$name" == "symlink" ]]; then
    # The user directory itself is a symlink escaping the tree: the lease must
    # reject the primary without following or mutating the external tree.
    local external="$TEMP_ROOT/symlink-external-user-tree"
    mkdir -p "$external"
    printf 'external-symlink-sentinel' > "$external/TEST_MARKER"
    chmod 640 "$external/TEST_MARKER"
    ln -s "$external" "$primary/Rime"
    local before_mode before_links
    before_mode="$(stat -f '%Lp' "$external/TEST_MARKER")"
    before_links="$(stat -f '%l' "$external/TEST_MARKER")"
    "$HARNESS" \
      "$BUNDLE/Contents/Resources/Rime" "$primary" ni \
      --storage-fallback "$fallback" > "$log"
    [[ "$(<"$external/TEST_MARKER")" == "external-symlink-sentinel" ]]
    [[ "$(stat -f '%Lp' "$external/TEST_MARKER")" == "$before_mode" ]]
    [[ "$(stat -f '%l' "$external/TEST_MARKER")" == "$before_links" ]]
  else
    # A non-directory occupies the user-directory name.
    printf 'not-a-directory' > "$primary/Rime"
    chmod 640 "$primary/Rime"
    local before_mode before_links
    before_mode="$(stat -f '%Lp' "$primary/Rime")"
    before_links="$(stat -f '%l' "$primary/Rime")"
    "$HARNESS" \
      "$BUNDLE/Contents/Resources/Rime" "$primary" ni \
      --storage-fallback "$fallback" > "$log"
    [[ "$(<"$primary/Rime")" == "not-a-directory" ]]
    [[ "$(stat -f '%Lp' "$primary/Rime")" == "$before_mode" ]]
    [[ "$(stat -f '%l' "$primary/Rime")" == "$before_links" ]]
  fi

  grep -qx 'storage_degraded=true' "$log"
  grep -q '^marked=ni$' "$log"
  grep -q '^candidates=.*你' "$log"
  [[ -z "$(find "$fallback" -mindepth 1 -print -quit)" ]]
}

run_case symlink
run_case regular-file
echo "runtime_storage_fallback=PASS cases=2 candidates=available external_sentinels=unchanged"
