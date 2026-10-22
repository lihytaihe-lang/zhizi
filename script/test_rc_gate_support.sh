#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
SUPPORT_TOOL="$ROOT_DIR/script/rc_gate_support.sh"
TEST_ROOT="$(/usr/bin/mktemp -d /tmp/agent-ime-rc-gate-support.XXXXXX)"
TEST_ROOT_INODE="$(/usr/bin/stat -f %i "$TEST_ROOT")"

cleanup() {
  local status="$?"
  trap - EXIT HUP INT TERM
  if [[ "$TEST_ROOT" == /tmp/agent-ime-rc-gate-support.* \
    && -d "$TEST_ROOT" && ! -L "$TEST_ROOT" \
    && "$(/usr/bin/stat -f %i "$TEST_ROOT" 2>/dev/null || true)" == "$TEST_ROOT_INODE" ]]; then
    /bin/chmod -R u+rwX "$TEST_ROOT" 2>/dev/null || true
    /bin/rm -rf "$TEST_ROOT"
  fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

fail() {
  echo "RC gate support test failed: $*" >&2
  exit 1
}

commit_fixture() {
  git -C "$TEST_ROOT" -c user.name='Agent IME Test' -c user.email='agent-ime-test@invalid' \
    -c commit.gpgsign=false commit -q -m "$1"
}

assert_verifies() {
  local message="$1"
  [[ "$("$SUPPORT_TOOL" verify-inputs "$TEST_ROOT" "$PRODUCT_COMMIT")" == "$PRODUCT_COMMIT" ]] \
    || fail "$message"
}

assert_rejected() {
  local expected_message="$1"
  local label="$2"
  if "$SUPPORT_TOOL" verify-inputs "$TEST_ROOT" "$PRODUCT_COMMIT" \
    > "$TEST_ROOT/$label.stdout" 2> "$TEST_ROOT/$label.stderr"; then
    fail "$label unexpectedly verified"
  fi
  /usr/bin/grep -Fq "$expected_message" "$TEST_ROOT/$label.stderr" \
    || fail "$label failure was not explicit"
  if [[ "$expected_message" == "tracked RC input differs from ProductCommit" ]]; then
    /usr/bin/grep -Fq "frozen_gates=SKIPPED reason=dirty-tree" "$TEST_ROOT/$label.stderr" \
      || fail "$label did not make the downstream frozen-gate skip visible"
  fi
}

write_adr() {
  local path="$1"
  local number="$2"
  printf '# ADR-%s fixture\n\n- Status: `provisional`\n- Date: 2026-07-13\n- Owner: fixture\n\n## Context\n\nlocked context %s\n\n## Decision\n\nlocked decision %s\n\n## Verification Evidence\n\ninitial evidence %s\n\n## Revisit Triggers\n\nlocked trigger %s\n' \
    "$number" "$number" "$number" "$number" "$number" > "$path"
}

update_allowed_adr_fields() {
  local path="$1"
  /usr/bin/awk '
    /^- Status:/ { print "- Status: `accepted-joint-batch`"; next }
    /^## Verification Evidence$/ {
      print
      print ""
      print "evidence descendant result"
      in_verification = 1
      next
    }
    in_verification && /^## / { in_verification = 0 }
    in_verification { next }
    { print }
  ' "$path" > "$path.tmp"
  /bin/mv "$path.tmp" "$path"
}

restore_from_head() {
  local path="$1"
  git -C "$TEST_ROOT" show "HEAD:$path" > "$TEST_ROOT/$path"
}

git -C "$TEST_ROOT" init -q
/bin/mkdir -p \
  "$TEST_ROOT/src" \
  "$TEST_ROOT/third_party/licenses" \
  "$TEST_ROOT/docs/current/adr" \
  "$TEST_ROOT/docs/current/security" \
  "$TEST_ROOT/docs/current/evidence/v0.2-v0.3"
for index in $(/usr/bin/jot 25 1); do
  printf 'fixture-%02d\n' "$index" > "$TEST_ROOT/src/input-$index.txt"
