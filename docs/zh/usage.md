[English](../en/usage.md) | [日本語](../ja/usage.md) | [简体中文](../zh/usage.md) | [首页](README.md)

<a id="webui"></a>
## WebUI

使用以下命令启动服务器渲染的浏览器界面：

```sh
cruise webui
```

此命令会在前台运行。若要改为管理后台实例，请使用：

```sh
cruise webui start --port 8484
cruise webui status
cruise webui status --json
cruise webui restart --port 8484
cruise webui stop
```

`start` 和 `restart` 接受下文所示的相同服务器选项；将这些选项放在子命令之后。若无法正常关闭，`stop --force` 允许强制终止。实例停止时，`status` 会以非零退出状态报告。停止 WebUI 并不表示要删除会话。生命周期命令管理的是本地实例。

除非另行指定，`restart` 会沿用之前的主机和端口。停止和重启会请求取消正在进行的 WebUI 操作。运行中的会话不会被删除，而是会被挂起，以便稍后恢复。

WebUI 实例记录（`$XDG_STATE_HOME/cruise/webui/instance.json`，通常位于 `~/.local/state/cruise/webui/`）包含控制凭据，因此应妥善保密。

默认 URL 为 `http://127.0.0.1:8484/`。使用以下选项自定义：

- `--host <ADDR>` -- 绑定的网络接口（默认值为 `127.0.0.1`；WebUI 不提供身份验证，因此不要将其绑定到不可信网络。）
- `--port <PORT>` -- 监听的 TCP 端口（默认值为 `8484`）。
- `-o`、`--open` -- 服务器启动后，在默认浏览器中打开 WebUI（默认不会打开浏览器；请自行访问打印出的 URL）。
- `--webui-dir <DIR>` -- 从磁盘提供 `templates/` 和 `static/`，用于开发。例如 `cruise webui --webui-dir webui`。
- `Ctrl-C` -- 停止服务器。正在进行的操作会被取消（会话变为 `Suspended`）。

Graph 选项卡会从 jsDelivr 加载 Mermaid，因此需要网络连接；其他所有 WebUI 选项卡均可离线使用。

在桌面端，通过侧边栏页脚选择显示语言；在移动端，通过页眉选择。选择后页面会重新加载。

<a id="usage"></a>
# 使用

```sh
# Create a session (plan -> approve)
cruise plan "implement the feature"

# Create a session and generate the plan in the background
cruise --plan "implement the feature"

# Interview-style planning: answer one question at a time, then the plan is written (SDK backend + TTY + `interactive_planning: true`)
cruise plan --grill "implement the feature"

# Enable only browser and app E2E for the built-in test-first workflow
cruise plan -c builtin:tdd -f browser_e2e,app_e2e "task"

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

# Open the full interactive keyboard client
cruise

# List and manage sessions with the CLI selector
cruise list

# Review a session's changes, or just their summary
cruise diff <SESSION_ID>
cruise diff <SESSION_ID> --stat

# Remove closed/merged PR sessions and terminal no-PR sessions
cruise clean

# Open the TUI in a remote project
cruise ssh devbox --cwd /srv/project

# Run remote commands and forward stdin or root options unchanged
cruise ssh devbox --cwd /srv/project -- plan "implement the feature"
echo "implement the feature" | cruise ssh devbox --cwd /srv/project -- plan
cruise ssh devbox -- list --json | jq .
cruise ssh devbox -- run
cruise ssh devbox -- --plan "implement the feature"
cruise ssh devbox -- --plan "implement the feature" --repo owner/repository

# Legacy: positional input without a subcommand is treated as `cruise plan`
cruise "implement the feature"
```

