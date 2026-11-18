[English](../en/usage.md) | [日本語](../ja/usage.md) | [简体中文](../zh/usage.md) | [首页](README.md)

<a id="webui"></a>
## Daemon 与 WebUI

Cruise 会为每个用户和 state directory 运行一个常驻 daemon。session 命令是 daemon 的客户端，包括 `plan`、`--plan`、`draft`、`run`、`run --all`、`exec`、`list`、`tell`、`clean` 和 `diff`。如果 daemon 尚未运行，这些命令会按需自动启动它。`cruise webui` 也会确保 daemon 正在运行、打印 WebUI 登录 URL，并可选择打开浏览器：

```sh
cruise webui
cruise webui --open
```

使用 `cruise daemon` 管理 daemon：

```sh
cruise daemon start
cruise daemon status --json
cruise daemon restart
cruise daemon stop --force
cruise daemon serve
```

`serve` 在前台运行，适用于容器或调试。`start`、`restart` 和 `serve` 接受 `--host`、`--port`、`--webui-dir` 以及 `-o`/`--open`。默认地址为 `127.0.0.1:8484`。未请求端口且 8484 已被占用时，Cruise 会警告并使用临时端口；共享主机上请显式设置 `--port` 或 `CRUISE_DAEMON_PORT`。任一值设为 `0` 都会请求临时端口。`cruise webui` 打印的登录 URL 包含一个五分钟后过期、只能使用一次的 nonce，而不是 registry API token。登录后会设置 HttpOnly、SameSite=Strict 的 `cruise_webui` cookie，其中是独立于 API token 的随机内存 session ID，并会在 daemon 退出时失效。直接访问 origin 会显示 401。`-o`/`--open` 通过 0600 临时 HTML redirect 文件打开页面，因此 URL 不会作为参数传给 opener。loopback 绑定只接受 `localhost`、`127.0.0.1` 和 `[::1]` Host 名称（任意端口），可阻止 DNS rebinding 使用的外部 Host 名称并允许 SSH 隧道。非 loopback 绑定通过明文 HTTP 提供服务，只能在可信网络中或 TLS proxy 后使用。

daemon registry 和日志位于 `$XDG_STATE_HOME/cruise/daemon/`（通常为 `~/.local/state/cruise/daemon/`）。`instance.json` 的权限为 `0600`，其中包含随机 API token；daemon 日志写入 `daemon.log`。请妥善保管此目录。没有开机启动服务，也不会在空闲时自动退出；registry 记录被删除时 daemon 才会退出。旧的 `cruise webui start|stop|restart|status` 命令已移除，请改用 `cruise daemon ...`。
Detached daemon 每 60 秒检查一次 `daemon.log`。日志超过 5 MiB 时，会用当前日志替换 `daemon.log.1`，然后截断实时日志；仅保留一代轮转日志。

从 v0.3.0 分离式 WebUI 升级时，只要旧服务器仍在运行，daemon 的 start、restart、serve 以及客户端自动启动都会被拒绝。请先运行 `cruise daemon stop`；它也会停止旧服务器并取消正在执行的工作，使 session 进入 `Suspended`，之后可用 `cruise run <id>` 恢复。`cruise daemon status` 会报告运行中的旧服务器；启动 daemon 时会清理已停止进程的旧 registry。

此外，若某个 session 仍由另一个被记录为 runner 的 cruise 进程执行（例如升级时仍在进行的 v0.3.0 `cruise run`），daemon 会拒绝对它进行 run 和 planning（HTTP 409，CLI 退出码 1，不修改 state）。v0.3.0 的后台 planning worker 不记录 runner，无法检测，因此请等升级前启动的后台 plan 完成后再操作这些 session。

