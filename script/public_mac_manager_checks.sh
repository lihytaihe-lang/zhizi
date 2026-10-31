#!/usr/bin/env bash
# Static checks on the public manager binary, sourced by package_public_mac.sh and by test_package_public_mac_checks.sh.
# Rust glues neighbouring string literals together, so the one legitimate use of "test-upload" - the label of the hourly task the
# cleanup retires - is cut out as a substring (not a whole line) before the forbidden-string scan. Nothing else in the cleanup code
# may contain that word.

FORBIDDEN_STRINGS=('zhizi.ihongui' 'test-upload' 'personalProfileV1Seeded')

assert_manager_binary_clean() {
  local executable="$1" pattern
  for pattern in "${FORBIDDEN_STRINGS[@]}"; do
    # No grep -q here: with pipefail, grep -q's early exit SIGPIPEs strings and turns a hit into a non-zero pipeline status.
    if strings "$executable" | {
      if [[ "$pattern" == 'test-upload' ]]; then
        sed -e 's/com\.agentinputlayer\.zhizi\.test-upload//g'
      else
        cat
      fi
    } | grep -F -- "$pattern" >/dev/null; then
      echo "selfcheck FAILED: '$pattern' found in $executable" >&2
      echo "the manager looks like a default-features (test-upload) build; rebuild with:" >&2
      echo "  ZHIZI_MANAGER_CARGO_FLAGS=--no-default-features script/build_manager.sh" >&2
      return 1
    fi
  done
}

# The public manager must still retire the test build's hourly upload task (see docs/current/tasks/FIX-PLAN-STALE-UPLOAD-TASK-2026-10-23.md).
assert_manager_has_legacy_cleanup() {
  local executable="$1" pattern
  # '--upload-test-logs' is not checked: the compiler turns a comparison with a short literal into integer compares, so the
  # string may not exist in the binary; that function is covered by unit tests instead.
  for pattern in 'com.agentinputlayer.zhizi.test-upload' 'Library/LaunchAgents' 'bootout'; do
    if ! strings "$executable" | grep -F -- "$pattern" >/dev/null; then
      echo "selfcheck FAILED: legacy upload-task cleanup code is missing ('$pattern' not in $executable)" >&2
      return 1
    fi
  done
}
