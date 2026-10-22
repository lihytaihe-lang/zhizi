# 贡献指南 · Contributing Guide

[中文](#中文) · [English](#english)

欢迎以 issue 和 pull request 的方式参与。本文分中文、英文两部分，内容一致。

---

## 中文

### 提 issue

- 先搜一下已有 issue，避免重复。
- 报 bug 请附上：
  - 版本号与构建号（输入法菜单里的版本行两者都显示）；
  - 平台（macOS / Windows，及系统版本）；
  - 复现步骤：按了哪些键（拼音串）、期望上屏什么、实际上屏什么。
- 功能建议请说明使用场景，不只说「希望有某功能」。

### 提 pull request

- 改动小而聚焦，一个 PR 只做一件事；较大的功能先开 issue 讨论。
- 每个改动都带上测试（测试放哪参考下面的测试命令与仓库里同类测试）。
- 不要手改派生/冻结文件：`native/resources/Rime/`、`quality/corpus/`、`quality/golden/` 都由生成脚本产出。改了源头请用 `scripts/tools/regen_derived_chain.sh` 重新生成派生链。
- 不要提交二进制或大文件。

### 贡献的授权

- **代码**：你提交到本仓库的代码按 Apache-2.0 授权（inbound=outbound：贡献随项目以同一许可分发，无需另行签署协议）。提交即表示你有权作出该授权。
- **词库数据**：词条投稿按 **CC0 1.0**（公有领域贡献）授权：在法律允许的范围内，你放弃对所投词条的著作权与邻接权，本项目可将其并入按 CC BY-SA 4.0 发布的词库，且无需为你署名；若你希望保留权利，请不要提交。词库整体仍按 CC BY-SA 4.0 授权——你放弃的只是自己对所投词条的权利。单个词条通常不构成有创造性的作品，按 CC0 提交便于词库长期维护与将来的重新授权。
- **皮肤**：皮肤与皮肤素材单独授权，经独立的皮肤模块分发；本仓库不接受皮肤的外部 PR，想法请开 issue。
- 提交即声明：内容为你本人创作或你有权提交的第三方内容，不含他人未公开的私密信息（也不要提交你自己的个人输入内容）。本项目没有 CLA。

### 提交信息风格

看 `git log`，一句话英文，格式 `type(scope): what and why`，type 用小写。常用 type：

- `feat`（新功能）、`fix`（修复）、`docs`（文档）、`test`（测试）
- `tools`（脚本工具）、`release`（发版）、`lexicon`（词库）、`derived`（派生链再生成）

scope 一般是组件，如 `macos`、`windows`、`core`、`skins`、`licenses`、`model`。示例：

```text
feat(windows): loader DLL + current.txt so an upgrade takes effect without a restart
fix(core): a two-letter prefix may complete to only one ASCII term, shortest reading first (the packs made 'so' show five brand names and no Chinese)
test(core): editing keys commit and pass; digits 6-0 do nothing when the second layer is off
docs: fix a typo in the build instructions
```

### 跑测试

各组件的单元测试（命令与 `script/test_all.sh` 及 `native/` 下的 Cargo / Swift 配置一致）：

| 组件 | 命令 |
|---|---|
| Rust 核心 | `cargo test --locked --manifest-path native/core/Cargo.toml` |
| IPC | `cargo test --locked --manifest-path native/ipc/Cargo.toml` |
| 管理台（Tauri） | `cargo test --locked --manifest-path src-tauri/Cargo.toml` |
| 前端 | `npm test` |
| macOS（Swift） | 先 `./script/bootstrap_native.sh` 准备 `.build-deps`，再 `swift test --package-path native/macos -c debug --disable-automatic-resolution` |
| Windows（TSF 外壳） | `cargo test --release --locked --manifest-path native/windows/zhizi-tsf/Cargo.toml --lib` |

`script/test_all.sh` 是全量回归：构建、全部单元测试与冻结评测回放门，耗时很长。只在必要时（发版、核心改动后的确认）由维护者运行，普通 PR 不要求跑。

### 词库投稿

词库在独立仓库 `zhizi-lexicon`（与本仓库同一个 GitHub 账号）维护，词条投稿请到那个仓库提交，本仓库不接受词条 PR。投稿的词条按 **CC0 1.0**（公有领域贡献）授权，词库整体仍按 CC BY-SA 4.0 授权。

### 沟通

- GitHub Issues（首选，公开可查）。
- 安全问题请用 GitHub 私密漏洞报告（仓库 Security 页 → Report a vulnerability）提交，见 SECURITY.md。
- 对事不对人，保持友善。

---

## English

### Filing issues

- Search existing issues first to avoid duplicates.
- Bug reports should include:
  - the version and build number (both are shown on the version line of the input-method menu);
  - the platform (macOS / Windows, plus the OS version);
  - reproduction steps: the exact key sequence (pinyin string), what you expected, and what was actually committed.
- For feature requests, describe the use case, not just the feature.

### Pull requests

- Keep PRs small and focused — one thing per PR; open an issue first for larger features.
- Every change comes with a test (see the commands below and neighboring tests in the tree).
- Do not hand-edit derived/frozen files: `native/resources/Rime/`, `quality/corpus/`, and `quality/golden/` are generated. If you change a source, regenerate with `scripts/tools/regen_derived_chain.sh`.
- Do not commit binaries or large files.

### Licensing of contributions

- **Code**: contributions to this repository are licensed under Apache-2.0 (inbound=outbound; no separate agreement is required). By submitting, you warrant you have the right to do so.
- **Lexicon data**: entries submitted to the lexicon are dedicated to the public domain under **CC0 1.0**: to the extent permitted by law, you waive copyright and related rights in the entries you submit, and this project may fold them into the CC BY-SA 4.0 lexicon without attribution to you; do not submit if you prefer to retain rights. The lexicon as a whole remains CC BY-SA 4.0 — what you waive is only your own rights to the entries you submit. Individual entries usually do not constitute creative works, and CC0 keeps long-term maintenance and future relicensing of the lexicon simple.
- **Skins**: skins and skin assets are separately licensed and distributed via a standalone skin module; this repository does not accept external skin PRs — please open an issue instead.
- By submitting, you declare the content is yours to contribute (or third-party content you have the right to submit) and contains no one else's undisclosed private information — nor your own personal typing. There is no CLA.

### Commit-message style

Following `git log`: one English line in the form `type(scope): what and why`, with a lowercase type. Common types:

- `feat`, `fix`, `docs`, `test`
- `tools`, `release`, `lexicon`, `derived`

The scope is usually a component such as `macos`, `windows`, `core`, `skins`, `licenses`, or `model`. Examples:

```text
feat(windows): loader DLL + current.txt so an upgrade takes effect without a restart
fix(core): a two-letter prefix may complete to only one ASCII term, shortest reading first (the packs made 'so' show five brand names and no Chinese)
test(core): editing keys commit and pass; digits 6-0 do nothing when the second layer is off
docs: fix a typo in the build instructions
```

### Running tests

Per-component unit tests (commands mirror `script/test_all.sh` and the Cargo / Swift configurations under `native/`):

| Component | Command |
|---|---|
| Rust core | `cargo test --locked --manifest-path native/core/Cargo.toml` |
| IPC | `cargo test --locked --manifest-path native/ipc/Cargo.toml` |
| Management app (Tauri) | `cargo test --locked --manifest-path src-tauri/Cargo.toml` |
| Frontend | `npm test` |
| macOS (Swift) | run `./script/bootstrap_native.sh` to prepare `.build-deps` first, then `swift test --package-path native/macos -c debug --disable-automatic-resolution` |
| Windows (TSF shell) | `cargo test --release --locked --manifest-path native/windows/zhizi-tsf/Cargo.toml --lib` |

`script/test_all.sh` is the full regression: builds, all unit tests, and the frozen replay gates — it runs for a long time. It is run by maintainers only when needed (releases, confirmation after core changes); ordinary PRs are not expected to run it.

### Lexicon contributions

The lexicon is maintained in a separate repository, `zhizi-lexicon` (same GitHub account as this one); please submit lexicon entries there — this repository does not accept entry PRs. Entries you submit are dedicated to the public domain under **CC0 1.0**, and the lexicon as a whole remains CC BY-SA 4.0.

### Communication

- GitHub Issues (preferred, publicly searchable).
- For security issues, use GitHub's private vulnerability reporting (the repository's Security tab → Report a vulnerability); see SECURITY.md.
- Be respectful; critique the code, not the person.