CLI 客户端负责解析 workflow 配置。它会随每个请求发送工作目录和 workflow override 环境变量（例如 `CRUISE_MODEL`、`CRUISE_PLAN_MODEL` 和 `CRUISE_SDK`），这些内容仅对该请求生效。`PATH`、`GH_TOKEN`、provider 凭据、`CRUISE_FORGE`、`CRUISE_SKIP_AGENT_AUTH_CHECK`、`CRUISE_DISABLE_NOTIFICATIONS`、`CRUISE_COMMIT_COAUTHOR_*` 等进程级变量来自 daemon 的环境；更改后请运行 `cruise daemon restart`。自动启动 daemon 时会从其环境中移除 workflow override 变量；显式运行 `cruise daemon start` 或 `serve` 会保留其环境。WebUI session 使用 daemon 的环境，并以用户 home directory 为工作目录。herdr 状态报告仍由客户端负责。

执行 `cruise update` 后，daemon 忙碌时会继续运行旧版本：CLI 客户端会拒绝请求，并提示等待空闲后运行 `cruise daemon restart`；重启前 WebUI 也会继续使用旧版本。空闲 daemon 会在下一个客户端请求时自动重启。

前台命令会从 daemon 接收并显示进度。Ctrl+C 会取消操作（`run` 变为 `Suspended`，规划状态会恢复），但关闭终端不会停止 daemon 正在执行的工作。可通过 `cruise run <session-id>` 或 WebUI 重新连接。Ask 和 Option 提示会显示在终端和 WebUI 中，先提交的答案生效。非交互式客户端遇到待回答提示时，会话会保持等待状态；请在 WebUI 中回答，或从终端运行 `cruise run <session-id>`。`cruise --plan` 的规划操作也在 daemon 内执行，不再使用分离的 worker process。

所有 session 修改都通过 daemon 进行，因此多个 CLI 进程和 WebUI 可以安全地同时操作 session。claim 可防止同一 session 被重复执行。本地命令包括 `config`、`usage`、`update`、`install`、`workflow *`、`docs`、`ssh` 和所有 `--dry-run`；它们不会连接本地 daemon。

Graph 选项卡会从 jsDelivr 加载 Mermaid，因此需要网络连接；其他所有 WebUI 选项卡均可离线使用。在桌面端，通过侧边栏页脚选择显示语言；在移动端，通过页眉选择。选择后页面会重新加载。

<a id="usage"></a>
# 使用

```sh
# Create a session (plan -> approve)
cruise plan "implement the feature"

# Create a session and generate the plan in the background
cruise --plan "implement the feature"

# Interview-style planning: answer one question at a time, then the plan is written (SDK backend + TTY + `interactive_planning: true`)
cruise plan --grill "implement the feature"

# Enable applicable UI E2E for the built-in test-first workflow
cruise plan -c builtin:tdd -f e2e "task"

# 显示通用 UI E2E policy 和 framework ID 列表
cruise docs e2e
# 显示 React Native／Expo 或 Tauri policy
cruise docs e2e react-native
cruise docs e2e tauri

# Plan against a GitHub repository instead of a local directory (temporary clone)
cruise plan --repo owner/repository "implement the feature"

# Background planning from stdin
echo "implement the feature" | cruise --plan stdin

# Save the task as a draft (no plan yet); generate the plan later from `cruise list`
cruise draft "implement the feature"

# Execute (or resume) a pending session -- Planned, Running, Failed, or Suspended
cruise run

# Execute a config directly in the current directory (no plan, no worktree, no PR)
cruise exec "do this"

# Run one agent without writing a workflow YAML
cruise exec --instant "do this"

# 在 TTY 中，裸运行 `cruise` 会打开交互式 session 选择器（与 `cruise list` 相同）
cruise

# List and manage sessions with the CLI selector
cruise list

# Review a session's changes, or just their summary
cruise diff <SESSION_ID>
cruise diff <SESSION_ID> --stat

# Remove closed/merged PR sessions and terminal no-PR sessions
cruise clean

# 未转发参数时，在远程项目中运行 `cruise list`
cruise ssh devbox --cwd /srv/project

# Run remote commands and forward stdin or root options unchanged
cruise ssh devbox --cwd /srv/project -- plan "implement the feature"
echo "implement the feature" | cruise ssh devbox --cwd /srv/project -- plan
cruise ssh devbox -- list --json | jq .
cruise ssh devbox -- run
cruise ssh devbox -- --plan "implement the feature"
cruise ssh devbox -- --plan "implement the feature" --repo owner/repository

# 位置参数恰好为 "docs" 时会与保留子命令冲突；请使用 cruise plan "docs"
# Legacy: positional input without a subcommand is treated as `cruise plan`
cruise "implement the feature"
```

