# 知字输入法（zhizi）

[English](README.md) · 中文 · [官网](https://zhizi.dabblecove.com/)

知字输入法是一个简体中文全拼输入法，运行在 macOS（Apple Silicon）与 Windows 11（x64）。输入核心完全离线；内置的「顺手·知字」小型上下文模型随包在本机运行，按光标前的文字给候选重新排序。

当前版本 0.1.5，更新记录见 [CHANGELOG.md](CHANGELOG.md)。

## 功能

- 全拼输入，支持声母简拼与混拼；八组模糊音（zh/z、ch/c、sh/s、n/l、f/h、an/ang、en/eng、in/ing）可独立开关。
- 整句输入：长拼音串给出多种切法的整句候选，由模型按上下文挑选。
- 手误纠错：打错、漏打、对调、多打元音等常见手误可修正（Windows 上可在开始菜单开关）。
- 上下文排序：光标前的文字参与候选排序；Windows 按应用里的真实内容读取光标前文，代码类应用无前文时标点默认半角。
- 中英混输：拼音串下直接给出英文候选；Shift 单击切换中英文（Windows 弹「中/英」提示、任务栏模式跟随、按应用记忆）。
- `=` 键展开四行候选，方向键在格子里移动选择。
- 用户词学习（UserDB）；Ctrl+Backspace 删除高亮候选的学习记录，Ctrl+Shift+Backspace 拉黑该候选。
- 候选框皮肤：静态与动画皮肤（支持动作序列、探出框外的装饰）、皮肤管理窗口；皮肤格式见 `native/resources/Skins/skin.schema.json`。
- 词库分层：基础词库之上叠加日常新词层（按类别收录日常名称）、口语短语层与成语、IT、财经扩展层。专业词库包（诗词、CAD、工业、AI 与英文术语）不在本仓库内：它们在 zhizi-lexicon 词库仓库，下载与投稿词条都去那里。
- 离线运行：输入法核心与模型不发起网络请求（发布门里有网络边界自动检查 `script/check_core_network_boundary.sh`）。

## 平台

- macOS：Apple Silicon（arm64）。
- Windows：Windows 11 x64；32 位应用进程不支持。
- 已知限制：Windows 安装包未做代码签名，SmartScreen 或杀毒软件可能告警；UWP 等受限进程内的可用性未全面验证。
- Linux：未支持。核心与平台层在架构上是分离的，但尚无 Linux 适配。

## 构建

### macOS

```bash
./script/build_native.sh release
```

- 需要 Apple Silicon macOS、Xcode 命令行工具、Rust 与 Node.js（版本以仓库锁定为准）。
- 首次构建时 `script/bootstrap_native.sh` 会把 librime 1.16.1、rime-pinyin-simp 基础词库与 Rust 标准库许可文件下载并按 SHA-256 锁定到 `.build-deps/`（该目录不进 Git）；之后可离线构建。
- 上下文模型不在本仓库里。构建前可选择运行 `python3 scripts/context_model/fetch_public_model.py` 下载并编译模型（见[模型与词库](#模型与词库)）；不运行则得到无模型的构建，排序质量下降。
- 产物是 `build/native/release/Agent Input Method.inputmethod`。装进系统属于另外的发布验证步骤，编译通过本身不算一次发布。

### Windows

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File script\build_windows.ps1 -Package
```

- 需要 Windows 11 x64 与 MSVC 工具链；librime、ONNX Runtime、VC 运行库、Inno Setup 等依赖的准备方式与目录布局，见脚本本身。

## 模型与词库

- 模型：顺手·知字（shunshou-zhizi-30m），候选重排用的小型上下文模型，随输入法分发，Mac 经 Core ML、Windows 经 ONNX Runtime 在本机执行。模型卡（训练数据、评测、记忆抽取检查）与许可（CC BY-SA 4.0）见：<https://huggingface.co/DabbleCove/shunshou-zhizi-30m>
- 词库：对外发布的词库在独立的 zhizi-lexicon 仓库维护（与本仓库同一账号下）。本仓库内的 `native/resources/Rime/` 是随输入法打包的派生词库，由生成脚本产出，请勿手改。

### 从源码构建时如何拿到模型（macOS）

模型文件不进本仓库：权重有几十 MB，模型以独立许可单独发布，也有自己的发布节奏。仓库里只有 `native/resources/ContextModel/` 下的校验清单与模型卡副本。

- 构建前一条命令获取并编译：`python3 scripts/context_model/fetch_public_model.py`。脚本从 <https://huggingface.co/DabbleCove/shunshou-zhizi-30m> 的固定修订 `b74f57068819380bce70ab3bec22696a81a95c95` 下载（仅 HTTPS，重定向主机仅限 huggingface.co 及其 CDN），逐文件按 `native/resources/ContextModel/HF_SHA256SUMS` 校验，用 `xcrun coremlcompiler` 在本机编译 `.mlpackage`，并放入 `copy_bundle_model.py` 读取的目录布局（`ime_context_model/release/zhizi-30m-v3`，设置了 `$ZHIZI_WORK` 时为 `$ZHIZI_WORK/release/zhizi-30m-v3`）。若该目录已存在，脚本会拒绝运行，因此不会动到本地私有的模型目录。
- 不执行这一步，构建仍会成功，输入法照常可用——候选按词库排序，没有上下文重排。模型许可为 CC BY-SA 4.0，训练数据详情见 Hugging Face 页面上的模型卡。
- 运行时模型经 Core ML 在本机推理，不发起网络请求（由发布门 `script/check_core_network_boundary.sh` 保证）；下载只发生在构建期。管理台里对应的是实验性开关「上下文模型排序（实验）」，默认关闭。
- 可复现性：下载按修订与 SHA-256 钉死；编译产物会与 `native/resources/ContextModel/SHA256SUMS` 对照。在 Xcode 27.0 上，所有可复现产物均一致，仅两个例外并会明确警告：`zhizi30m_v2.mlmodelc/coremldata.bin` 内嵌编译器写入的转换时间戳与 UUID（不可能逐字节复现）。

## 目录结构

| 目录 | 内容 |
|---|---|
| `native/core/` | Rust 跨平台输入核心（状态机、候选、词库、模型接入） |
| `native/host/` | Rust 宿主与命令行工具（评测 harness、设置工具） |
| `native/ipc/` | 进程间通信组件 |
| `native/macos/` | macOS 输入法（Swift Package：InputMethodKit 宿主、候选窗、测试） |
| `native/windows/` | Windows TSF 外壳、加载器与 Inno Setup 安装器 |
| `native/resources/` | 随包资源：Rime 词库与皮肤（派生/冻结文件） |
| `script/` | bash 构建安装与回归入口（`build_native.sh`、`build_windows.ps1`、`test_all.sh`） |
| `scripts/` | Node/Python 生成器与校验器 |
| `src/`、`src-tauri/` | 管理台（Tauri 2 + React + TypeScript），提供皮肤管理等设置界面 |
| `quality/` | 词库来源登记（`corpus/`，冻结）与回归用评测集（`golden/`，冻结） |
| `docs/` | 项目文档（规格、任务卡、许可登记） |

## 许可

- 输入法代码：Apache-2.0，见 [LICENSE](LICENSE)。
- `native/resources/Rime/`、`native/resources/Lexicon/` 与 `quality/corpus/*provenance*.tsv` 中的词库快照数据：CC BY-SA 4.0，见 [LICENSE-LEXICON](LICENSE-LEXICON)；同时保留 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) 所列的上游许可与署名。
- `native/resources/Skins/` 中的免费皮肤素材：适用单独的 [LICENSE-SKINS](LICENSE-SKINS)；付费皮肤不在本仓库中。
- 第三方依赖仍按各自许可使用，见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
- 顺手·知字模型（shunshou-zhizi-30m）在 [Hugging Face](https://huggingface.co/DabbleCove/shunshou-zhizi-30m) 单独以 CC BY-SA 4.0 发布。

把词库打进应用时，词库部分仍须按其许可履行署名与相同方式共享义务，具体分发方式的合规请自行确认。

## 参与贡献

见 [CONTRIBUTING.md](docs/public/CONTRIBUTING.md)（issue / PR / 测试 / 词库投稿格式）。

## 联系

- 问题与建议：GitHub Issues
- 安全问题：用 GitHub 私密漏洞报告（仓库 Security 页 → Report a vulnerability）提交，见 SECURITY.md
