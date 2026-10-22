#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 verify-inputs <repo-root> [expected-commit] | fingerprint <repo-root> <commit> | is-public-build <repo-root>" >&2
  exit 2
}

[[ $# -ge 1 ]] || usage
COMMAND="$1"
shift

# A public source export (scripts/tools/public_export.py) carries a tracked, empty PUBLIC_BUILD file at its root and none of the
# internal ADR documents; only then are the ADR checks skipped. Every other check (tracked inputs equal to the commit, no untracked
# build inputs, no submodules, no symlinks, no skip-worktree flags) still runs, and PUBLIC_BUILD is itself a tracked input, so adding
# or removing it changes the fingerprint. The private repository has no such file.
public_build() {
  local root="$1"
  [[ -f "$root/PUBLIC_BUILD" && ! -L "$root/PUBLIC_BUILD" && ! -s "$root/PUBLIC_BUILD" ]] \
    && git -C "$root" ls-files --error-unmatch -- PUBLIC_BUILD >/dev/null 2>&1
}

# Keep this list deliberately broader than either product alone. A joint RC is
# valid only when every source, resource, quality fixture, build/gate script,
# lockfile and shipped license input is exactly the content recorded by the
# ProductCommit stamped into both bundles.
TRACKED_INPUT_PATHS=(
  PUBLIC_BUILD
  .gitattributes
  .gitignore
  .gitmodules
  .npmrc
  .node-version
  .nvmrc
  .tool-versions
  .cargo
  .swiftpm
  Cargo.toml
  Cargo.lock
  build.rs
  Package.swift
  Package.resolved
  rust-toolchain
  rust-toolchain.toml
  native
  src
  src-tauri
  quality
  script
  scripts
  index.html
  public
  package.json
  package-lock.json
  npm-shrinkwrap.json
  pnpm-lock.yaml
  yarn.lock
  '.env*'
  'tsconfig*.json'
  'vite.config.*'
  'postcss.config.*'
  'tailwind.config.*'
  THIRD_PARTY_NOTICES.md
  third_party/THUOCL
  third_party/licenses
  docs/current/PRODUCT_SPEC.md
  docs/current/TECHNICAL_SPEC.md
  docs/current/VERSION_ROADMAP.md
  docs/current/RELEASE_ACCEPTANCE_MATRIX.md
  docs/current/ADR_PLAN.md
  docs/current/EVIDENCE_PROTOCOL.md
  docs/current/tasks/V0.2_NATIVE_FOUNDATION.md
  docs/current/tasks/V0.3_TRADITIONAL_IME.md
  docs/current/SPEC_AUDIT_2026-07-12.md
  docs/current/security/PLAINTEXT_SECRET_INVENTORY.md
  docs/privacy.md
  docs/current/evidence/v0.2-v0.3/DOGFOOD_RUNBOOK.md
  docs/current/evidence/v0.2-v0.3/DOGFOOD_ATTESTATION.template.json
  docs/current/licenses
)

# ADR decisions are ProductInputs, but the evidence phase must be able to move
# their status and append verification results without rebuilding the product.
# These nine exact files therefore use a normalized fingerprint and verifier
# below instead of the byte-for-byte TRACKED_INPUT_PATHS comparison.
ADR_INPUT_PATHS=(
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

# Do not scan generated target/.build directories as untracked inputs. Scan all
# locations the compilers, bundlers, harnesses and packaging scripts can consume,
# without applying .gitignore or .git/info/exclude rules.
UNTRACKED_INPUT_PATHS=(
  .gitattributes
  .npmrc
  .node-version
  .nvmrc
  .tool-versions
  .cargo
  .swiftpm/configuration
  .swiftpm/configuration.json
  Cargo.toml
  Cargo.lock
  build.rs
  Package.swift
  Package.resolved
  rust-toolchain
  rust-toolchain.toml
  native/core/Cargo.toml
  native/core/Cargo.lock
  native/core/.cargo
  native/core/agent-ime-core
  native/ipc/Cargo.toml
  native/ipc/Cargo.lock
  native/ipc/.cargo
  native/ipc/build.rs
  native/ipc/src
  native/ipc/tests
  native/ipc/benches
  native/ipc/examples
  native/macos/Package.swift
  native/macos/Package.resolved
  native/macos/.swiftpm/configuration
  native/macos/.swiftpm/configuration.json
  native/macos/Resources
  native/macos/Sources
  native/macos/Tests
  native/resources
  src
  src-tauri/Cargo.toml
  src-tauri/Cargo.lock
  src-tauri/.cargo
  src-tauri/build.rs
  src-tauri/capabilities
  src-tauri/Info.plist
  src-tauri/Entitlements.plist
  src-tauri/entitlements.plist
  src-tauri/icons
  src-tauri/fixtures
  src-tauri/migrations
  src-tauri/src
  src-tauri/tests
  src-tauri/benches
  src-tauri/examples
  src-tauri/tauri.conf.json
  quality
  script
  scripts
  index.html
  public
  package.json
  package-lock.json
  npm-shrinkwrap.json
  pnpm-lock.yaml
  yarn.lock
  'tsconfig*.json'
  'vite.config.*'
  'postcss.config.*'
  'tailwind.config.*'
  THIRD_PARTY_NOTICES.md
  third_party/THUOCL
  third_party/licenses
  docs/current/PRODUCT_SPEC.md
  docs/current/TECHNICAL_SPEC.md
  docs/current/VERSION_ROADMAP.md
  docs/current/RELEASE_ACCEPTANCE_MATRIX.md
  docs/current/ADR_PLAN.md
  docs/current/EVIDENCE_PROTOCOL.md
  docs/current/tasks/V0.2_NATIVE_FOUNDATION.md
  docs/current/tasks/V0.3_TRADITIONAL_IME.md
  docs/current/SPEC_AUDIT_2026-07-12.md
  docs/current/security/PLAINTEXT_SECRET_INVENTORY.md
  docs/privacy.md
  docs/current/evidence/v0.2-v0.3/DOGFOOD_RUNBOOK.md
  docs/current/evidence/v0.2-v0.3/DOGFOOD_ATTESTATION.template.json
  docs/current/adr
  docs/current/licenses
  '.env*'
)

emit_expected_adr_inventory() {
  printf '%s\n' "${ADR_INPUT_PATHS[@]}" | LC_ALL=C sort
}

normalize_adr_stream() {
  LC_ALL=C /usr/bin/awk '
    BEGIN {
      status_count = 0
      verification_count = 0
      section_seen = 0
      in_verification = 0
    }
    /^## / {
      section_seen = 1
      if ($0 == "## Verification Evidence") {
        verification_count += 1
        in_verification = 1
        print $0
        print "<MUTABLE_VERIFICATION_EVIDENCE>"
        next
      }
      in_verification = 0
    }
    in_verification { next }
    /^- Status:/ {
      status_count += 1
      if (section_seen) status_after_section = 1
      if (!in_verification) print "- Status: <MUTABLE_STATUS>"
      next
    }
    { print }
    END {
      if (status_count != 1 || status_after_section || verification_count != 1) exit 3
    }
  '
}

assert_commit_adr_inventory() {
  local root="$1"
  public_build "$root" && return 0
  local commit="$2"
  local expected_file="$3"
  local actual_file="$4"
  emit_expected_adr_inventory > "$expected_file"
  git -C "$root" ls-tree -r --name-only "$commit" -- docs/current/adr \
    | LC_ALL=C sort > "$actual_file"
  cmp -s "$expected_file" "$actual_file" || {
    echo "ProductCommit ADR inventory must contain exactly ADR-001 through ADR-009" >&2
    return 3
  }
}

assert_index_adr_inventory() {
  local root="$1"
  public_build "$root" && return 0
  local expected_file="$2"
  local actual_file="$3"
  emit_expected_adr_inventory > "$expected_file"
  git -C "$root" ls-files -- docs/current/adr | LC_ALL=C sort > "$actual_file"
  cmp -s "$expected_file" "$actual_file" || {
    echo "index ADR inventory must contain exactly ADR-001 through ADR-009" >&2
    return 3
  }
}

assert_worktree_adr_inventory() {
  local root="$1"
  public_build "$root" && return 0
  local expected_file="$2"
  local actual_file="$3"
  emit_expected_adr_inventory > "$expected_file"
  (
    cd "$root"
    /usr/bin/find docs/current/adr -mindepth 1 -maxdepth 1 -print | LC_ALL=C sort
  ) > "$actual_file"
  cmp -s "$expected_file" "$actual_file" || {
    echo "worktree ADR inventory must contain exactly ADR-001 through ADR-009" >&2
    return 3
  }
}

normalize_commit_adr_to_file() {
  local root="$1"
  local commit="$2"
  local path="$3"
  local output="$4"
  local entry mode type object entry_path entry_count
  entry="$(git -C "$root" ls-tree "$commit" -- "$path")"
  entry_count="$(printf '%s\n' "$entry" | /usr/bin/awk 'NF { count += 1 } END { print count + 0 }')"
  [[ "$entry_count" == "1" ]] || {
    echo "ProductCommit ADR is missing or ambiguous: $path" >&2
    return 3
  }
  read -r mode type object entry_path <<< "$entry"
  [[ "$mode" == "100644" && "$type" == "blob" && "$entry_path" == "$path" ]] || {
    echo "ProductCommit ADR must be a regular non-executable blob: $path" >&2
    return 3
  }
  git -C "$root" cat-file blob "$object" | normalize_adr_stream > "$output" || {
    echo "ProductCommit ADR has an invalid mutable-section structure: $path" >&2
    return 3
  }
}

normalize_index_adr_to_file() {
  local root="$1"
  local path="$2"
  local output="$3"
  local entry mode object stage entry_path entry_count
  entry="$(git -C "$root" ls-files --stage -- "$path")"
  entry_count="$(printf '%s\n' "$entry" | /usr/bin/awk 'NF { count += 1 } END { print count + 0 }')"
  [[ "$entry_count" == "1" ]] || {
    echo "index ADR is missing, unmerged, or ambiguous: $path" >&2
    return 3
  }
  read -r mode object stage entry_path <<< "$entry"
  [[ "$mode" == "100644" && "$stage" == "0" && "$entry_path" == "$path" ]] || {
    echo "index ADR must be a stage-0 regular non-executable blob: $path" >&2
    return 3
  }
  git -C "$root" cat-file blob "$object" | normalize_adr_stream > "$output" || {
    echo "index ADR has an invalid mutable-section structure: $path" >&2
    return 3
  }
}

normalize_worktree_adr_to_file() {
  local root="$1"
  local path="$2"
  local output="$3"
  [[ -f "$root/$path" && ! -L "$root/$path" && ! -x "$root/$path" ]] || {
    echo "worktree ADR must be a regular non-executable file: $path" >&2
    return 3
  }
  normalize_adr_stream < "$root/$path" > "$output" || {
    echo "worktree ADR has an invalid mutable-section structure: $path" >&2
    return 3
  }
}

verify_normalized_adrs() {
  local root="$1"
  public_build "$root" && return 0
  local expected="$2"
  local temporary_root="$3"
  local path slug expected_normalized index_normalized worktree_normalized

  assert_commit_adr_inventory "$root" "$expected" \
    "$temporary_root/expected-commit-inventory" "$temporary_root/actual-commit-inventory"
  assert_index_adr_inventory "$root" \
    "$temporary_root/expected-index-inventory" "$temporary_root/actual-index-inventory"
  assert_worktree_adr_inventory "$root" \
    "$temporary_root/expected-worktree-inventory" "$temporary_root/actual-worktree-inventory"

  for path in "${ADR_INPUT_PATHS[@]}"; do
    slug="$(basename "$path" .md)"
    expected_normalized="$temporary_root/$slug.expected"
    index_normalized="$temporary_root/$slug.index"
    worktree_normalized="$temporary_root/$slug.worktree"
    normalize_commit_adr_to_file "$root" "$expected" "$path" "$expected_normalized"
    normalize_index_adr_to_file "$root" "$path" "$index_normalized"
    normalize_worktree_adr_to_file "$root" "$path" "$worktree_normalized"
    cmp -s "$expected_normalized" "$index_normalized" || {
      echo "index normalized ADR decision differs from ProductCommit: $path" >&2
      return 3
    }
    cmp -s "$expected_normalized" "$worktree_normalized" || {
      echo "worktree normalized ADR decision differs from ProductCommit: $path" >&2
      return 3
    }
  done
}

canonical_repo() {
  local requested="$1"
  local root top
  root="$(cd "$requested" && pwd -P)"
  top="$(git -C "$root" rev-parse --show-toplevel 2>/dev/null)" || {
    echo "RC input guard requires a Git worktree: $root" >&2
    return 3
  }
  top="$(cd "$top" && pwd -P)"
  [[ "$root" == "$top" ]] || {
    echo "RC input guard root is not the Git worktree root: $root != $top" >&2
    return 3
  }
  printf '%s\n' "$root"
}

resolve_commit() {
  local root="$1"
  local requested="$2"
  git -C "$root" rev-parse --verify "${requested}^{commit}" 2>/dev/null || {
    echo "invalid ProductCommit: $requested" >&2
    return 3
  }
}

fingerprint_commit() {
  local root="$1"
  local commit="$2"
  local entry_count temporary_root path slug normalized_file fingerprint
  entry_count="$(git -C "$root" ls-tree -r --full-tree "$commit" -- "${TRACKED_INPUT_PATHS[@]}" | wc -l | tr -d ' ')"
  [[ "$entry_count" =~ ^[0-9]+$ && "$entry_count" -ge 25 ]] || {
    echo "ProductCommit input set is unexpectedly small: $entry_count entries" >&2
    return 3
  }
  temporary_root="$(mktemp -d /tmp/agent-ime-rc-fingerprint.XXXXXX)"
  if ! assert_commit_adr_inventory "$root" "$commit" \
    "$temporary_root/expected-inventory" "$temporary_root/actual-inventory"; then
    rm -rf "$temporary_root"
    return 3
  fi
  for path in $(public_build "$root" || printf '%s\n' "${ADR_INPUT_PATHS[@]}"); do
    slug="$(basename "$path" .md)"
    normalized_file="$temporary_root/$slug.normalized"
    if ! normalize_commit_adr_to_file "$root" "$commit" "$path" "$normalized_file"; then
      rm -rf "$temporary_root"
      return 3
    fi
  done
  if ! fingerprint="$({
    LC_ALL=C git -C "$root" ls-tree -r --full-tree "$commit" -- "${TRACKED_INPUT_PATHS[@]}"
    for path in $(public_build "$root" || printf '%s\n' "${ADR_INPUT_PATHS[@]}"); do
      slug="$(basename "$path" .md)"
      normalized_file="$temporary_root/$slug.normalized"
      printf '100644 normalized-adr %s\t%s\n' \
        "$(shasum -a 256 "$normalized_file" | awk '{ print $1 }')" "$path"
    done
  } | LC_ALL=C sort | shasum -a 256 | awk '{ print $1 }')"; then
    rm -rf "$temporary_root"
    return 3
  fi
  rm -rf "$temporary_root"
  printf '%s\n' "$fingerprint"
}

