#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHECKSUM="$ROOT_DIR/script/artifact_checksum.sh"
REAL_BUNDLE="${1:-$ROOT_DIR/build/native/release/Agent Input Method.inputmethod}"
EMPTY_SHA="e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

FIXROOT="$ROOT_DIR/tmp/phase01-20261018/a5fx"
mkdir -p "$FIXROOT"
WORK="$(mktemp -d "$FIXROOT/a5.XXXXXX")"

PASS=0
FAIL=0

cleanup() {
  chmod -R u+rwX "$WORK" 2>/dev/null || true
  rm -rf "$WORK"
  rmdir "$FIXROOT" 2>/dev/null || true
}
trap cleanup EXIT

pass() { PASS=$((PASS + 1)); echo "PASS: $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL: $1" >&2; }

# 旧实现逐字拷贝（本任务修改前的 script/artifact_checksum.sh），仅作对照
checksum_legacy() {
  local BUNDLE="$1"
  (
    cd "$BUNDLE"
    find . -type f -print | LC_ALL=C sort | while IFS= read -r file; do
      hash="$(shasum -a 256 "$file" | awk '{print $1}')"
      printf '%s  %s\n' "$hash" "$file"
    done
  ) | shasum -a 256 | awk '{print $1}'
}

assert_same() {
  local label="$1" dir="$2" expected="${3:-}"
  local legacy_status=0 legacy_out="" new_status=0 new_out=""
  legacy_out="$(checksum_legacy "$dir")" || legacy_status=$?
  new_out="$("$CHECKSUM" "$dir" 2>/dev/null)" || new_status=$?
  if [[ "$legacy_status" -ne 0 || "$new_status" -ne 0 ]]; then
    fail "$label: nonzero exit (legacy=$legacy_status new=$new_status)"
    return
  fi
  if [[ "$legacy_out" != "$new_out" ]]; then
    fail "$label: legacy [$legacy_out] != new [$new_out]"
    return
  fi
  if [[ -n "$expected" && "$new_out" != "$expected" ]]; then
    fail "$label: expected [$expected] got [$new_out]"
    return
  fi
  pass "$label: $new_out"
}

assert_exit() {
  local label="$1" want="$2" dir="$3" st=0
  "$CHECKSUM" "$dir" >/dev/null 2>/dev/null || st=$?
  if [[ "$st" -eq "$want" ]]; then
    pass "$label: exit $st"
  else
    fail "$label: want exit $want, got $st"
  fi
}

[[ -d "$REAL_BUNDLE" ]] || { echo "missing real bundle: $REAL_BUNDLE" >&2; exit 2; }

D="$WORK/spaces"
mkdir -p "$D"
printf 'content-a' > "$D/has space.txt"
printf 'content-b123' > "$D/double  space.bin"
assert_same "spaces filenames" "$D"

D="$WORK/colon"
mkdir -p "$D"
printf 'colon content' > "$D/name: with colon.txt"
assert_same "colon+space filenames" "$D"

D="$WORK/unicode"
mkdir -p "$D"
printf '中文内容' > "$D/中文 文件名.txt"
printf 'emoji' > "$D/😀输入法😀.plist"
printf 'accents' > "$D/café-niño.txt"
assert_same "unicode filenames" "$D"

D="$WORK/nested"
mkdir -p "$D/a/b/c/d"
printf 'root' > "$D/Contents Info.plist"
printf 'deep' > "$D/a/b/c/d/deep-file.txt"
printf 'mid' > "$D/a/b/mid.dat"
printf 'hidden' > "$D/.hidden"
assert_same "nested directories" "$D"

D="$WORK/empty"
mkdir -p "$D"
assert_same "empty bundle" "$D" "$EMPTY_SHA"

D="$WORK/single"
mkdir -p "$D"
printf 'only file' > "$D/Info.plist"
assert_same "single file" "$D"

# Bundle checksum varies with the Info.plist build number, so do not pin a
# fixed value here; only require the new implementation to match the legacy
# reference byte-for-byte on the real bundle.
assert_same "real release bundle" "$REAL_BUNDLE"

D="$WORK/backslash"
mkdir -p "$D"
printf 'x' > "$D/back\\slash.txt"
assert_exit "backslash filename rejected" 3 "$D"

D="$WORK/newline"
mkdir -p "$D"
printf 'x' > "$D/new"$'\n'"line.txt"
assert_exit "newline filename rejected" 3 "$D"

D="$WORK/carriage-return"
mkdir -p "$D"
printf 'x' > "$D/carriage"$'\r'"return.txt"
assert_exit "carriage-return filename rejected" 3 "$D"

D="$WORK/symlink"
mkdir -p "$D"
printf 'x' > "$D/real.txt"
ln -s real.txt "$D/link.txt"
assert_exit "symlink rejected" 3 "$D"

assert_exit "missing directory" 2 "$WORK/does-not-exist"

D="$WORK/unreadable"
mkdir -p "$D"
printf 'readable' > "$D/ok.txt"
printf 'locked' > "$D/locked.txt"
chmod 000 "$D/locked.txt"
st=0
"$CHECKSUM" "$D" >/dev/null 2>&1 || st=$?
chmod 600 "$D/locked.txt"
if [[ "$st" -ne 0 ]]; then
  pass "unreadable file fails loudly: exit $st"
else
  fail "unreadable file did not fail"
fi

echo "$PASS passed, $FAIL failed"
[[ "$FAIL" -eq 0 ]]
