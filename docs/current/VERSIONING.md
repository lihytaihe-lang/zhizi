# 版本号与文件命名（owner 2026-10-16 定）

owner：「版本命名以及文件命名都往上加版本号，0.1.0、0.1.1 以此类推，不要一直都是 0.1.0，这样对大家没有直观感受。」

## 规则
- **只有一套版本号，所有交出去的构建共用**：装给 owner 自己的、打给朋友的，都从同一个序列往上数（owner 10-16：「给我更新了 0.1.2，下次给朋友们打包就有可能是 0.1.3，我们都用同一套版本命名体系，不乱就行」）。**每个新构建（不同的提交）用新的版本号**；同一个构建既装给 owner 又打成朋友包，两处用同一个号。
- 流程：要装机或打包的新构建开始前，先运行 `script/bump_version.sh`（加一）→ 在 `CHANGELOG.md` 写这一版的更新 → 提交 → 再构建、回归、装机/打包。
- **发布版本号**写在仓库根目录 `VERSION`（一行，如 `0.1.1`），形如 `主.次.修订`。
- **每发一次给别人的包就加一**：朋友测试包、公开发布都算。小改动（修 bug、换词库/模型）加修订位（0.1.1 → 0.1.2）；一批新功能或体验大改加次版本位（0.1.x → 0.2.0）；第一个正式公开版是 1.0.0。
- **构建号**（CFBundleVersion，= 1000 + 提交数）仍是内部使用的，用来对应具体提交；显示时放在版本号后面：「知字 0.1.1 · 构建 1639」。
- 所有给人看的文件名都带发布版本号：`知字-0.1.1.dmg`、`知字-setup-0.1.1.exe`；包所在目录 `releases/<用途>-<日期>/`。
- 以前的包：`知字-1591.dmg` + `知字-setup-0.1.0-beta.1.exe`（10-14 第一版朋友包）算 **0.1.0**；10-16 第二版（Mac 构建 1639）是 **0.1.1**。

## 谁读 `VERSION`
- Mac 输入法：`script/build_native.sh` 写进 Info.plist 的 `ZhiziReleaseVersion`，输入法菜单末尾显示「知字 0.1.1 · 构建 1639」。
- Mac dmg 文件名：`script/package_friend_mac.sh`。
- Windows：`script/build_windows.ps1` 读 `VERSION`，传给 Inno Setup（`AppVersion`、输出文件名 `知字-setup-<版本>.exe`，「应用和功能」里的版本也是它）。

## 不动的
- 内部库 crate（`native/core`、`native/ipc`、`native/host`、Windows 的 `zhizi-tsf`/`zhizi-loader`、`zhizi-gram`）的 Cargo 版本是各自独立的，不跟 `VERSION`；Rime 的 `distribution_version`、schema/词典文件头的 `version`、IPC 握手串里的 `0.3.0` 是协议/格式标识，不是产品版本，不要改。

## 2026-10-21 统一到 VERSION（开源前）
- 以前 Mac 输入法和管理台 bundle 的 `CFBundleShortVersionString` 是更早的内部版本线 0.3.0，用户看到的却是 `VERSION`（0.1.x），两套并存。现在统一：`script/sync_version.sh` 把 `VERSION` 写进输入法 `InputMethod-Info.plist`、`package.json`、`package-lock.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`/`Cargo.lock` 和 Swift 常量 `ReleaseVersion.current`（测试 harness 报告的 `artifactVersion`）；`--check` 不一致就退出 1。
- `script/bump_version.sh` 改完 `VERSION` 会自动调用同步；`build_native.sh`、`build_manager.sh` 开头跑 `--check`，忘了同步会直接失败。
- 各回归/冻结/演练脚本里原先写死的 `"0.3.0"` 改为读 `VERSION`（`PRODUCT_VERSION`）。
- 装机脚本 `install_input_method.sh` 的 `validate_bundle` 同时接受当前 `VERSION` 和 `0.3.0`：本机已装的和历史备份包还是 0.3.0，回滚必须仍能通过校验。旧版本在没有这些包之后可以删掉这个兼容。