CLI 和 TUI 会针对工作流事件 **Action required**、**Plan ready**、**Completed** 和 **Failed**，尽力发送桌面、声音和 Slack webhook 通知。桌面通知需要 macOS 或 Linux 桌面会话；声音默认关闭。通知发送失败不会改变工作流结果或退出状态。设置 `CRUISE_DISABLE_NOTIFICATIONS=1` 可禁用这三个渠道。有关设置和 WebUI 行为，请参阅[通知](cli-reference.md#notifications)。

<a id="herdr-integration"></a>
## herdr 集成

在 [herdr](https://herdr.dev) 窗格内（已设置 `HERDR_ENV=1`、`HERDR_PANE_ID` 和 `HERDR_BIN_PATH`），TUI、`cruise plan`、`cruise run` 和 `cruise exec` 会报告其生命周期状态。状态报告尽力而为，不会影响工作流的成功、失败或退出状态。设置 `CRUISE_DISABLE_HERDR=1` 可将其禁用。
在 herdr 窗格内，使用 `cruise tell <session-id> "instruction"` 向正在运行的会话下达指令。

<a id="tui-interactive-keyboard-client"></a>
## TUI（交互式键盘客户端）

不带参数运行 `cruise` 会打开 Cruise 官方交互式键盘客户端：它是 CLI 和 WebUI 之外的第三种客户端。它保留现有 CLI 行为，同时将 WebUI 的会话管理工作流带到终端中。

```sh
# Open the interactive client
cruise
```

典型流程：运行 `cruise`，按 `n`，选择 Source、工作目录或仓库以及工作流配置，然后输入任务。按 `Ctrl-P` 进行普通规划，按 `Ctrl-G` 进行 grill 规划，按 `Ctrl-U` 直接将输入用作计划；也可以按 `Tab`，逐个回答剩余问题，并在最后选择启动模式。输入 Task 后，剩余问题依次为 Images、已声明的工作流标志（如有）、要跳过的步骤（禁用标志的步骤会隐藏）、Workspace（仅 Directory 会话）、适用时的 Dirty tree，以及 Launch。按 `1` 检查或操作会话，然后按 `3` 在 **Run All** 中运行已规划队列。在 Run All 中，按 `p` 编辑共享并行度，输入正整数，然后按 `Enter` 保存或按 `Esc` 取消。

TUI 需要在 macOS、Linux 或 Windows 上运行交互式 TTY。它不是自动化接口：自动化、JSON 输出和 CI 场景请使用 CLI。GitHub 工作流需要外部 [`gh` CLI](https://cli.github.com/)；TUI 不会捆绑或替代它。仅针对当前分支的工作不需要 `gh`（GitLab 仓库使用 `glab`）。

<a id="screens-and-workflows"></a>
### 界面和工作流

TUI 有三个视图：

- **Sessions** -- 浏览全局会话列表。选择会话后，可在其 **Info**、**Graph**、**Plan**、**Log** 或 **Diff** 详情选项卡中查看。[`cruise list` 操作](sessions.md#cruise-list-actions)矩阵介绍的是列表菜单，而不是完整的 TUI 操作集。它也支持 Ask 和 Option 提示、Clean、worktree/当前分支选择、Publish as Issue，以及 PR 链接。
- **New Session** -- 通过逐步对话创建会话或草稿：每次显示一个问题，上方列出已填写的答案，下方列出剩余问题。问题顺序为 Source、工作目录或仓库、工作流配置、任务、图像、工作流标志（如配置已声明）、跳过的步骤、工作区模式、允许脏工作树（仅当前分支运行），最后是启动模式。先前答案使某些问题不再适用时，会跳过这些问题。
- **Run All** -- 运行 Planned 或 Suspended 会话，并实时显示并行度、应用内状态和提示音。

PR 和 Issue URL 会以文本形式显示；成功执行 Publish as Issue 后，也会自动打开新 Issue 的 URL。其他 Markdown 链接仍以文本显示。仅 CLI 提供的 `config` 和 `exec` 操作可通过相应 CLI 命令使用，而不是通过 TUI 界面操作。

规划正在进行，或会话阶段为 **Awaiting Input**、**Awaiting Approval** 或 **Planned** 时，选中的会话会打开其 **Plan** 选项卡；其他情况下则打开 **Info**。`ask_user` 会暂停会话而不打开模态框：会话会标记为 **Awaiting Input**，问题只显示在该会话的 **Plan** 选项卡中。按 `o` 或在该会话上选择 **Answer Prompt**，即可打开 Plan 选项卡并聚焦详情窗格。按 `Enter` 开始编辑，再按一次 `Enter` 提交；按 `Esc` 离开编辑，同时保留草稿。取消运行会使会话变为 `Suspended`；取消规划会恢复之前的状态和计划。在 New Session 中，任务不能为空白，或至少需要一个图像附件；选择 GitHub 来源时必须填写仓库。

对于交互式规划问题，换行会在所选会话的 Plan 选项卡中按相邻行显示。回答字段内嵌于界面、属于特定会话，并非模态框。`Option` 请求仍使用模态队列及现有的 `o`/Run All 行为。

Sessions 侧边栏显示的是呈现状态；在规划、运行或等待用户操作时，它可能与持久化会话 **Phase** 不同。实心 `●` 表示 **Awaiting Input** 和 **Awaiting Approval**（浅蓝色）、**Running** 和 **Planning**（黄色）、**Planned**（蓝色）、**Completed**（绿色）、**Failed** 和 **Plan Failed**（红色），以及 **Suspended**（紫色）。草稿使用绿色 `◯`。即使设置了 `NO_COLOR`，状态文本仍会显示。

<a id="keyboard-map"></a>
### 键盘映射

TUI 仅使用键盘操作。按键固定，无法配置：

| 按键 | 操作 |
|-----|--------|
| `1` / `2` / `3` | 切换到 Sessions / New Session / Run All |
| `n` | 在 New Session 的第一个问题 Source 处打开；输入任务前先选择位置和配置 |
| `r` | 刷新 |
| `L` | 选择 UI 语言或 `auto`，并将其保存到应用配置 |
| `?` | 显示帮助 |
| `q` / `Ctrl-C` | 退出；若有活动任务，退出前会确认，然后取消工作 |
| `Ctrl-P` | 根据当前答案创建 New Session 并开始普通规划 |
| `Ctrl-G` | 根据当前答案创建 New Session 并开始 grill 规划 |
| `Ctrl-U` | 根据当前答案创建 New Session，并直接将输入用作计划 |
| `Ctrl-S` | 将 New Session 的答案保存为草稿 |
| `Tab` / `Shift-Tab` | 下一个 / 上一个问题（若存在匹配路径，Tab 会先完成路径，包括任意工作流配置路径）；在 Sessions 中，在侧边栏和详情窗格之间切换焦点 |
| 方向键 / `j` / `k` / `PgUp` / `PgDn` / `Home` / `End` | 浏览列表和选项。在对话中，`Up` / `Down` 用于选择工作流配置候选项、调用最近使用的目录或 `gh` 仓库，或浏览跳过步骤列表。在文本问题中，`Left` / `Right` / `Home` / `End` 用于编辑文本，`j` / `k` 会作为文本输入；`j` / `k` 用于浏览非文本选项。在 Sessions 中，聚焦侧边栏时，这些按键用于切换会话（`Right` 将焦点移至详情窗格）；聚焦详情窗格时，`Up` / `Down` / `PgUp` / `PgDn` / `Home` / `End` 用于滚动 Info、Plan 和 Log，或移动 Graph 节点选择；`Left` / `Right` 用于切换详情选项卡 |
| `[` / `]` | 在详情选项卡之间切换 |
| `a` | 打开操作面板 |
| `p` | 在 Run All 中编辑共享并行度；按 `Enter` 保存，按 `Esc` 取消 |
| `e` | 在 Run All 中编辑自动重新入队上限；按 `Enter` 保存，按 `Esc` 取消 |
| `o` | 在 Sessions 中，在 Plan 选项卡中打开所选会话的 Ask；其他情况下处理 Option 队列或打开专用 PR/Issue URL |
| `f` | 跟随日志 |
| `c` | 在 Sessions 中，确认是否 Clean 可回收会话和已关闭 PR 的会话 |
| `Ctrl-R` | 在多行 Edit Settings 对话框中，在仅保存和保存并重新生成之间切换 |
| `Enter` | 在 Plan Ask 中开始编辑或提交回答；其他情况下接受 New Session 答案并进入下一问题、启动会话或保存为草稿，或在任务/图像中插入换行 |
| `Ctrl-Enter` | 前进到当前 New Session 问题（任务和图像编辑器中必须使用此键，因为 `Enter` 会插入换行）；若在多行 Edit Settings 中，则保存（并在已选择时重新生成） |
| `Space` | 切换当前选项或高亮的跳过步骤 |
| `Esc` | 离开 Plan Ask 编辑而不丢弃草稿；其他情况下返回上一个问题，或在第一个问题处返回 Sessions |

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
