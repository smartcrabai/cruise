[English](../en/README.md) · [日本語](../ja/README.md) · [简体中文](../zh/README.md)

<a id="cruise"></a>
# cruise

<p align="center">
  <img src="../icon.png" alt="cruise logo" width="160" height="160">
</p>

Cruise 是一款以 jcode SDK 为主要且默认后端（`sdk: jcode`）的 YAML 驱动编码代理工作流编排工具。

Cruise 通过声明式工作流驱动编码代理：规划 → 审批 → 编写测试 → 实现 → 测试 → 审查 → 可选地创建 PR 并运行 PR 后自动化流程。它负责在步骤间传递变量、条件分支和循环控制。同时支持 Claude 和 OpenCode SDK 后端以及自定义 CLI 命令。

> **注意：** Cruise 可在 macOS、Linux 和 Windows（x86-64，`x86_64-pc-windows-msvc`）上运行。开发和测试主要在 macOS 上进行；Linux 和 Windows 上的验证相对较少。

## 文档索引

三个语言版本的主题文件名相同；请从下表打开相应主题。未来修改指南时，请同步更新英文、日文和简体中文版。

| 主题 | 说明 |
|---|---|
| [installation.md](installation.md) | 前置条件、安装、更新和托管 provider CLI |
| [usage.md](usage.md) | WebUI、使用示例、herdr、TUI 和 workflow 包 |
| [cli-reference.md](cli-reference.md) | CLI 命令、选项、会话管理及应用语言设置 |
| [sessions.md](sessions.md) | 会话生命周期、管理、工作区模式、隔离和并行执行 |
| [configuration.md](configuration.md) | Workflow 与应用配置、语言优先级和环境变量 |
| [sdk.md](sdk.md) | SDK 后端、认证、权限模式和工具限制 |
| [steps.md](steps.md) | 步骤类型、PR 后自动化及变量参考 |
| [workflow-control.md](workflow-control.md) | 控制流、分组和 workflow 组合 |
| [workflow-examples.md](workflow-examples.md) | 完整 workflow 示例 |
| [github-actions.md](github-actions.md) | GitHub Actions 集成、provider、认证、安全和故障排查 |
| [telemetry.md](telemetry.md) | OpenTelemetry traces、metrics、日志和隐私说明 |

<a id="development-build"></a>
## 开发构建

要在本地测试 launcher，需要构建两个二进制文件。`cruise` launcher 会启动同目录下的 `cruise-runtime`，因此两者都不可缺少：

```sh
cargo build --bin cruise --bin cruise-runtime
```

`cargo test --all-features --test workflow_defaults_cli` 还会在 Unix 上以 1 MiB 主线程栈预算检查 CLI 启动；Windows CI 会测试原生主线程限制。

<a id="support"></a>
## 支持

如果你觉得 cruise 有用，欢迎考虑[赞助 smartcrabai](https://github.com/sponsors/smartcrabai)，
以支持项目的开发和维护。

<a id="license"></a>
## 许可证

MIT
