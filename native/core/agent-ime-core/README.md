# 知序独立核心 0.3.1

Rust 原生拼音核心，提供 C ABI 和拥有会话/结果资源的 Rust 接口。可加载词库、处理组字和候选、提交选词、接入有界学习及外部评分/生成实现。核心没有 Tauri、AppKit、CoreML、librime 或网络依赖；本包不携带神经模型和平台输入法外壳。

组件版本为 **0.3.1**，现有 **ABI 6** 保持不变，与 Mac 应用版本分开。C 头文件已纠正键编号：永久屏蔽为18，不支持的键为19。请使用同包头文件与静态库，并核对 `aim_core_abi_version()`。

下面命令面向解包后的独立源码目录。原产品仓库使用 `--manifest-path native/core/agent-ime-core/Cargo.toml` 指定 crate。

## Rust 接用

验证工具链：Rust/Cargo 1.94.1。先准备独立、私有的测试目录：

```sh
mkdir -m 700 profile
cargo run --release --example zhixu -- "$PWD/dictionary" "$PWD/profile" nihao
```

示例打印真实核心候选并用空格提交。`examples/zhixu.rs` 演示 `Engine::open`、逐键输入、页面快照和提交。所有文字/候选都是拥有的 `String`，原生输出释放后仍有效。

```rust,no_run
use agent_ime_core::sdk::{Engine, KeyEvent, KeyKind, Security};
use std::path::Path;
let mut engine = Engine::open(Path::new("/absolute/dictionary"),
    Path::new("/absolute/private-profile"), "org.example.editor")?;
let page = engine.process(KeyEvent { kind: KeyKind::Text, text: "nihao",
    modifiers: 0, security: Security::NonSecure, delivery_id: 1 })?;
println!("{}", page.marked_text);
# Ok::<(), agent_ime_core::sdk::Error>(())
```

实际宿主必须提供真实安全状态。`Security::default()` 是 `Unknown`；未知或安全输入不进入普通组字/学习路径。交付事件编号由宿主管理；0表示未验证、不可据此持久学习，非零选择事件应跨会话唯一，重放同一事件复用其编号。示例只用于独立测试目录。

`Engine` 不能跨线程移动或共享。同一进程中活动的 Rust 会话必须使用同一词库/个人目录；最后一个会话释放后可切换。创建失败不会占用部署。使用原始 C API 的宿主也须遵守同一部署和非重入合同。`reload_dictionaries` 在组字中拒绝，并保留当前页面。

## C 接用

运行 `tools/build-c-demo.sh` 构建静态库与 `examples/c/zhixu_demo.c`。脚本从当前 Rust 编译器读取所需系统链接库。然后：

```sh
./target/sdk-demo/zhixu-demo "$PWD/dictionary" "$PWD/profile" nihao
```

C 输出不是 NUL 结尾字符串，按指针/字节长度读取；每次成功或失败的输出都用 `aim_result_free` 释放一次。`aim_core_version()` 是例外：返回静态 NUL 结尾版本串，只借用、不释放。完整接口见 [SDK-API.md](SDK-API.md) 和 [C 头文件](include/agent_ime_core.h)。

## 学习、词库与模型

```sh
mkdir -m 700 learning-profile
cargo run --release --example learning -- "$PWD/learning-profile"
```

此显式写入示例使用公开合成词，验证学习存储、删除/恢复、导出/合并与重复恢复幂等。输入法自动学习仍遵循核心原有验证/保留窗口，不由示例缩短。容量保持1MiB、词条和选择各4096条及45天失效；超限导入整体拒绝。导出不含服务密钥，但包含个人词和应用偏好，应作为个人数据保存。

发行包的 `dictionary` 提供按现有公开政策导出的词层及固定版本基础拼音表；专业词层空壳不包含原私有/分开发行的数据。二元模型与神经推理由宿主按各自许可另行提供，缺失可选模型时保留普通输入。原生核心的词库加载与页/选择行为可在公开测试中重现；这不等于 Mac 完整模型效果或 Windows/Ubuntu 系统输入法已验收。

评分、整句生成和后台搜索仍有完整 C API，可通过 `unsafe Engine::as_raw` 接用；对象所有权、形状、时限与结果过期规则见接口说明。Rust 入口只复制结果，不内置宿主的推理框架。

## 构建与验收

```sh
cargo build --release --locked
cargo test --locked
cargo doc --no-deps
```

独立发行包默认运行可公开重现的SDK测试；产品仓库中的完整内部语料/性能测试留在原产品测试流程，未冒称为发行包新执行。导出清单记录数据搬移、公开词层处理及内部测试配置的隔离。包内附源代码/数据SHA和许可证；发布说明区分本机实测与待验平台。
