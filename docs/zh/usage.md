[English](../en/usage.md) | [日本語](../ja/usage.md) | [简体中文](../zh/usage.md) | [首页](README.md)

<a id="webui"></a>
### WebUI

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

`start` 和 `restart` 接受下文所示的相同服务器选项；将这些选项放在子命令之后。若无法正常关闭，`stop --force` 允许强制终止。实例停止时，状态命令会以非零退出状态报告。停止 WebUI 并不表示要删除会话。生命周期命令管理的是本地实例，而非系统服务或自动启动监视器。

前台和后台服务器共用每个状态目录中的同一注册表，即使 `XDG_DATA_HOME` 不同也如此。请使用相同的 `XDG_STATE_HOME` 管理已启动的实例。重复执行 `start` 会报告正在运行的实例；若要指定不同的主机或端口，必须使用 `restart`。`status --json` 会报告 `running`、`stale`、`mode`、`origin`、`pid`、`version`、`started_at`、`log_path` 和 `outdated` 等字段。状态命令不会移除过期记录；`stop` 会移除过期记录，但不会向已被复用的 PID 发送信号。

除非另行指定，`restart` 会沿用之前的主机和端口。它会在调用命令时的当前工作目录中启动，不一定是原服务器所在的目录。若使用开发资源，请再次传入 `--webui-dir`；使用相对路径时，请从预期目录运行。停止和重启会请求取消正在进行的 WebUI 操作；运行中的会话会被挂起，以便稍后恢复，而不是删除。强制终止可能导致无法完成正常清理。

注册表、锁文件和后台日志位于 `$XDG_STATE_HOME/cruise/webui/`（通常为 `~/.local/state/cruise/webui/`）。实例记录包含控制凭据，应妥善保密。后台日志名为 `webui.log`；后续启动时，如果日志大于 5 MiB，会将其轮转为 `webui.log.1`。在 Windows 上，若未设置 `XDG_STATE_HOME`，默认状态目录为 `%LOCALAPPDATA%\cruise\state`。Unix 和 Windows 均支持分离启动、状态查询、停止和重启；在 Windows 上，若正常控制请求失败，可能需要使用 `stop --force`。

默认 URL 为 `http://127.0.0.1:8484/`。使用以下选项自定义：

- `--host <ADDR>` -- 绑定的网络接口（默认值为 `127.0.0.1`；WebUI 不提供身份验证，因此不要将其绑定到不可信网络。Merge PR 路由由同一个未认证服务器提供，其屏幕确认也不是访问控制：切勿将 WebUI 暴露给不可信网络。）
- `--port <PORT>` -- 监听的 TCP 端口（默认值为 `8484`）。
- `-o`、`--open` -- 服务器启动后，在默认浏览器中打开 WebUI（默认不会打开浏览器；请自行访问打印出的 URL）。
- `--webui-dir <DIR>` -- 从磁盘提供 `templates/` 和 `static/`，用于开发。例如 `cruise webui --webui-dir webui`；每次请求都会重新读取模板，因此可编辑并刷新而无需重启。
- `Ctrl-C` -- 停止服务器。正在进行的操作会被取消（会话变为 Suspended），服务器会在约 10 秒内退出；再次按 `Ctrl-C` 会立即退出。

Graph 选项卡会从 jsDelivr 加载 Mermaid，因此需要网络连接；其他所有 WebUI 选项卡均可离线使用。

在桌面端，通过侧边栏页脚选择显示语言；在移动端，通过页眉选择。选择后页面会重新加载，并保存到浏览器的 `cruise_ui_language` Cookie，期限约为一年，不会更改应用配置。有效 Cookie 的优先级高于服务器的 UI 语言；没有 Cookie 时，则使用服务器进程的默认值。

TSX 组件导入路径相对于包含该导入语句的文件。例如，`templates/components/session-row.tsx` 应使用 `./phase-badge` 导入同级的徽章，而不是 `./components/phase-badge`。

<a id="usage"></a>
## 使用

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

`ssh` 子命令保留了精确的旧版输入 `ssh`；若要规划文本内容恰好为 `ssh` 的任务，请使用 `cruise plan "ssh"`。隐藏入口 `mcp-bridge` 同样保留精确输入 `mcp-bridge`；若要规划该任务，请使用 `cruise plan "mcp-bridge"`。`tell` 子命令也被保留；若要规划文本恰好为 `tell` 的任务，请使用 `cruise plan "tell"`。

