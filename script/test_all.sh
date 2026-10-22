#!/usr/bin/env bash
# The complete validation suite.  Every step below is a recorded gate: the
# runner (scripts/tools/regression_gate_lib.sh) captures each command's exit
# code, duration and per-gate log, keeps running independent gates after a
# failure, blocks only dependents of a failed prerequisite (deps / core_lib /
# bundle), and exits non-zero whenever any gate FAILs or is BLOCKED.  Nothing
# is inferred from historical logs and no exit code is a "known stop": a gate
# exiting 6 (quality-gate contract) fails the run exactly like any other
# non-zero exit.  `set -e` is deliberately absent — the suite must collect all
# independent evidence instead of stopping at the first red gate (the old
# behavior hid every gate after the first failure).
#
# Usage: script/test_all.sh          run the full suite
#        script/test_all.sh --list   print the gate table without running
set -u -o pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../tools/regression_gate_lib.sh disable=SC1091
source "$ROOT_DIR/scripts/tools/regression_gate_lib.sh"
regression_env_init test_all

if [[ "${1:-}" == "--list" ]]; then
  LIST_ONLY=1
elif [[ ${1:-} == "" ]]; then
  LIST_ONLY=0
else
  echo "usage: $0 [--list]" >&2
  exit 2
fi

cd "$ROOT_DIR"
# The runner's EXIT trap calls unregister_dev_bundles.sh after every exit,
# including signals: builds and fixtures share the installed input method's
# bundle identifier, so never leave one registered where LaunchServices could
# start it for the owner's typing.  It unregisters dev bundles only, never the
# owner's installed copy.

# --- static checks and unit tests (independent of any build artifact) --------
gate_add bootstrap_native "" deps '"$REG_SCRIPT_DIR/bootstrap_native.sh"'
gate_add heavy_garble_words_check "" "" 'node "$REG_ROOT/scripts/generate_heavy_garble_words.mjs" --check'
gate_add context_disambig_check "" "" 'node "$REG_ROOT/scripts/check_context_disambig.mjs"'
gate_add freeze_corpus_check "" "" 'node "$REG_ROOT/scripts/freeze_candidate_corpus.mjs"'
gate_add daily_dictionary_check "" "" 'node "$REG_ROOT/scripts/validate_agent_daily_dictionary.mjs"'
gate_add idiom_dictionary_check "" "" '"$REG_SCRIPT_DIR/check_idiom_dictionary.sh"'
gate_add modern_dictionary_check "" "" '"$REG_SCRIPT_DIR/check_modern_dictionary.sh"'
gate_add project_terms_check "" "" 'node "$REG_ROOT/scripts/generate_project_terms.mjs" --check'
gate_add exact_reading_index_check "" "" 'node "$REG_ROOT/scripts/generate_exact_reading_index.mjs" --check'
gate_add owner_error_ranking "" "" '"$REG_SCRIPT_DIR/test_owner_error_ranking.sh"'
gate_add owner_error_ranking_full "" "" '"$REG_SCRIPT_DIR/test_owner_error_ranking_full.sh"'
gate_add learning_cleanup_round5_evidence "" "" '"$REG_SCRIPT_DIR/test_learning_cleanup_round5_evidence.sh"'
gate_add owner_abbrev_competition "" "" '"$REG_SCRIPT_DIR/test_owner_abbrev_competition.sh"'
gate_add owner_abbrev_impact_full "" "" '"$REG_SCRIPT_DIR/test_owner_abbrev_impact_full.sh"'
gate_add owner_context_disambig "" "" '"$REG_SCRIPT_DIR/test_owner_context_disambig.sh"'
gate_add candidate_skins_check "" "" 'node "$REG_ROOT/scripts/generate_candidate_skins.mjs" --check'
gate_add native_license_assets_check "" "" 'node "$REG_ROOT/scripts/validate_native_license_assets.mjs"'
gate_add locked_dependency_registry_check "" "" 'node "$REG_ROOT/scripts/generate_locked_dependency_registry.mjs" --check'
gate_add v010_upgrade_fixture "" "" '"$REG_SCRIPT_DIR/test_v010_upgrade_fixture.sh"'
gate_add frontend_tests "" "" 'npm test'
# The "unittest" evaluation id: python unit tests for scripts/tools and
# scripts/eval (picks up test_build_friends_set.py and the regression-cycle
# self-test; the self-test only ever launches stubbed cycles, so this cannot
# recurse into a real run).
gate_add unittest "" "" '(cd "$REG_ROOT/scripts/tools" && PYTHONPATH="$REG_ROOT" python3 -m unittest discover -p "test_*.py") && (cd "$REG_ROOT/scripts/eval" && PYTHONPATH="$REG_ROOT" python3 -m unittest discover -p "test_*.py")'
gate_add dogfood_checkpoint "" "" 'node "$REG_ROOT/scripts/test_dogfood_checkpoint.mjs"'
gate_add rc_gate_support "" "" '"$REG_SCRIPT_DIR/test_rc_gate_support.sh"'
gate_add freeze_joint_rc_contract "" "" '"$REG_SCRIPT_DIR/test_freeze_joint_rc_contract.sh"'
gate_add replay_gate_contract "" "" '"$REG_SCRIPT_DIR/check_replay_gate_contract.sh"'
gate_add frontend_build "" "" 'npm run build'

