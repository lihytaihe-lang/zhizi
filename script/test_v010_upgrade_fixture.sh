#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_DIR="$ROOT_DIR/src-tauri/fixtures/v0.1.0"
FIXTURE_B64="$FIXTURE_DIR/agent-input-layer.sqlite3.b64"
PROVENANCE="$FIXTURE_DIR/FIXTURE_PROVENANCE.json"

for command in git jq base64 shasum sqlite3; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "required command missing: $command" >&2
    exit 2
  }
done

[[ -f "$FIXTURE_B64" && -f "$PROVENANCE" ]] || {
  echo "v0.1.0 upgrade fixture inventory is incomplete" >&2
  exit 3
}

TEMP_ROOT="$(mktemp -d /private/tmp/agent-ime-v010-upgrade.XXXXXX)"
cleanup() {
  rm -rf "$TEMP_ROOT"
}
trap cleanup EXIT

DECODED="$TEMP_ROOT/agent-input-layer.sqlite3"
/usr/bin/base64 -D -i "$FIXTURE_B64" -o "$DECODED"

expected_bytes="$(jq -er '.artifact.decodedBytes' "$PROVENANCE")"
expected_sha256="$(jq -er '.artifact.decodedSHA256' "$PROVENANCE")"
actual_bytes="$(wc -c < "$DECODED" | tr -d ' ')"
actual_sha256="$(shasum -a 256 "$DECODED" | awk '{print $1}')"
[[ "$actual_bytes" == "$expected_bytes" ]] || {
  echo "v0.1.0 fixture byte count mismatch" >&2
  exit 4
}
[[ "$actual_sha256" == "$expected_sha256" ]] || {
  echo "v0.1.0 fixture checksum mismatch" >&2
  exit 4
}

generator_commit="$(jq -er '.generator.repositoryCommit' "$PROVENANCE")"
generator_version="$(jq -er '.generator.packageVersion' "$PROVENANCE")"
git -C "$ROOT_DIR" cat-file -e "$generator_commit^{commit}"
actual_version="$(git -C "$ROOT_DIR" show "$generator_commit:package.json" | jq -er '.version')"
[[ "$actual_version" == "$generator_version" && "$generator_version" == "0.1.0" ]] || {
  echo "v0.1.0 fixture generator version mismatch" >&2
  exit 5
}

for kind in storage migration cargoManifest cargoLock; do
  source_path="$(jq -er ".generator.${kind}SourcePath" "$PROVENANCE")"
  expected_source_sha="$(jq -er ".generator.${kind}SourceSHA256" "$PROVENANCE")"
  actual_source_sha="$(git -C "$ROOT_DIR" show "$generator_commit:$source_path" | shasum -a 256 | awk '{print $1}')"
  [[ "$actual_source_sha" == "$expected_source_sha" ]] || {
    echo "v0.1.0 $kind source provenance mismatch" >&2
    exit 5
  }
done

[[ "$(sqlite3 "$DECODED" 'PRAGMA integrity_check;')" == "ok" ]] || {
  echo "v0.1.0 fixture integrity check failed" >&2
  exit 6
}
[[ "$(sqlite3 "$DECODED" 'SELECT COUNT(*) FROM history;')" == "2" ]] || exit 6
[[ "$(sqlite3 "$DECODED" 'SELECT COUNT(*) FROM user_rules;')" == "2" ]] || exit 6
[[ "$(sqlite3 "$DECODED" 'SELECT COUNT(*) FROM settings;')" == "4" ]] || exit 6
expected_setting_keys=$'providerSettings\nselectedModeId\nshortcutSettings\nstorageInitialized'
actual_setting_keys="$(sqlite3 "$DECODED" 'SELECT key FROM settings ORDER BY key;')"
[[ "$actual_setting_keys" == "$expected_setting_keys" ]] || {
  echo "v0.1.0 fixture setting inventory mismatch" >&2
  exit 6
}

printf 'v010_upgrade_fixture=PASS commit=%s bytes=%s sha256=%s history=2 rules=2 settings=4 privacy=synthetic-only\n' \
  "$generator_commit" "$actual_bytes" "$actual_sha256"