CLI 和 TUI 会针对工作流事件 **Action required**、**Plan ready**、**Completed** 和 **Failed**，尽力发送桌面、声音和 Slack webhook 通知。桌面通知需要 macOS 或 Linux 桌面会话；声音默认关闭。通知发送失败不会改变工作流结果或退出状态。设置 `CRUISE_DISABLE_NOTIFICATIONS=1` 可禁用这三个渠道。有关设置和 WebUI 行为，请参阅[通知](cli-reference.md#notifications)。

<a id="herdr-integration"></a>
### herdr 集成

在 [herdr](https://herdr.dev) 窗格内（已设置 `HERDR_ENV=1`、`HERDR_PANE_ID` 和 `HERDR_BIN_PATH`），TUI、`cruise plan`、`cruise run` 和 `cruise exec` 会通过 `"$HERDR_BIN_PATH" pane report-agent` 报告其生命周期状态，来源为 `custom:cruise`、代理为 `cruise`：规划或执行步骤时为 `working`，等待回答、选项选择或计划审核时为 `blocked`，其他情况下为 `idle`。退出时会释放控制权。状态报告尽力而为，绝不会改变工作流的成功、失败或退出状态。设置 `CRUISE_DISABLE_HERDR=1` 可将其禁用。
在 herdr 窗格内，使用 `cruise tell <session-id> "instruction"` 向正在运行的会话下达指令。

<a id="tui-interactive-keyboard-client"></a>
### TUI（交互式键盘客户端）

不带参数运行 `cruise` 会打开 cruise 官方交互式键盘客户端：它是 CLI 和 WebUI 之外的第三种客户端。它保留现有 CLI 行为，同时将 WebUI 的会话管理工作流带到终端中。

```sh
# Open the interactive client
cruise
```

典型流程：运行 `cruise`，按 `n`，选择 Source、工作目录或仓库以及工作流配置，然后输入任务。按 `Ctrl-P` 进行普通规划，按 `Ctrl-G` 进行 grill 规划，按 `Ctrl-U` 直接将输入用作计划；也可以按 `Tab`，逐个回答剩余问题，并在最后选择启动模式。输入 Task 后，剩余问题依次为 Images、已声明的工作流标志（如有）、要跳过的步骤（禁用标志的步骤会隐藏）、Workspace（仅 Directory 会话）、适用时的 Dirty tree，以及 Launch。按 `1` 检查或操作会话，然后按 `3` 在 **Run All** 中运行已规划队列。在 Run All 中，按 `p` 编辑共享并行度设置，输入正整数，然后按 `Enter` 保存或按 `Esc` 取消。

TUI 需要在 macOS、Linux 或 Windows 上运行交互式 TTY。它是交互式客户端，不是自动化接口：自动化、JSON 输出和 CI 场景请使用 CLI。GitHub 工作流使用外部 [`gh` CLI](https://cli.github.com/)；TUI 不会捆绑或替代它。仅针对当前分支的工作不需要 `gh`（GitLab 仓库使用 `glab`）。

<a id="screens-and-workflows"></a>
#### 界面和工作流

TUI 有三个视图：

- **Sessions** -- 浏览全局会话列表。选择会话后，可在其 **Info**、**Graph**、**Plan**、**Log** 或 **Diff** 详情选项卡中查看。[`cruise list` 操作](sessions.md#cruise-list-actions)矩阵介绍的是列表菜单，而不是完整的 TUI 操作集。Info 选项卡显示已声明的工作流标志及其开关状态。Graph 选项卡显示节点列表，以及所选节点的依赖项和边详情；禁用标志的步骤会隐藏，但旧版无版本图检查点除外，此类检查点不经过筛选而直接显示；Markdown 会被解析并设置样式。这里包括 Ask 和 Option 提示、Clean、worktree/当前分支选择、Publish as Issue，以及 PR 链接。
- **New Session** -- 通过逐步对话创建会话或草稿：每次显示一个问题，上方列出已填写的答案，下方列出剩余问题。问题顺序为 Source（本地 Directory 或 GitHub 仓库）、工作目录或仓库、工作流配置、任务、图像、工作流标志（仅当配置声明了标志时显示）、跳过的步骤（禁用标志的步骤会隐藏）、工作区模式、允许脏工作树（仅当前分支运行），最后是启动模式（普通规划、grill 规划、将输入用作计划或保存为草稿）。如果先前答案使某些问题不再适用，则会跳过这些问题。对于本地 Directory 会话，Workflow config 问题包含 Auto-detect，并按照已配置的分类和 Others 可见性，整理与 CLI 相同优先级的文件候选项和内置工作流。GitHub 会话会在克隆的仓库中解析 Auto-detect，因此不会包含调用方本地的候选项，但仍接受任意路径。可在任何问题处按 `Ctrl-P`、`Ctrl-G`、`Ctrl-U` 或 `Ctrl-S`，使用当前答案启动或保存会话。Directory 和路径答案支持补全；历史记录可用方向键调用；草稿和选择历史的保留方式见[新建会话表单持久化](sessions.md#new-session-form-persistence)。
- **Run All** -- 运行 Planned 或 Suspended 会话，并实时显示并行度、应用内状态和提示音。按 `p` 编辑 WebUI/TUI 共享的并行度设置。更改会保存到应用配置中，且可在批次运行期间进行；新限制会在下一个调度点生效，不会取消已在运行的工作进程。同一个 TUI 进程可并行运行不同会话；同一会话的重复工作会被拒绝。

PR 和 Issue URL 会以文本形式显示；成功执行 Publish as Issue 后，也会自动打开新 Issue 的 URL。专用 PR/Issue URL 操作会在 macOS 上通过 `open`、Windows 上通过 `explorer.exe`，或 Linux 上通过 `xdg-open` 打开链接；其他 Markdown 链接仍以文本显示。仅 CLI 提供的 `config` 和 `exec` 操作仍通过相应 CLI 命令使用，而不是通过 TUI 界面操作。

New Session 对话会在答案变更 500 ms 后自动保存。规划正在进行，或会话阶段为 **Awaiting Input**、**Awaiting Approval** 或 **Planned** 时，选中的会话会打开其 **Plan** 选项卡；其他情况下则打开 **Info**。其他屏幕状态都是临时的，但以下情况除外：共享 Run All 并行度设置会保存到应用配置；手动选择的详情选项卡会在 TUI 进程存续期间按会话保留，包括刷新和规划更新期间，并且会覆盖上述默认选项卡。`ask_user` 会暂停会话而不打开模态框：会话会标记为 **Awaiting Input**，问题只显示在该会话的 **Plan** 选项卡中。按 `o` 或在该会话上选择 **Answer Prompt**，即可打开 Plan 选项卡并聚焦详情窗格；按 `Enter` 开始编辑，再按一次 `Enter` 提交；按 `Esc` 离开编辑，同时保留草稿。执行期间的 Option 会继续排在现有模态队列中；单次运行的 Option 会自动打开，而 Run All 会显示队列标记。删除、Discard、Reset to Planned、Publish as Issue、Stage Local Merge、Merge Locally & Cleanup、Merge PR（预览模态框中可选择 Squash、Merge 或 Rebase 并按 Enter）、Clean、Run All / Cancel Run All，以及在有活动工作时退出，均会要求确认；Cancel、Approve、Run、Resume、Retry、Generate Plan 和 Open PR 会立即执行。取消运行会使会话变为 `Suspended`；取消规划会恢复之前的状态和计划。在 New Session 中，任务不能为空白，或至少需要一个图像附件；选择 GitHub 来源时必须填写仓库。工作目录留空表示 `.`，工作流配置留空表示自动检测。正常退出、panic、SIGTERM 和 SIGHUP 时都会恢复终端状态。会话错误会保留在应用和会话状态中；只有终端、根节点或事件循环故障才会导致 TUI 退出。

对于交互式规划问题，换行会在所选会话的 Plan 选项卡中按相邻行显示。回答字段内嵌于界面、属于特定会话，并非模态框。`Option` 请求仍使用模态队列及现有的 `o`/Run All 行为。

Sessions 侧边栏显示的是呈现状态；在规划、运行或等待用户操作时，它可能与持久化会话 **Phase** 不同。实心 `●` 表示 **Awaiting Input** 和 **Awaiting Approval**（浅蓝色）、**Running** 和 **Planning**（黄色）、**Planned**（蓝色）、**Completed**（绿色）、**Failed** 和 **Plan Failed**（红色），以及 **Suspended**（紫色）。草稿使用绿色 `◯`。即使设置了 `NO_COLOR`，状态文本仍会显示。

<a id="keyboard-map"></a>
#### 键盘映射

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
| `p` | 在 Run All 中编辑共享并行度设置；按 `Enter` 保存，按 `Esc` 取消 |
| `e` | 在 Run All 中编辑自动重新入队上限；按 `Enter` 保存，按 `Esc` 取消 |
| `o` | 在 Sessions 中，在 Plan 选项卡中打开所选会话的 Ask；其他情况下处理 Option 队列或打开专用 PR/Issue URL |
| `f` | 跟随日志 |
| `c` | 在 Sessions 中，确认是否 Clean 可回收会话和已关闭 PR 的会话 |
| `Ctrl-R` | 在多行 Edit Settings 对话框中，在仅保存和保存并重新生成之间切换 |
| `Enter` | 在 Plan Ask 中开始编辑或提交回答；其他情况下接受 New Session 答案并进入下一问题、启动或保存会话为草稿，或在任务/图像中插入换行 |
| `Ctrl-Enter` | 前进到当前 New Session 问题（任务和图像编辑器中必须使用此键，因为 `Enter` 会插入换行）；若在多行 Edit Settings 中，则保存（并在已选择时重新生成） |
| `Space` | 切换当前选项或高亮的跳过步骤 |
| `Esc` | 离开 Plan Ask 编辑而不丢弃草稿；其他情况下返回上一个问题，或在第一个问题处返回 Sessions |

<a id="layout-logs-and-process-behavior"></a>
#### 布局、日志和进程行为

- **至少 120 列**时，布局使用固定 **34 列侧边栏**和详情窗格。
- **80--119 列**时，Sessions 和 Run All 会将两个窗格垂直堆叠（侧边栏/摘要在详情/日志上方）。
- 小于 **80x24** 时，TUI 会显示调整大小提示。
- 在 Sessions 中，当前聚焦的窗格（侧边栏或详情）带有粗边框。
- 遵循 `NO_COLOR`；不会仅依靠颜色传达标签和状态。
- 空闲时的更新由事件驱动。外部状态每 3 秒轮询一次，活动工作使用 100 ms 微调指示器。
- 内存中的会话日志最多保留最近 10,000 行；Run All 视图最多保留最近 2,000 行。每个会话的完整运行输出仍位于 `$XDG_DATA_HOME/cruise/sessions/<session-id>/run.log`（默认是 `~/.local/share/cruise/sessions/<session-id>/run.log`）。
- TUI 无法将 stdin 交给交互式子命令：子进程不会与 TUI 共用 stdin，因此此类命令无法通过 TUI 提示输入。
- 不支持由不同 cruise 进程并发修改同一会话。在单个 TUI 进程内，不同会话可以并发运行；Run All 使用已配置的 run-all 并行度。
- 对于需要操作的提示、已就绪计划、已完成运行和失败，TUI 会异步发送已配置的桌面、声音和 webhook 通知，不会阻塞渲染或改变工作流状态。设置 `CRUISE_DISABLE_NOTIFICATIONS=1` 可禁用这三个渠道。

CLI 仍是自动化、JSON（`cruise list --json`）以及 CI/非交互式使用的标准客户端。现有命令工作流请参阅 [CLI 参考](cli-reference.md#cli-reference)。

<a id="workflow-packages"></a>
### 工作流包

`cruise workflow add <owner/repo[/path.yaml][@ref]>` 会从 GitHub 安装到 `$XDG_CONFIG_HOME/cruise/workflows/`（默认 `~/.config/cruise/workflows/`）。若未指定路径，根目录中的 `cruise-package.yaml` 会选择一个包；否则，单个工作流默认使用 `cruise.yaml`。显式指定 YAML 路径时会选择单个工作流，但以保留名称 `cruise-package.yaml` 或 `cruise-package.yml` 结尾的路径会被拒绝。若未指定 ref，则使用仓库默认分支，并固定到一个 commit。使用 `--name` 可覆盖包名称（`[A-Za-z0-9_-]+`）。

包清单会明确列出要安装的工作流，例如：

```yaml
description: Review and implementation workflows
workflows:
  - name: review
    path: workflows/review.yaml
  - name: implement
    path: workflows/implement.yaml
```

名称忽略大小写时也必须唯一，并且只能包含字母、数字、`_` 或 `-`。路径必须是唯一的、相对于仓库根目录的 `.yaml` 或 `.yml` 路径，不得包含空、`.` 或 `..` 路径部分，也不得包含反斜杠；路径不得以 `cruise-package.yaml` 或 `cruise-package.yml` 结尾。清单中未知的键会被拒绝。只会安装清单中列出的工作流，而不是仓库中的所有 YAML。嵌套的 `workflow_call` 步骤和 `prompt_file` 内容会在安装时内联，使 YAML 文件可独立使用。

单个工作流包保留 `workflows/<name>.yaml` 和 `<name>.cruise-package.json`。工作流包集合使用 `workflows/<name>/<workflow>.yaml` 和 `.cruise-package-lock.json`，后者会记录来源、ref、commit，以及每个已安装 YAML 的 SHA-256。包集合中的成员会在常规配置选择器中显示为 `<name>/<workflow>.yaml`。现有的单个工作流包不会迁移为包集合。

`add` 和 `update` 会预览来源、commit、工作流和命令，警告存在不可信命令，并询问 `[y/N]`。没有 TTY 时，需传入 `--yes`（仍会打印预览）。安装不会运行工作流。`update <name>` 会重新解析记录的 ref，但不会更改包类型；`remove <name>` 会移除包；`list` 会显示内置和已安装的包。包集合的更改会作为一个整体暂存。若 YAML 已编辑，或包集合包含额外文件，`update` 和 `remove` 会拒绝操作，以保护用户更改。

使用 `cruise workflow doctor <name>` 检查已安装的包，包括其 YAML 是否已修改。
