# Testing a public checkout

Use the commands below from the checkout root to run source tests. These tests do not establish installed-input-method behavior, signing, notarization or release readiness.

## Prerequisites and commands

- Rust: install the Rust toolchain and prepare the dependency cache before running Cargo. Keep `--offline --locked` enabled and use writable `CARGO_TARGET_DIR` directories as shown. Public manager tests require `--no-default-features`.
- Frontend: install the project's npm dependencies, including TypeScript, before running `node scripts/run-frontend-tests.mjs`. Each fresh checkout needs its own dependencies.
- Python: the sprite-sheet tests require numpy/Pillow. Alignment batching requires Python 3.10+ for `zip(strict=True)`; use Python 3.11 or newer for the command below.
- Host: the command below builds only. Its default librime feature compiles a C shim and requires librime headers/libraries and a prepared `.build-deps` cache.
- Swift: the `swift_tests` gate requires macOS/Xcode, the Swift package at `native/macos`, the current Rust core, librime dylib and `codesign`. The gate runs `swift build` and `swift test` with `--disable-automatic-resolution`; see `script/test_all.sh:82` for the full preparation sequence.

```sh
CARGO_TARGET_DIR=/tmp/zhizi-public-core cargo test --offline --locked --manifest-path native/core/Cargo.toml
CARGO_TARGET_DIR=/tmp/zhizi-public-manager cargo test --offline --locked --no-default-features --manifest-path src-tauri/Cargo.toml
CARGO_TARGET_DIR=/tmp/zhizi-public-manager cargo test --offline --locked --no-default-features --manifest-path src-tauri/Cargo.toml --bins
CARGO_TARGET_DIR=/tmp/zhizi-public-host cargo build --offline --locked --manifest-path native/host/Cargo.toml
node scripts/run-frontend-tests.mjs
(cd scripts/tools && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v test_public_export test_build_sprite_sheet test_full_regression_cycle)
(cd scripts/eval && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v test_build_friends_set)
(cd scripts/context_model && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v test_copy_bundle_model test_copy_bundle_model_tiers test_quality_metadata_compatibility)
(cd scripts/context_model/zhizi && PYTHONDONTWRITEBYTECODE=1 python3.11 -m unittest -v test_check_host_alignment_batching)
```

## Recorded test results

The existing result record has no version or date and is not a guarantee for the current checkout. A fresh-machine bootstrap has not been verified.

| Test or build | Recorded result |
| --- | --- |
| Core | 212 passed / 0 failed / 6 ignored (measured 2026-10-22 on the exported tree) |
| Manager library | 127 passed / 0 failed / 1 ignored (measured 2026-10-22 on the exported tree, `cargo test --lib --no-default-features`) |
| Manager binaries | 31 passed |
| Host build | Exit 0 |
| Frontend (Node) | 43 passed |
| Sprite sheet | 10 passed |
| Public exporter | 19 passed |
| Regression runner self-tests | 17 passed |
| Friends-set | 7 passed |
| Original model copy | 2 passed |
| Model tiers | 11 passed |
| Metadata compatibility | 1 passed |
| Alignment batching (Python 3.11) | 7 passed |

The regression runner self-tests use synthetic stub commands and do not run real regression or product builds. `test_fetch_public_model` runs 13 tests, with 12 passed and 1 error in the existing record; it does not download models during testing. The Python 3.9 alignment-batching record contains 5 errors. The core/host compilation record reports no further missing embedded source inputs with four synthetic fixtures included; the manager record also contains a missing runtime test input.

### Known failing tests

- Manager library: none when run the way the public build is configured (`--no-default-features`). With the default features the tests do not compile in the public tree, because the test-upload module and the owner's seed files are not part of it.
- `test_fetch_public_model`: 1 error out of 13 tests; the individual test name was not recorded and is unverified.
- Alignment batching under Python 3.9: 5 errors; individual test names were not recorded and are unverified.

## Regression gates and availability

`script/test_all.sh --list` lists 59 gates. Use that inventory and the prerequisites below to choose applicable gates; do not use the full suite as a shortcut to public-checkout acceptance. It includes historical Git provenance, product/release builds and installer checks.

Categories describe prerequisites, not passing results: 1 = portable/offline/public source; 2 = macOS/Xcode or macOS shell tools; 3 = maintainer private data; 4 = network bootstrap. Several regression gates depend on private data on a maintainer's machine; the public repository does not provide the data, so those gates cannot run. Category 3 gates are grouped in this statement rather than listed individually below. The public manager commands above use a separate feature selection.