done
printf 'frozen acceptance contract\n' > "$TEST_ROOT/docs/current/PRODUCT_SPEC.md"
printf 'locked secret inventory\n' > "$TEST_ROOT/docs/current/security/PLAINTEXT_SECRET_INVENTORY.md"
printf 'locked privacy policy\n' > "$TEST_ROOT/docs/privacy.md"
printf 'locked dogfood runbook\n' > "$TEST_ROOT/docs/current/evidence/v0.2-v0.3/DOGFOOD_RUNBOOK.md"
printf '{"formatVersion":1}\n' \
  > "$TEST_ROOT/docs/current/evidence/v0.2-v0.3/DOGFOOD_ATTESTATION.template.json"

NATIVE_LICENSE_PATHS=(
  third_party/licenses/boost-BSL-1.0.txt
  third_party/licenses/glog-BSD-3-Clause.txt
  third_party/licenses/leveldb-BSD-3-Clause.txt
  third_party/licenses/marisa-trie-COPYING.txt
  third_party/licenses/OpenCC-Apache-2.0.txt
  third_party/licenses/darts-clone-COPYING.txt
  third_party/licenses/RapidJSON-LICENSE.txt
  third_party/licenses/yaml-cpp-MIT.txt
)
for path in "${NATIVE_LICENSE_PATHS[@]}"; do
  printf 'locked native license fixture: %s\n' "$(basename "$path")" > "$TEST_ROOT/$path"
done

ADR_PATHS=(
  docs/current/adr/ADR-001-rime-assets.md
  docs/current/adr/ADR-002-native-build-organization.md
  docs/current/adr/ADR-003-shared-snapshot.md
  docs/current/adr/ADR-004-ipc.md
  docs/current/adr/ADR-005-install-signing.md
  docs/current/adr/ADR-006-secure-input.md
  docs/current/adr/ADR-007-userdb.md
  docs/current/adr/ADR-008-candidate-mixer-ranking.md
  docs/current/adr/ADR-009-context-routing.md
)
for index in $(/usr/bin/jot 9 1); do
  number="$(printf '%03d' "$index")"
  write_adr "$TEST_ROOT/${ADR_PATHS[$((index - 1))]}" "$number"
done

git -C "$TEST_ROOT" add src docs third_party/licenses
commit_fixture 'product inputs'
PRODUCT_COMMIT="$(git -C "$TEST_ROOT" rev-parse HEAD)"
PRODUCT_FINGERPRINT="$("$SUPPORT_TOOL" fingerprint "$TEST_ROOT" "$PRODUCT_COMMIT")"
[[ "$PRODUCT_FINGERPRINT" =~ ^[0-9a-f]{64}$ ]] || fail "ProductCommit fingerprint is invalid"
assert_verifies "the ProductCommit did not verify at its own HEAD"

# Native license texts are shipped ProductInputs, not mutable evidence. A
# committed descendant change must alter the fingerprint and be rejected when
# verifying against ProductCommit.
printf 'forbidden post-ProductCommit mutation\n' >> "$TEST_ROOT/${NATIVE_LICENSE_PATHS[0]}"
git -C "$TEST_ROOT" add "${NATIVE_LICENSE_PATHS[0]}"
commit_fixture 'changed locked native license'
assert_rejected 'tracked RC input differs from ProductCommit' 'changed-native-license'
[[ "$("$SUPPORT_TOOL" fingerprint "$TEST_ROOT" HEAD)" != "$PRODUCT_FINGERPRINT" ]] \
  || fail "a changed native license did not alter the ProductInputs fingerprint"
git -C "$TEST_ROOT" show "$PRODUCT_COMMIT:${NATIVE_LICENSE_PATHS[0]}" \
  > "$TEST_ROOT/${NATIVE_LICENSE_PATHS[0]}"
git -C "$TEST_ROOT" add "${NATIVE_LICENSE_PATHS[0]}"
commit_fixture 'restore locked native license'
assert_verifies "restoring the exact native license did not restore verification"

# The directory is scanned without ignore rules so an extra, untracked license
# cannot enter a build after ProductCommit.
printf 'untracked license payload\n' \
  > "$TEST_ROOT/third_party/licenses/untracked-native-license.txt"
