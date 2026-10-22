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
  mkdir -p "$primary/Rime/pinyin_simp.userdb" "$fallback"
  chmod 700 "$primary" "$primary/Rime" "$primary/Rime/pinyin_simp.userdb" "$fallback"

  if [[ "$name" == "symlink" ]]; then
    local external="$TEMP_ROOT/symlink-external.userdb"
    rm -rf "$primary/Rime/pinyin_simp.userdb"
    mkdir -p "$external"
    printf 'MANIFEST-000001\n' > "$external/CURRENT"
    printf 'fixture' > "$external/MANIFEST-000001"
    printf 'external-symlink-sentinel' > "$external/TEST_MARKER"
    chmod 640 "$external/TEST_MARKER"
    ln -s "$external" "$primary/Rime/pinyin_simp.userdb"
    local before_mode before_links
    before_mode="$(stat -f '%Lp' "$external/TEST_MARKER")"
    before_links="$(stat -f '%l' "$external/TEST_MARKER")"
    env DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$HARNESS" \
      "$BUNDLE/Contents/Resources/Rime" "$primary" ni \
      --storage-fallback "$fallback" > "$log"
    [[ "$(<"$external/TEST_MARKER")" == "external-symlink-sentinel" ]]
    [[ "$(stat -f '%Lp' "$external/TEST_MARKER")" == "$before_mode" ]]
    [[ "$(stat -f '%l' "$external/TEST_MARKER")" == "$before_links" ]]
  else
    local external="$TEMP_ROOT/hardlink-external-CURRENT"
    printf 'MANIFEST-000001\n' > "$external"
    ln "$external" "$primary/Rime/pinyin_simp.userdb/CURRENT"
    printf 'fixture' > "$primary/Rime/pinyin_simp.userdb/MANIFEST-000001"
    chmod 640 "$external"
    local before_mode before_links
    before_mode="$(stat -f '%Lp' "$external")"
    before_links="$(stat -f '%l' "$external")"
    env DYLD_LIBRARY_PATH="$BUNDLE/Contents/Frameworks" "$HARNESS" \
      "$BUNDLE/Contents/Resources/Rime" "$primary" ni \
      --storage-fallback "$fallback" > "$log"
    [[ "$(<"$external")" == "MANIFEST-000001" ]]
    [[ "$(stat -f '%Lp' "$external")" == "$before_mode" ]]
    [[ "$(stat -f '%l' "$external")" == "$before_links" ]]
  fi

  grep -qx 'storage_degraded=true' "$log"
  grep -q '^marked=ni$' "$log"
  grep -q '^candidates=.*你' "$log"
  [[ -z "$(find "$fallback" -mindepth 1 -print -quit)" ]]
}

run_case symlink
run_case hardlink
echo "runtime_storage_fallback=PASS cases=2 candidates=available external_sentinels=unchanged"