# --- native toolchain gates (need the verified locked dependencies) ----------
gate_add cargo_test_src_tauri deps '' 'cargo test --locked --offline --manifest-path "$REG_ROOT/src-tauri/Cargo.toml"'
gate_add cargo_check_src_tauri deps '' 'cargo check --locked --offline --manifest-path "$REG_ROOT/src-tauri/Cargo.toml"'
gate_add cargo_test_core deps '' 'cargo test --locked --offline --manifest-path "$REG_ROOT/native/core/Cargo.toml"'
gate_add cargo_test_core_latency deps '' 'cargo test --release --locked --offline --manifest-path "$REG_ROOT/native/core/Cargo.toml" large_lexicon_keystroke_pipeline_meets_frozen_latency_budget -- --nocapture'
gate_add cargo_test_ipc deps '' 'cargo test --locked --offline --manifest-path "$REG_ROOT/native/ipc/Cargo.toml"'
gate_add cargo_build_core_release deps core_lib 'cargo build --locked --offline --manifest-path "$REG_ROOT/native/core/Cargo.toml" --release --features librime'
# The librime dylib is copied onto the workspace volume after Swift links the
# test bundle.  Re-sign that exact copy so macOS does not reject the first test
# load under external-volume library validation; no product binary is changed.
# The Swift test bundle links the current Rust core, so a failed core build
# must block it instead of letting it link a stale library.
gate_add swift_tests deps,core_lib '' 'touch "$REG_ROOT/native/macos/Sources/AgentIMECoreBridge/bridge.c" && swift build --package-path "$REG_ROOT/native/macos" -c debug --disable-automatic-resolution && swift_bin_dir="$(swift build --package-path "$REG_ROOT/native/macos" -c debug --disable-automatic-resolution --show-bin-path)" && cp "$REG_ROOT/.build-deps/librime/1.16.1/lib/librime.1.dylib" "$swift_bin_dir/librime.1.dylib" && codesign --force --sign - "$swift_bin_dir/librime.1.dylib" && swift test --package-path "$REG_ROOT/native/macos" -c debug --disable-automatic-resolution'