assert_rejected 'untracked RC build/gate input is forbidden' 'untracked-native-license'
/bin/rm "$TEST_ROOT/third_party/licenses/untracked-native-license.txt"
assert_verifies "removing the untracked native license did not restore verification"

# EvidenceCommit may add result carriers and update only the normalized ADR
# status/evidence fields. Those changes must retain the ProductInputs digest.
printf 'evidence-only descendant\n' \
  > "$TEST_ROOT/docs/current/evidence/v0.2-v0.3/JOINT_EVIDENCE.md"
printf 'mutable APP results\n' \
  > "$TEST_ROOT/docs/current/evidence/v0.2-v0.3/APP_MATRIX.md"
printf 'mutable fault results\n' \
  > "$TEST_ROOT/docs/current/evidence/v0.2-v0.3/FAULT_MATRIX.md"
for path in "${ADR_PATHS[@]}"; do
  update_allowed_adr_fields "$TEST_ROOT/$path"
done
git -C "$TEST_ROOT" add docs/current/evidence docs/current/adr
commit_fixture 'evidence only'
assert_verifies "an evidence-only descendant must preserve ProductCommit verification"
EVIDENCE_COMMIT="$(git -C "$TEST_ROOT" rev-parse HEAD)"
[[ "$("$SUPPORT_TOOL" fingerprint "$TEST_ROOT" "$EVIDENCE_COMMIT")" == "$PRODUCT_FINGERPRINT" ]] \
  || fail "allowed EvidenceCommit ADR changes altered the normalized ProductInputs fingerprint"

# Any decision change is forbidden even when committed in a descendant.
/usr/bin/sed -i '' 's/locked decision 001/weakened decision 001/' \
  "$TEST_ROOT/${ADR_PATHS[0]}"
git -C "$TEST_ROOT" add "${ADR_PATHS[0]}"
commit_fixture 'forbidden ADR decision change'
assert_rejected 'index normalized ADR decision differs from ProductCommit' 'committed-adr-decision-change'
/usr/bin/sed -i '' 's/weakened decision 001/locked decision 001/' \
  "$TEST_ROOT/${ADR_PATHS[0]}"
git -C "$TEST_ROOT" add "${ADR_PATHS[0]}"
commit_fixture 'restore ADR decision'
assert_verifies "restoring the exact normalized ADR decision did not restore verification"

# The index and worktree are independently checked; restoring one side cannot
# conceal a forbidden change on the other side.
/usr/bin/sed -i '' 's/locked context 002/staged forbidden context 002/' \
  "$TEST_ROOT/${ADR_PATHS[1]}"
git -C "$TEST_ROOT" add "${ADR_PATHS[1]}"
restore_from_head "${ADR_PATHS[1]}"
assert_rejected 'index normalized ADR decision differs from ProductCommit' 'staged-adr-change'
git -C "$TEST_ROOT" reset -q HEAD -- "${ADR_PATHS[1]}"
assert_verifies "clearing the staged forbidden ADR change did not restore verification"

/usr/bin/sed -i '' 's/locked context 003/unstaged forbidden context 003/' \
  "$TEST_ROOT/${ADR_PATHS[2]}"
assert_rejected 'worktree normalized ADR decision differs from ProductCommit' 'unstaged-adr-change'
restore_from_head "${ADR_PATHS[2]}"
assert_verifies "restoring the unstaged forbidden ADR change did not restore verification"

write_adr "$TEST_ROOT/docs/current/adr/ADR-010-extra.md" '010'
assert_rejected 'worktree ADR inventory must contain exactly ADR-001 through ADR-009' 'added-adr'
/bin/rm "$TEST_ROOT/docs/current/adr/ADR-010-extra.md"

/bin/mv "$TEST_ROOT/${ADR_PATHS[6]}" "$TEST_ROOT/ADR-007.saved"
assert_rejected 'worktree ADR inventory must contain exactly ADR-001 through ADR-009' 'deleted-adr'
/bin/mv "$TEST_ROOT/ADR-007.saved" "$TEST_ROOT/${ADR_PATHS[6]}"
assert_verifies "restoring the exact ADR inventory did not restore verification"

