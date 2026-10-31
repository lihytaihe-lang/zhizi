#!/usr/bin/env bash
# Tests for script/public_mac_manager_checks.sh with fake "manager binaries" (plain files); builds nothing.
set -uo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/script/public_mac_manager_checks.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
expect() { # expect <ok|reject> <description> <command...>
  local want="$1" what="$2"; shift 2
  if "$@" >/dev/null 2>&1; then got=ok; else got=reject; fi
  if [[ "$got" == "$want" ]]; then echo "ok   $what"; else echo "FAIL $what (wanted $want, got $got)"; fail=1; fi
}
printf 'plain\ncom.agentinputlayer.zhizi.test-upload\nLibrary/LaunchAgents\nbootout\n' > "$T/clean-public"
printf 'xxcom.agentinputlayer.zhizi.test-uploadLibrary/LaunchAgentsbootoutyy\n' > "$T/clean-glued"
printf 'plain\ncom.agentinputlayer.zhizi.test-upload\nLibrary/LaunchAgents\nbootout\nhttps://zhizi.ihongui.com\n' > "$T/leaks-url"
printf 'plain\nbootout\nLibrary/LaunchAgents\ntest-upload\n' > "$T/leaks-feature"
printf 'plain\npersonalProfileV1Seeded\n' > "$T/leaks-seed"
printf 'plain\nnothing here\n' > "$T/no-cleanup"
printf 'com.agentinputlayer.zhizi.test-upload\nLibrary/LaunchAgents\n' > "$T/no-bootout"
expect ok     "public build: markers alone are not a leak"            assert_manager_binary_clean "$T/clean-public"
expect ok     "public build: markers glued to other literals"         assert_manager_binary_clean "$T/clean-glued"
expect reject "upload URL present"                                    assert_manager_binary_clean "$T/leaks-url"
expect reject "bare test-upload feature string present"               assert_manager_binary_clean "$T/leaks-feature"
expect reject "personal seed marker present"                          assert_manager_binary_clean "$T/leaks-seed"
expect ok     "cleanup code present"                                  assert_manager_has_legacy_cleanup "$T/clean-public"
expect ok     "cleanup code present, glued"                           assert_manager_has_legacy_cleanup "$T/clean-glued"
expect reject "cleanup code missing entirely"                         assert_manager_has_legacy_cleanup "$T/no-cleanup"
expect reject "bootout missing"                                       assert_manager_has_legacy_cleanup "$T/no-bootout"
[[ $fail -eq 0 ]] && echo "PASS: manager binary checks" || { echo "FAILED"; exit 1; }
