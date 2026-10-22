# 安全政策 · Security Policy

[中文](#中文) · [English](#english)

---

## 中文

### 支持的版本 / Supported versions

| 版本 | 是否支持 |
|---|---|
| 最新发布版 | 支持 |
| 更早版本 | 不支持，请先升级再报告 |

### 如何报告漏洞

- **请通过 GitHub 私密漏洞报告（private vulnerability reporting）提交**：入口在仓库的 Security 标签 → Report a vulnerability。报告内容只有维护者可见。
- **请不要用公开 issue、讨论或 PR 报告安全漏洞**，也请不要在修复前公开细节；请给我们合理的时间处理。

### 范围

**接受报告：**

- 输入法应用与核心（native/core、macOS/Windows 宿主、管理台）中的安全问题；
- 破坏「核心零联网」边界的问题：输入法核心设计为完全在本地处理按键，不发起网络请求，该边界由 `script/check_core_network_boundary.sh` 在回归中检查；
- 本地存储、日志或导出内容中的隐私泄露（例如把用户输入发送到不该去的地方、明文留存敏感内容）；
- 提权、任意代码执行、供应链破坏等常规安全问题。

**不接受：**

- 第三方依赖（librime、Tauri、Rust crate 等）本身的漏洞——请直接向上游报告；若依赖漏洞影响本项目，欢迎告知我们受影响范围；
- 公开构建中不存在的功能（例如测试上传通道不在公开构建里）；
- 普通功能 bug（请走 issue）；
- 没有新影响的已知问题、纯理论问题。

### 响应

- 尽力而为（best effort）：我们会在看到报告后尽快确认收到，但不承诺具体响应或修复时限。
- 请在报告中附上：版本号与构建号（输入法菜单版本行两者都显示）、平台与系统版本、复现步骤、影响说明。
- 本项目目前没有漏洞赏金计划。

---

## English

### Supported versions

| Version | Supported |
|---|---|
| Latest release | Yes |
| Older versions | No — please upgrade first |

### Reporting a vulnerability

- **Please report via GitHub private vulnerability reporting**: the repository's Security tab → Report a vulnerability. Reports are visible to maintainers only.
- **Please do not use public issues, discussions, or PRs for security reports**, and do not disclose details publicly before a fix; give us reasonable time to respond.

### Scope

**In scope:**

- Security issues in the input-method app and core (native/core, the macOS/Windows hosts, the management app);
- Anything breaking the "core has no network" boundary: the input-method core processes keystrokes entirely locally and makes no network requests; the boundary is enforced by `script/check_core_network_boundary.sh` in regression;
- Privacy leaks in local storage, logs, or exported content (e.g. user input ending up where it should not, sensitive content stored in cleartext);
- Privilege escalation, arbitrary code execution, supply-chain compromise, and other conventional issues.

**Out of scope:**

- Vulnerabilities in third-party dependencies themselves (librime, Tauri, Rust crates, …) — report those upstream; you are welcome to tell us about the impact on this project;
- Functionality absent from public builds (e.g. the test-upload channel is not part of public builds);
- Ordinary functional bugs (use issues);
- Known issues with no new impact, or purely theoretical findings.

### Response

- Best effort: we will acknowledge reports when we see them, but we do not commit to specific response or fix timelines.
- Please include: the version and build number (both shown on the input-method menu's version line), platform and OS version, reproduction steps, and impact.
- There is currently no bug-bounty program.