LOCKED_PATHS=(
  docs/current/security/PLAINTEXT_SECRET_INVENTORY.md
  docs/privacy.md
  docs/current/evidence/v0.2-v0.3/DOGFOOD_RUNBOOK.md
  docs/current/evidence/v0.2-v0.3/DOGFOOD_ATTESTATION.template.json
)
for index in $(/usr/bin/jot 4 1); do
  path="${LOCKED_PATHS[$((index - 1))]}"
  printf 'forbidden mutation\n' >> "$TEST_ROOT/$path"
  assert_rejected 'tracked RC input differs from ProductCommit' "locked-product-input-$index"
  restore_from_head "$path"
done
assert_verifies "restoring all fully locked ProductInputs did not restore verification"

printf 'weakened acceptance contract\n' > "$TEST_ROOT/docs/current/PRODUCT_SPEC.md"
git -C "$TEST_ROOT" add docs/current/PRODUCT_SPEC.md
commit_fixture 'changed acceptance contract'
assert_rejected 'tracked RC input differs from ProductCommit' 'spec-change'
printf 'frozen acceptance contract\n' > "$TEST_ROOT/docs/current/PRODUCT_SPEC.md"
git -C "$TEST_ROOT" add docs/current/PRODUCT_SPEC.md
commit_fixture 'restore acceptance contract'
assert_verifies "restoring the exact frozen acceptance contract did not restore verification"

# --- public source export: an empty tracked PUBLIC_BUILD file and no internal ADR documents
PUBLIC_ROOT="$TEST_ROOT/public-export"
rm -rf "$PUBLIC_ROOT"
mkdir -p "$PUBLIC_ROOT"
(cd "$TEST_ROOT" && git ls-files -z | /usr/bin/grep -zv '^docs/current/adr/' | xargs -0 -I{} /bin/sh -c 'mkdir -p "$1/$(dirname "$2")" && cp "$2" "$1/$2"' _ "$PUBLIC_ROOT" {})
public_commit() {
  git -C "$PUBLIC_ROOT" -c user.name='Agent IME Test' -c user.email='agent-ime-test@invalid' -c commit.gpgsign=false commit -q -m "$1"
}
public_verifies() {
  [[ "$("$SUPPORT_TOOL" verify-inputs "$PUBLIC_ROOT" "$PUBLIC_COMMIT" 2> "$PUBLIC_ROOT.stderr")" == "$PUBLIC_COMMIT" ]] || fail "$1: $(cat "$PUBLIC_ROOT.stderr")"
}
public_rejected() {
  if "$SUPPORT_TOOL" verify-inputs "$PUBLIC_ROOT" "$PUBLIC_COMMIT" > /dev/null 2> "$PUBLIC_ROOT.stderr"; then fail "$2 unexpectedly verified"; fi
  /usr/bin/grep -Fq "$1" "$PUBLIC_ROOT.stderr" || fail "$2 failure was not explicit: $(cat "$PUBLIC_ROOT.stderr")"
}
git -C "$PUBLIC_ROOT" init -q
git -C "$PUBLIC_ROOT" add -A
public_commit 'public export without the marker'
PUBLIC_COMMIT="$(git -C "$PUBLIC_ROOT" rev-parse HEAD)"
public_rejected 'ADR inventory must contain exactly ADR-001 through ADR-009' 'no-marker-no-adr'