无论由 CLI 还是 WebUI 启动工作，工作流事件 **Action required**、**Plan ready**、**Completed** 和 **Failed** 的所有通知都由 daemon 发送。声音和 Slack webhook 始终由 daemon 投递。连接着 WebUI 浏览器标签页时，桌面横幅以浏览器通知的形式显示；没有连接 WebUI 浏览器时，daemon 会显示原生桌面横幅（macOS/Linux 桌面会话）。声音默认关闭。通知发送失败不会改变工作流结果或退出状态。要禁用 daemon 的桌面通知、声音和 webhook 投递，请在 daemon 环境中设置 `CRUISE_DISABLE_NOTIFICATIONS=1`，并运行 `cruise daemon restart` 重启；这不会禁用 WebUI 浏览器通知。有关设置和 WebUI 行为，请参阅[通知](cli-reference.md#notifications)。

<a id="herdr-integration"></a>
## herdr 集成

在 [herdr](https://herdr.dev) 窗格内（已设置 `HERDR_ENV=1`、`HERDR_PANE_ID` 和 `HERDR_BIN_PATH`），`cruise plan`、`cruise run` 和 `cruise exec` 会报告其生命周期状态。状态报告尽力而为，不会影响工作流的成功、失败或退出状态。设置 `CRUISE_DISABLE_HERDR=1` 可将其禁用。
在 herdr 窗格内，使用 `cruise tell <session-id> "instruction"` 向正在运行的会话下达指令。

<a id="workflow-packages"></a>
## 工作流包

`cruise workflow add <owner/repo[/path.yaml][@ref]>` 会从 GitHub 安装到 `$XDG_CONFIG_HOME/cruise/workflows/`（默认 `~/.config/cruise/workflows/`）。若未指定路径且仓库根目录包含 `cruise-package.yaml`，则选择一个包；否则，单个工作流默认使用 `cruise.yaml`。显式指定 YAML 路径时会选择单个工作流。若未指定 ref，则使用仓库默认分支。使用 `--name` 可覆盖包名称（`[A-Za-z0-9_-]+`）。

包清单会明确列出要安装的工作流，例如：

```yaml
description: Review and implementation workflows
workflows:
  - name: review
    path: workflows/review.yaml
  - name: implement
    path: workflows/implement.yaml
```

名称忽略大小写时也必须唯一，并且只能包含字母、数字、`_` 或 `-`。路径必须是唯一的、相对于仓库根目录的 `.yaml` 或 `.yml` 路径。只会安装清单中列出的工作流，而不是仓库中的所有 YAML。

单个工作流包保留 `workflows/<name>.yaml` 和 `<name>.cruise-package.json`。包集合使用 `workflows/<name>/<workflow>.yaml`。包集合中的成员会在常规配置选择器中显示为 `<name>/<workflow>.yaml`。现有的单个工作流包不会迁移为包集合。

`add` 和 `update` 会预览来源、commit、工作流和命令，警告存在不可信命令，并询问 `[y/N]`。没有 TTY 时，需传入 `--yes`（仍会打印预览）。安装不会运行工作流。`update <name>` 会重新解析记录的 ref，但不会更改包类型；`remove <name>` 会移除包；`list` 会显示内置和已安装的包。包成员会作为一个整体暂存。`update` 和 `remove` 会拒绝操作已编辑的 YAML 和包含额外文件的包集合，以保护用户更改。

使用 `cruise workflow doctor <name>` 检查已安装的包，包括其 YAML 是否已修改。
