[English](../en/README.md) · [日本語](../ja/README.md) · [简体中文](../zh/README.md)

# cruise

<p align="center">
  <img src="../icon.png" alt="cruise logo" width="160" height="160">
</p>

Cruise 是一款基于 YAML 的 AI 工作流编排器。

Cruise 依据声明式工作流，半确定性地自动推进规划 → 审批 → 编写测试 → 实现 → 测试 → 审查 → 可选地创建 PR 并执行类似 PR 后处理的任务。它支持在步骤间传递变量、条件分支和循环控制。Cruise 基于 Jcode 构建，资源占用很低。它还支持 Claude 和 OpenCode SDK 后端，以及自定义 CLI 命令（例如 `codex exec`）。

## 文档索引

三个语言版本的主题文件名相同；请从下表打开相应主题。未来修改指南时，请同步更新英文、日文和简体中文版。

| 主题 | 说明 |
|---|---|
| [installation.md](installation.md) | 按操作系统安装、更新和托管 provider CLI |
| [usage.md](usage.md) | WebUI、CLI 示例、herdr、TUI 和 workflow 包 |
| [cli-reference.md](cli-reference.md) | 命令、参数、选项、语言设置和通知 |
| [sessions.md](sessions.md) | 生命周期、工作区模式、隔离、恢复和并行执行 |
| [configuration.md](configuration.md) | 配置解析、默认值、设置和 prompt 语言 |
| [sdk.md](sdk.md) | 后端、身份验证、权限和工具限制 |
| [steps.md](steps.md) | 步骤类型、PR 后自动化及变量 |
| [workflow-control.md](workflow-control.md) | 流程控制、flags、条件、分组和 workflow 组合 |
| [workflow-examples.md](workflow-examples.md) | 完整开发流程和简单的自动 commit 流程 |
| [github-actions.md](github-actions.md) | Action 设置、命令、provider、安全和故障排查 |
| [telemetry.md](telemetry.md) | OpenTelemetry 配置、导出内容和隐私说明 |

<a id="development-build"></a>
## 开发构建

要在本地测试 launcher，需要构建两个二进制文件。`cruise` launcher 会启动同目录下的 `cruise-runtime`，因此两者都不可缺少。

```sh
cargo build --bin cruise --bin cruise-runtime
```

<a id="support"></a>
## 支持

如果 Cruise 对你有帮助，欢迎考虑[赞助 smartcrabai](https://github.com/sponsors/smartcrabai)，以支持项目的开发和维护。
