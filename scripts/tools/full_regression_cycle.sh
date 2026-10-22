#!/usr/bin/env bash
# The single full-validation entry.  It execs into script/test_all.sh, which
# owns the complete 59-gate list, the recording runner, dependency blocking,
# signal handling, the unregister-dev-bundles EXIT trap and the final build
# number — exactly one runner layer and one copy of the gate list.
#
# The old wrapper pre-ran build_native.sh, echoed exit codes it ignored, and
# re-ran nine tail checks to compensate for test_all.sh stopping at its first
# failure — while still exiting 0 on failure.  All of that is gone; the exit
# code of this entry is the honest verdict of the suite.
#
# Usage: scripts/tools/full_regression_cycle.sh [--list] > log 2>&1
set -u

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
exec "$ROOT_DIR/script/test_all.sh" "$@"