# --- release bundle and everything that consumes it --------------------------
gate_add build_release deps bundle '"$REG_SCRIPT_DIR/build_native.sh" release'
# Runs here, immediately after the release bundle exists, because the gate's
# binary inspection is fail-hard and needs a built host to examine.  Until
# 2026-07-25 this gate was invoked by nothing at all, while a dozen evidence
# files cited its PASS as the proof that the core never networks.
gate_add core_network_boundary bundle '' '"$REG_SCRIPT_DIR/check_core_network_boundary.sh" "$REG_ROOT/build/native/release/Agent Input Method.inputmethod"'
# Equivalence proof that artifact_checksum.sh hashes bundles identically to
# the legacy find|shasum pipeline (13 assertions, fixtures under mktemp).
# Needs=bundle, not dependency-free: the script exits 2 ("missing real
# bundle") when build/native/release/*.inputmethod is absent, and a
# dependency-free gate would turn every unbuilt checkout into a FAIL instead
# of a BLOCKED row tied to the failed build itself.
gate_add artifact_checksum_equivalence bundle '' '"$REG_SCRIPT_DIR/test_artifact_checksum.sh"'
# Builds its own release harness, which links the current Rust core.  Strict
# mode: a genuinely unavailable context model must FAIL the suite here — the
# runner would otherwise record the script's internal SKIP+exit-0 as PASS.
gate_add context_model_integration deps,core_lib '' 'AIM_CONTEXT_MODEL_REQUIRED=1 "$REG_SCRIPT_DIR/test_context_model_integration.sh"'
gate_add short_input_factory_top1 deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_short_input_factory_top1.sh"'
gate_add learning_degradation deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_learning_degradation.sh"'
gate_add dakle_owner_hit deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_dakle_owner_hit.sh"'
gate_add context_ranking_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_context_ranking_replay.sh"'
gate_add mixed_language_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_mixed_language_replay.sh"'
gate_add cn_en_mixed_narrow_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_cn_en_mixed_narrow_replay.sh"'
gate_add fuzzy_exact_priority_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_fuzzy_exact_priority_replay.sh"'
gate_add offcorpus_probe deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_offcorpus_probe.sh"'
gate_add q3b1_garble_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_q3b1_garble_replay.sh"'
gate_add q3b2_heavy_garble_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_q3b2_heavy_garble_replay.sh"'
gate_add partial_coverage_top1_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_partial_coverage_top1_replay.sh"'
gate_add q1_pointwise_invariance deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_q1_pointwise_invariance.sh"'
gate_add mixed_abbrev_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_mixed_abbrev_replay.sh"'
gate_add idiom_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_idiom_replay.sh"'
gate_add lexicon_l1_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_lexicon_l1_replay.sh"'
gate_add project_terms_replay deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_project_terms_replay.sh"'
gate_add runtime_storage_fallback deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_runtime_storage_fallback.sh"'
gate_add userdb_process_recovery deps,bundle '' '"$REG_SCRIPT_DIR/test_userdb_process_recovery.sh" --development-fixture "$REG_ROOT/build/native/release/Agent Input Method.inputmethod"'
# Re-runs bootstrap and builds the release core library itself, so it needs no
# prerequisite resource: with a broken dependency environment it fails on its
# own evidence instead of being blocked.
gate_add userdb_word_cleanup '' '' '"$REG_SCRIPT_DIR/test_userdb_word_cleanup.sh"'
gate_add performance_controller_path deps,core_lib,bundle '' '"$REG_SCRIPT_DIR/test_performance_controller_path.sh"'
gate_add install_input_method bundle '' '"$REG_SCRIPT_DIR/test_install_input_method.sh"'
gate_add build_manager deps '' 'CARGO_NET_OFFLINE=true "$REG_SCRIPT_DIR/build_manager.sh"'

gates_validate
if [[ "$LIST_ONLY" -eq 1 ]]; then
  gates_list
  exit 0
fi
gates_run
rc=$?
# The build number (1000 + commit count) is printed after the result, also when
# the suite is red; a HEAD that cannot be counted is a harness error, never a
# silent pass.  (scripts/tools/full_regression_cycle.sh execs into this suite,
# so this is the single place that owns it.)
if commit_count="$(git rev-list --count HEAD 2>/dev/null)"; then
  echo "BUILD_NUMBER=$((1000 + commit_count))"
else
  echo "cannot count commits for build number" >&2
  rc=2
fi
exit "$rc"