verify_inputs() {
  local root="$1"
  local expected="$2"
  local head temporary_root untracked_file index_flags_file fingerprint

  head="$(git -C "$root" rev-parse HEAD)"
  git -C "$root" merge-base --is-ancestor "$expected" "$head" 2>/dev/null || {
    echo "ProductCommit is not an ancestor of HEAD: head=$head product=$expected" >&2
    return 3
  }

  temporary_root="$(mktemp -d /tmp/agent-ime-rc-inputs.XXXXXX)"
  trap 'rm -rf "$temporary_root"' RETURN

  if ! git -C "$root" diff --no-ext-diff --quiet "$expected" -- "${TRACKED_INPUT_PATHS[@]}" \
    || ! git -C "$root" diff --no-ext-diff --cached --quiet "$expected" -- "${TRACKED_INPUT_PATHS[@]}"; then
    echo "tracked RC input differs from ProductCommit:" >&2
    git -C "$root" diff --no-ext-diff --name-status "$expected" -- "${TRACKED_INPUT_PATHS[@]}" >&2 || true
    git -C "$root" diff --no-ext-diff --cached --name-status "$expected" -- "${TRACKED_INPUT_PATHS[@]}" >&2 || true
    echo "frozen_gates=SKIPPED reason=dirty-tree" >&2
    return 3
  fi

  verify_normalized_adrs "$root" "$expected" "$temporary_root"

  untracked_file="$temporary_root/untracked"
  index_flags_file="$temporary_root/index-flags"

  git -C "$root" ls-files -z --others -- "${UNTRACKED_INPUT_PATHS[@]}" > "$untracked_file"
  if [[ -s "$untracked_file" ]]; then
    echo "untracked RC build/gate input is forbidden:" >&2
    tr '\0' '\n' < "$untracked_file" >&2
    return 3
  fi

  git -C "$root" ls-files -v -- "${TRACKED_INPUT_PATHS[@]}" "${ADR_INPUT_PATHS[@]}" > "$index_flags_file"
  if awk 'substr($0, 1, 1) != "H" { found=1; print } END { exit(found ? 0 : 1) }' \
    "$index_flags_file" >&2; then
    echo "skip-worktree/assume-unchanged or unsupported index flags are forbidden for RC inputs" >&2
    return 3
  fi

  if git -C "$root" ls-files --stage -- "${TRACKED_INPUT_PATHS[@]}" "${ADR_INPUT_PATHS[@]}" | awk '$1 == "160000" { found=1 } END { exit(found ? 0 : 1) }'; then
    echo "Git submodules are forbidden inside the RC input set" >&2
    return 3
  fi
  if git -C "$root" ls-files --stage -- "${TRACKED_INPUT_PATHS[@]}" "${ADR_INPUT_PATHS[@]}" | awk '$1 == "120000" { found=1 } END { exit(found ? 0 : 1) }'; then
    echo "Git symlinks are forbidden inside the RC input set" >&2
    return 3
  fi

  fingerprint="$(fingerprint_commit "$root" "$expected")"
  [[ "$fingerprint" =~ ^[0-9a-f]{64}$ ]] || {
    echo "unable to fingerprint ProductCommit inputs" >&2
    return 3
  }
  rm -rf "$temporary_root"
  trap - RETURN
  printf '%s\n' "$expected"
}

case "$COMMAND" in
  verify-inputs)
    [[ $# -ge 1 && $# -le 2 ]] || usage
    ROOT="$(canonical_repo "$1")"
    REQUESTED_COMMIT="${2:-HEAD}"
    COMMIT="$(resolve_commit "$ROOT" "$REQUESTED_COMMIT")"
    verify_inputs "$ROOT" "$COMMIT"
    ;;
  fingerprint)
    [[ $# -eq 2 ]] || usage
    ROOT="$(canonical_repo "$1")"
    COMMIT="$(resolve_commit "$ROOT" "$2")"
    fingerprint_commit "$ROOT" "$COMMIT"
    ;;
  is-public-build)
    [[ $# -eq 1 ]] || usage
    public_build "$(canonical_repo "$1")"
    ;;
  *)
    usage
    ;;
esac
