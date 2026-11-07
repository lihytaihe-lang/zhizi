# 公开仓库测试说明

在仓库根目录使用以下命令运行源码测试。这些测试不代表已安装输入法的行为、签名、公证或发布验收。

## 前置条件与命令

- Rust：预装 Rust 工具链并准备依赖缓存。保留 `--offline --locked`，按下列命令使用可写的 `CARGO_TARGET_DIR` 目录。公开管理台测试须用 `--no-default-features`。
- 前端：运行 `node scripts/run-frontend-tests.mjs` 前，预装项目 npm 依赖，包括 TypeScript。每份新克隆都需要自己的依赖。
- Python：sprite-sheet 测试需要 numpy/Pillow。alignment batching 的 `zip(strict=True)` 需要 Python 3.10+；以下命令使用 Python 3.11 或更新版本。
- Host：以下命令只构建。需要准备好的 `.build-deps` 缓存；构建已不再依赖 librime。
- Swift：`swift_tests` 门需要 macOS/Xcode、`native/macos` 中的 Swift package、当前 Rust core 和 `codesign`。该门使用 `--disable-automatic-resolution` 运行 `swift build` 和 `swift test`；完整准备步骤见 `script/test_all.sh:82`。

```sh
CARGO_TARGET_DIR=/tmp/zhizi-public-core cargo test --offline --locked --manifest-path native/core/Cargo.toml
CARGO_TARGET_DIR=/tmp/zhizi-public-manager cargo test --offline --locked --no-default-features --manifest-path src-tauri/Cargo.toml
CARGO_TARGET_DIR=/tmp/zhizi-public-manager cargo test --offline --locked --no-default-features --manifest-path src-tauri/Cargo.toml --bins
CARGO_TARGET_DIR=/tmp/zhizi-public-host cargo build --offline --locked --manifest-path native/host/Cargo.toml
node scripts/run-frontend-tests.mjs
(cd scripts/tools && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v test_public_export test_make_public_repo test_sync_version test_full_regression_cycle)
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v scripts.tools.test_build_sprite_sheet scripts.lexicon.test_build_catalog scripts.lexicon.test_build_release_pack scripts.lexicon.test_check_contribution
(cd scripts/eval && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v test_build_friends_set)
(cd scripts/context_model && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest -v test_copy_bundle_model test_copy_bundle_model_tiers test_quality_metadata_compatibility test_fetch_public_model)
(cd scripts/context_model/zhizi && PYTHONDONTWRITEBYTECODE=1 python3.11 -m unittest -v test_check_host_alignment_batching)
```

## 测试结果

2026-10-23 在 0.1.5 的全新导出树（628 个文件）上，用上面的命令实测，macOS（Apple silicon）。系统 `python3` 是 3.9.6；alignment batching 用 Python 3.11 运行。数字只描述这一份检出，不代表之后的版本。

| 测试或构建 | 结果 |
| --- | --- |
| Core | 218 passed / 0 failed / 6 ignored |
| 管理台主库（`--no-default-features`） | 135 passed / 0 failed / 1 ignored |
| 管理台二进制 | 31 passed |
| Host 构建 | 退出 0 |
| 前端（Node） | 48 passed |
| 公开导出器（`test_public_export`） | 25 passed |
| 公开仓库生成器（`test_make_public_repo`） | 12 passed |
| 版本同步（`test_sync_version`） | 5 passed |
| 回归 runner 自测（`test_full_regression_cycle`） | 17 passed（约 200 秒） |
| Sprite sheet（须从仓库根目录运行） | 10 passed |
| 词库工具（须从仓库根目录运行） | 42 passed |
| Friends-set | 7 passed |
| 模型拷贝 | 2 passed |
| 模型三档 | 12 passed |
| Metadata compatibility | 1 passed |
| 公开模型下载 | 13 passed |
| Alignment batching（Python 3.11） | 7 passed |

导出树里未测：Swift 测试（开发检出里执行 246 项、跳过 16 项、0 失败；那里准备好了完整的 macOS 构建）和下一节的回归门。

回归 runner 自测使用合成 stub 命令，不执行真实回归或产品构建。公开模型下载的测试不下载模型。

### 已知失败的测试

下列测试只在运行方式不对时失败，各有通过的运行方式。

- 在自己目录里运行的 sprite sheet 和词库工具：`test_build_sprite_sheet` 有 2 项错误（`test_keying_soft_edges_and_despill`、`test_existing_alpha_and_euclidean_tolerance`），`test_check_contribution` 有 1 项（`test_inventory`），都是 `ModuleNotFoundError: No module named 'scripts'`。它们导入 `scripts.…`，须像上面的命令那样从仓库根目录运行。
- Python 3.9 下的 alignment batching：7 项中 5 项出错，`TypeError: zip() takes no keyword arguments`（`zip(strict=True)` 需要 Python 3.10+）；Python 3.11 下全部通过。
- 带默认特性的管理台主库：测试在公开树里编译不过，因为测试上传模块和维护者的种子文件不在公开树里。请用 `--no-default-features`。