Categories 1 and 2 can still be unavailable where inputs are omitted. Missing inputs are not automatically private. Some generators require omitted `third_party`/ground-truth assets whose provenance and export availability remain unverified. Public dictionaries/provenance, dependency caches and build-generated temporary directories are separate prerequisites. Bootstrap-dependent builds can also require category 4 on an empty machine.

Known limits: sparse cold-start performance samples can occasionally exceed the frozen budget; withdrawing the latest selection cannot undo Rime's own learning.

| Gate | Category | Evidence and availability |
| --- | --- | --- |
| `bootstrap_native` | 4 | `script/test_all.sh:39; script/bootstrap_native.sh:257,296` — Downloads pinned librime and rime-pinyin-simp when cache absent; macOS bootstrap. |
| `heavy_garble_words_check` | 1 | `script/test_all.sh:40; scripts/generate_heavy_garble_words.mjs:12-24` — Reads pinned .build-deps pinyin dictionary and vendored Rime dictionaries; requires prepared cache. |
| `context_disambig_check` | 1 | `script/test_all.sh:41; scripts/check_context_disambig.mjs:9-13` — Reads third_party/context-disambig and quality/mined targets; omitted public inputs, privacy not established. |
| `freeze_corpus_check` | 1 | `script/test_all.sh:42; scripts/freeze_candidate_corpus.mjs:8-12` — Reads base/fuzzy ground-truth and generalization corpus; omitted inputs, not automatically private. |
| `daily_dictionary_check` | 1 | `script/test_all.sh:43; scripts/validate_agent_daily_dictionary.mjs:42-43` — Reads agent-modern-source.tsv and mixed-language-regression-v2.json, omitted public inputs. |
| `idiom_dictionary_check` | 1 | `script/test_all.sh:44; script/check_idiom_dictionary.sh:5-11; scripts/generate_idiom_dictionary.py:56,71,85` — Pinned pypinyin plus third_party/THUOCL text inputs not exported. |
| `modern_dictionary_check` | 1 | `script/test_all.sh:45; script/check_modern_dictionary.sh:5-10; scripts/generate_modern_dictionary.py:14` — Pinned pypinyin and omitted agent-modern-source.tsv. |
| `project_terms_check` | 1 | `script/test_all.sh:46; scripts/generate_project_terms.mjs:35-36,122,181,317` — Scans source/docs/ADR; omitted internal docs and new sources change derived snapshot; regeneration not verified. |
| `exact_reading_index_check` | 1 | `script/test_all.sh:47; scripts/generate_exact_reading_index.mjs:6-39,112-116` — Reads vendored Rime dictionaries; checks generated indices. |
| `candidate_skins_check` | 1 | `script/test_all.sh:54; scripts/generate_candidate_skins.mjs:7-12,255,377-387` — Enumerates skin packages and compares generated catalogs; public whitelist may differ; regeneration not verified. |
| `native_license_assets_check` | 1 | `script/test_all.sh:55; scripts/validate_native_license_assets.mjs:13-14,49-50,75` — Reads license manifest and repositoryPath assets, including omitted third_party/licenses; cache/toolchain required. |
| `locked_dependency_registry_check` | 1 | `script/test_all.sh:56; scripts/generate_locked_dependency_registry.mjs:8-11,58,73` — Reads package-lock and Cargo locks/metadata offline; cached dependencies required. |
| `v010_upgrade_fixture` | 2 | `script/test_all.sh:57; script/test_v010_upgrade_fixture.sh:5-7,28,44-55` — Synthetic sqlite fixture, macOS base64 -D, historical git commit required; pure export lacks history. |
| `frontend_tests` | 1 | `script/test_all.sh:58; scripts/run-frontend-tests.mjs:60-78; scripts/typescript-test-loader.mjs:4` — Synthetic src tests; installed TypeScript required. Installed TypeScript required; recorded result: 43 passed. |
| `unittest` | 2 | `script/test_all.sh:63; scripts/tools/test_full_regression_cycle.py:1-10` — tools/eval tests include synthetic shell runner tests using macOS utilities; no real suite is run. Individual portable tests listed below. |
| `dogfood_checkpoint` | 1 | `script/test_all.sh:64; scripts/test_dogfood_checkpoint.mjs:137-179,219-220` — Creates synthetic events in temporary directory; synthetic input. |
| `rc_gate_support` | 2 | `script/test_all.sh:65; script/test_rc_gate_support.sh:6-14,83-132` — Synthetic git fixture, BSD stat -f; synthetic input. |
| `freeze_joint_rc_contract` | 2 | `script/test_all.sh:66; script/test_freeze_joint_rc_contract.sh:11-20,227-228` — Synthetic fixture, BSD stat and xcrun Swift. |
| `replay_gate_contract` | 1 | `script/test_all.sh:67; script/check_replay_gate_contract.sh:17-23` — Static source contract only, no corpus or built bundle required. |
| `frontend_build` | 1 | `script/test_all.sh:68; package.json:9; scripts/typescript-test-loader.mjs:4` — Requires installed npm dependencies and root TypeScript/Vite/index configuration omitted by export; not verified. |
| `cargo_test_core` | 1 | `script/test_all.sh:73; native/core/agent-ime-core/src/state.rs:1638,2254` — Synthetic embedded corpus included; 167 passed, 6 ignored. |
| `cargo_test_core_latency` | 1 | `script/test_all.sh:74; ` — Same synthetic core data, release latency benchmark; timing sensitive, no recorded run. |
| `cargo_test_ipc` | 1 | `script/test_all.sh:75; native/ipc/Cargo.toml:1` — Rust IPC crate tests; cached Cargo dependencies required, not separately run. |
| `cargo_build_core_release` | 1 | `script/test_all.sh:76; native/core/agent-ime-core/src/rime.rs:1` — Requires prepared librime cache/toolchain; synthetic input. Release build no recorded run. |
| `swift_tests` | 2 | `script/test_all.sh:82; ` — Swift package, librime dylib, codesign; macOS/Xcode required. |
| `build_release` | 2 | `script/test_all.sh:85; script/build_native.sh:33,47-60,100-101` — macOS bundle and public model optional; bootstrap may need network (4); omitted third_party assets currently block. |
| `core_network_boundary` | 2 | `script/test_all.sh:90; script/check_core_network_boundary.sh:125-142` — Inspects built macOS bundle Mach-O and source; no actual networking. |
| `artifact_checksum_equivalence` | 2 | `script/test_all.sh:97; script/test_artifact_checksum.sh:9` — Creates synthetic checksum fixtures but also requires a real macOS bundle. |
| `short_input_factory_top1` | 2 | `script/test_all.sh:102; script/test_short_input_factory_top1.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `learning_degradation` | 2 | `script/test_all.sh:103; script/test_learning_degradation.sh:7` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `context_ranking_replay` | 2 | `script/test_all.sh:105; script/test_context_ranking_replay.sh:23` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `mixed_language_replay` | 2 | `script/test_all.sh:106; script/test_mixed_language_replay.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `cn_en_mixed_narrow_replay` | 2 | `script/test_all.sh:107; script/test_cn_en_mixed_narrow_replay.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `fuzzy_exact_priority_replay` | 2 | `script/test_all.sh:108; script/test_fuzzy_exact_priority_replay.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `offcorpus_probe` | 2 | `script/test_all.sh:109; script/test_offcorpus_probe.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `q3b1_garble_replay` | 2 | `script/test_all.sh:110; script/test_q3b1_garble_replay.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `q3b2_heavy_garble_replay` | 2 | `script/test_all.sh:111; script/test_q3b2_heavy_garble_replay.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `q1_pointwise_invariance` | 2 | `script/test_all.sh:113; script/test_q1_pointwise_invariance.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `mixed_abbrev_replay` | 2 | `script/test_all.sh:114; script/test_mixed_abbrev_replay.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `idiom_replay` | 2 | `script/test_all.sh:115; script/test_idiom_replay.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `lexicon_l1_replay` | 2 | `script/test_all.sh:116; script/test_lexicon_l1_replay.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `project_terms_replay` | 2 | `script/test_all.sh:117; script/test_project_terms_replay.sh:6` — macOS bundle/harness; referenced corpus is omitted from public export. Provenance not established; do not label missing data as private automatically. |
| `runtime_storage_fallback` | 2 | `script/test_all.sh:118; script/test_runtime_storage_fallback.sh:5-16,24-32` — Synthetic primary/fallback storage; built macOS bundle and Swift harness. |
| `userdb_process_recovery` | 2 | `script/test_all.sh:119; script/test_userdb_process_recovery.sh:64` — --development-fixture uses synthetic process/userdb data, built macOS bundle. |
| `userdb_word_cleanup` | 2 | `script/test_all.sh:123; script/test_userdb_word_cleanup.sh:5-27` — Synthetic temp UserDB; bootstrap can need network (4), Swift and release core build. |
| `performance_controller_path` | 2 | `script/test_all.sh:124; script/test_performance_controller_path.sh:19,29` — Synthetic base-500 plus macOS controller harness; sparse cold-start samples can exceed budget. |
| `install_input_method` | 2 | `script/test_all.sh:125; script/test_install_input_method.sh:7` — Installer tests use synthetic destinations and built macOS bundle; not authorization to install. |