: > "$PUBLIC_ROOT/PUBLIC_BUILD"
public_rejected 'ADR inventory must contain exactly ADR-001 through ADR-009' 'untracked-marker'
git -C "$PUBLIC_ROOT" add PUBLIC_BUILD
public_commit 'public export with the marker'
PUBLIC_COMMIT="$(git -C "$PUBLIC_ROOT" rev-parse HEAD)"
public_verifies "a public export with a tracked empty PUBLIC_BUILD did not verify"
"$SUPPORT_TOOL" is-public-build "$PUBLIC_ROOT" || fail "is-public-build did not recognise a public export"
if "$SUPPORT_TOOL" is-public-build "$TEST_ROOT" 2>/dev/null; then fail "is-public-build recognised the private repository"; fi
PUBLIC_FINGERPRINT="$("$SUPPORT_TOOL" fingerprint "$PUBLIC_ROOT" "$PUBLIC_COMMIT")"
[[ "$PUBLIC_FINGERPRINT" =~ ^[0-9a-f]{64}$ ]] || fail "public export fingerprint is invalid"
[[ "$PUBLIC_FINGERPRINT" != "$PRODUCT_FINGERPRINT" ]] || fail "public and private fingerprints must differ"

# every other check still runs in a public build
printf 'public export changed input\n' > "$PUBLIC_ROOT/src/input-1.txt"
public_rejected 'tracked RC input differs from ProductCommit' 'public-changed-input'
git -C "$PUBLIC_ROOT" checkout -q -- src/input-1.txt
public_verifies "restoring the public input did not restore verification"
printf 'untracked build input\n' > "$PUBLIC_ROOT/src/untracked-build-input.txt"
public_rejected 'untracked RC build/gate input is forbidden' 'public-untracked-input'
rm -f "$PUBLIC_ROOT/src/untracked-build-input.txt"
public_verifies "removing the untracked public input did not restore verification"

# a non-empty marker is not accepted; an untracked one in the private repository changes nothing (the ADR checks still run there)
printf 'x\n' > "$PUBLIC_ROOT/PUBLIC_BUILD"
git -C "$PUBLIC_ROOT" add PUBLIC_BUILD
public_commit 'non-empty marker'
PUBLIC_COMMIT="$(git -C "$PUBLIC_ROOT" rev-parse HEAD)"
public_rejected 'ADR inventory must contain exactly ADR-001 through ADR-009' 'non-empty-marker'
: > "$TEST_ROOT/PUBLIC_BUILD"
assert_verifies "an untracked PUBLIC_BUILD must not change how the private repository verifies"
git -C "$TEST_ROOT" add PUBLIC_BUILD
mv "$TEST_ROOT/docs/current/adr/ADR-009-context-routing.md" "$TEST_ROOT/adr9.saved"
# PUBLIC_BUILD is a tracked input: adding it after the ProductCommit (even with a deleted ADR) is a change of inputs, not a way around the ADR checks
assert_rejected 'tracked RC input differs from ProductCommit' 'a-marker-added-after-the-product-commit-cannot-hide-a-missing-adr'
mv "$TEST_ROOT/adr9.saved" "$TEST_ROOT/docs/current/adr/ADR-009-context-routing.md"
git -C "$TEST_ROOT" rm -q --cached PUBLIC_BUILD
rm -f "$TEST_ROOT/PUBLIC_BUILD"
assert_verifies "the private repository stopped verifying after the marker experiment"

printf 'changed input\n' > "$TEST_ROOT/src/input-1.txt"
git -C "$TEST_ROOT" add src/input-1.txt
commit_fixture 'changed product input'
assert_rejected 'tracked RC input differs from ProductCommit' 'input-change'

PRODUCT_TREE="$(git -C "$TEST_ROOT" rev-parse "$PRODUCT_COMMIT^{tree}")"
UNRELATED_COMMIT="$(
  printf 'unrelated history\n' | \
    env GIT_AUTHOR_NAME='Agent IME Test' GIT_AUTHOR_EMAIL='agent-ime-test@invalid' \
      GIT_COMMITTER_NAME='Agent IME Test' GIT_COMMITTER_EMAIL='agent-ime-test@invalid' \
      git -C "$TEST_ROOT" commit-tree "$PRODUCT_TREE"
)"
git -C "$TEST_ROOT" update-ref refs/heads/unrelated "$UNRELATED_COMMIT"
git -C "$TEST_ROOT" symbolic-ref HEAD refs/heads/unrelated
assert_rejected 'ProductCommit is not an ancestor of HEAD' 'non-ancestor'

echo "RC gate support tests passed"