## 回归门与可用性

`script/test_all.sh --list` 列出 59 个门。根据该清单和下列前置条件选择适用的门，不要把整轮套件当作公开仓库验收的捷径。其中包含历史 Git 溯源、产品/release 构建和安装检查。

分类表示前置条件，不等于通过：1=可移植、离线、公开源码；2=需要 macOS/Xcode 或 macOS shell 工具；3=维护者私有数据；4=联网 bootstrap。若干回归门依赖维护者本机私有数据，公开仓库不提供数据，无法运行。第 3 类门合并在本句说明，不在下表逐项列名。上面的公开管理台命令使用单独的 feature 选择。

第 1、2 类在缺少输入时也可能无法运行。未导出的输入不自动等于私有数据。部分生成器需要未导出的 `third_party`/ground-truth 文件，其来源与公开可用性未核实。公开词库及来源记录、依赖缓存、构建生成的临时目录是不同的前置条件。需要 bootstrap 的构建在空缓存机器上也可能需要第 4 类条件。

已知限制：性能门冷启动取样少时可能偶发超出冻结预算；撤回最近一次选择无法撤销 Rime 自己的学习。

| 门 | 分类 | 依据与可用性 |
| --- | --- | --- |
| `bootstrap_native` | 4 | `script/test_all.sh:39; script/bootstrap_native.sh:257,296` — 空缓存时下载固定版本的 rime-pinyin-simp 词库和 Rust 许可文件；macOS bootstrap。 |
| `heavy_garble_words_check` | 1 | `script/test_all.sh:40; scripts/generate_heavy_garble_words.mjs:12-24` — 读取 .build-deps 中固定版本的拼音词典和仓库内 Rime 词典；需要准备缓存。 |
| `context_disambig_check` | 1 | `script/test_all.sh:41; scripts/check_context_disambig.mjs:9-13` — 缺 third_party/context-disambig 与 quality/mined targets；尚不能认定全部为私有。 |
| `freeze_corpus_check` | 1 | `script/test_all.sh:42; scripts/freeze_candidate_corpus.mjs:8-12` — 缺 base/fuzzy ground-truth 与 generalization corpus；未自动判为私有。 |
| `daily_dictionary_check` | 1 | `script/test_all.sh:43; scripts/validate_agent_daily_dictionary.mjs:42-43` — 缺 agent-modern-source.tsv 与 mixed-language-regression-v2.json。 |
| `idiom_dictionary_check` | 1 | `script/test_all.sh:44; script/check_idiom_dictionary.sh:5-11; scripts/generate_idiom_dictionary.py:56,71,85` — 需固定 pypinyin 与未导出的 THUOCL 源文本。 |
| `modern_dictionary_check` | 1 | `script/test_all.sh:45; script/check_modern_dictionary.sh:5-10; scripts/generate_modern_dictionary.py:14` — 需固定 pypinyin 与未导出的 agent-modern-source.tsv。 |
| `project_terms_check` | 1 | `script/test_all.sh:46; scripts/generate_project_terms.mjs:35-36,122,181,317` — 扫描源码、文档、ADR；公开树缺内部文档且新增文件使派生快照过期，未重生成。 |
| `exact_reading_index_check` | 1 | `script/test_all.sh:47; scripts/generate_exact_reading_index.mjs:6-39,112-116` — 读取仓库内 Rime 词典，检查生成的索引。 |
| `candidate_skins_check` | 1 | `script/test_all.sh:54; scripts/generate_candidate_skins.mjs:7-12,255,377-387` — 枚举皮肤并比对派生目录；公开白名单可能改变结果，未重生成。 |
| `native_license_assets_check` | 1 | `script/test_all.sh:55; scripts/validate_native_license_assets.mjs:13-14,49-50,75` — 许可清单指向未导出的 third_party/licenses；需缓存/工具链。 |
| `locked_dependency_registry_check` | 1 | `script/test_all.sh:56; scripts/generate_locked_dependency_registry.mjs:8-11,58,73` — 离线读取 package-lock、Cargo 锁文件及 metadata；需要依赖缓存。 |
| `v010_upgrade_fixture` | 2 | `script/test_all.sh:57; script/test_v010_upgrade_fixture.sh:5-7,28,44-55` — 合成 sqlite 夹具；macOS base64 -D；需要历史 Git commit，纯导出目录没有历史。 |
| `frontend_tests` | 1 | `script/test_all.sh:58; scripts/run-frontend-tests.mjs:60-78; scripts/typescript-test-loader.mjs:4` — 合成测试 48 项通过，需预装 TypeScript。 |
| `unittest` | 2 | `script/test_all.sh:63; scripts/tools/test_full_regression_cycle.py:1-10` — tools/eval 包含使用 macOS 工具的合成 shell runner 自测；不是执行真实整轮。 |
| `dogfood_checkpoint` | 1 | `script/test_all.sh:64; scripts/test_dogfood_checkpoint.mjs:137-179,219-220` — 在临时目录创建合成事件。 |
| `rc_gate_support` | 2 | `script/test_all.sh:65; script/test_rc_gate_support.sh:6-14,83-132` — 合成 Git 夹具；需要 BSD stat -f。 |
| `freeze_joint_rc_contract` | 2 | `script/test_all.sh:66; script/test_freeze_joint_rc_contract.sh:11-20,227-228` — 合成夹具；需要 BSD stat 和 xcrun Swift。 |
| `replay_gate_contract` | 1 | `script/test_all.sh:67; script/check_replay_gate_contract.sh:17-23` — 只检查静态源码契约，不需要语料或构建后的 bundle。 |
| `frontend_build` | 1 | `script/test_all.sh:68; package.json:9; scripts/typescript-test-loader.mjs:4` — 需预装 npm 依赖及未导出的根 TypeScript/Vite/index 配置；未验收。 |
| `cargo_test_core` | 1 | `script/test_all.sh:73; native/core/agent-ime-core/src/state.rs:1638,2254` — 包含合成嵌入夹具；218 项通过、6 项忽略。 |
| `cargo_test_core_latency` | 1 | `script/test_all.sh:74; ` — 合成数据的 release 延迟门，取样敏感；未运行。 |
| `cargo_test_ipc` | 1 | `script/test_all.sh:75; native/ipc/Cargo.toml:1` — Rust IPC crate 测试；需 Cargo 依赖缓存，没有单独运行记录。 |
| `cargo_build_core_release` | 1 | `script/test_all.sh:76` — 需准备依赖缓存和工具链；没有 release 构建记录。 |
| `swift_tests` | 2 | `script/test_all.sh:82; ` — Swift package、codesign；需要 macOS/Xcode，没有运行记录。 |
| `build_release` | 2 | `script/test_all.sh:85; script/build_native.sh:33,47-60,100-101` — macOS bundle；模型可选；bootstrap 可能联网；当前缺第三方资产。 |
| `core_network_boundary` | 2 | `script/test_all.sh:90; script/check_core_network_boundary.sh:125-142` — 检查构建后的 macOS bundle Mach-O 和源码，不发起实际网络请求；没有运行记录。 |
| `artifact_checksum_equivalence` | 2 | `script/test_all.sh:97; script/test_artifact_checksum.sh:9` — 创建合成校验和夹具，同时需要真实 macOS bundle；没有运行记录。 |
| `short_input_factory_top1` | 2 | `script/test_all.sh:102; script/test_short_input_factory_top1.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `learning_degradation` | 2 | `script/test_all.sh:103; script/test_learning_degradation.sh:7` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `context_ranking_replay` | 2 | `script/test_all.sh:105; script/test_context_ranking_replay.sh:23` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `mixed_language_replay` | 2 | `script/test_all.sh:106; script/test_mixed_language_replay.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `cn_en_mixed_narrow_replay` | 2 | `script/test_all.sh:107; script/test_cn_en_mixed_narrow_replay.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `fuzzy_exact_priority_replay` | 2 | `script/test_all.sh:108; script/test_fuzzy_exact_priority_replay.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `offcorpus_probe` | 2 | `script/test_all.sh:109; script/test_offcorpus_probe.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `q3b1_garble_replay` | 2 | `script/test_all.sh:110; script/test_q3b1_garble_replay.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `q3b2_heavy_garble_replay` | 2 | `script/test_all.sh:111; script/test_q3b2_heavy_garble_replay.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `q1_pointwise_invariance` | 2 | `script/test_all.sh:113; script/test_q1_pointwise_invariance.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `mixed_abbrev_replay` | 2 | `script/test_all.sh:114; script/test_mixed_abbrev_replay.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `idiom_replay` | 2 | `script/test_all.sh:115; script/test_idiom_replay.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `lexicon_l1_replay` | 2 | `script/test_all.sh:116; script/test_lexicon_l1_replay.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `project_terms_replay` | 2 | `script/test_all.sh:117; script/test_project_terms_replay.sh:6` — 需 macOS bundle/harness；引用语料未导出，来源未核实，不自动判为私有。 |
| `runtime_storage_fallback` | 2 | `script/test_all.sh:118; script/test_runtime_storage_fallback.sh:5-16,24-32` — 合成主存储与回退存储；需要构建后的 macOS bundle 和 Swift harness。 |
| `userdb_process_recovery` | 2 | `script/test_all.sh:119; script/test_userdb_process_recovery.sh:64` — --development-fixture 使用合成进程与 userdb 数据；需要构建后的 macOS bundle。 |
| `userdb_word_cleanup` | 2 | `script/test_all.sh:123; script/test_userdb_word_cleanup.sh:5-27` — 合成临时 UserDB；Swift/release core；bootstrap 空缓存时还可能联网。 |
| `performance_controller_path` | 2 | `script/test_all.sh:124; script/test_performance_controller_path.sh:19,29` — 合成 base-500 与 macOS harness；少量冷启动取样可能越线。 |
| `install_input_method` | 2 | `script/test_all.sh:125; script/test_install_input_method.sh:7` — 安装器测试使用合成目标目录与构建后的 macOS bundle；运行测试不代表授权安装。 |
