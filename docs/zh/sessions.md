[English](../en/sessions.md) | [日本語](../ja/sessions.md) | [简体中文](../zh/sessions.md) | [索引](README.md)

<a id="session-management"></a>
## 会话管理

Cruise 将会话数据存储在 `$XDG_DATA_HOME/cruise/sessions/`（默认值：`~/.local/share/cruise/sessions/`）。每个 `state.json` 都有一个带类型标记的 `config` 引用：

```json
{"config":{"kind":"file","path":"/absolute/path/cruise.yaml"}}
```

其他受支持的类型包括 `builtin_snapshot`（可选地带有内置目录项的 `name`，例如 `{"kind":"builtin_snapshot","name":"simple"}`；不含 `name` 的旧会话仍可加载）、`repo_snapshot`（带有相对于克隆目录的 `relative_path`）以及 `inline_snapshot`。`file` 引用是实时的：加载时或执行期间重新加载时会读取文件中的编辑内容。
使用 `--skip-planning`（将输入直接作为计划）创建的会话会保存 `"input": ""` 和 `"input_as_plan": true`；任务文本保存在 `plan.md` 中。
快照引用只读取会话自身的 `sessions/<session-id>/config.yaml`，绝不会重新发现或静默回退到其他配置。内置、内联和临时克隆工作流会以解析后的 YAML 序列化到该文件中，因此即使其源文件或克隆目录消失，仍可运行。包含 `config_source` 和 `config_path` 的旧版 `state.json` 文件不会迁移或加载；使用这种格式时请创建新会话。

<a id="runtime-file-layout"></a>
### 运行时文件布局

Cruise 遵循 [XDG 基本目录规范](https://specifications.freedesktop.org/basedir-spec/latest/)，并将运行时文件分散存储在以下目录中：

| 类型 | 路径 |
|------|------|
| 用户工作流 YAML 配置（`workflows/*.yaml` / `*.yml`） | `$XDG_CONFIG_HOME/cruise/workflows/`（默认值：`~/.config/cruise/workflows/`） |
| 用户工作流默认值 | `$XDG_CONFIG_HOME/cruise/default.yaml`（默认值：`~/.config/cruise/default.yaml`） |
| 项目工作流默认值 | `./.cruise/default.yaml` |
| 应用设置（`config.json` 或 `config.yaml`） | `$XDG_CONFIG_HOME/cruise/`（默认值：`~/.config/cruise/`） |
| 会话、工作树和临时 `--repo` 克隆 | `$XDG_DATA_HOME/cruise/`（默认值：`~/.local/share/cruise/`） |
| 状态文件（`history.json`、`new_session_draft.json`、每个安装的 `installations/<installation-id>/update-transaction.json`） | `$XDG_STATE_HOME/cruise/`（默认值：`~/.local/state/cruise/`） |
| WebUI 实例注册表、锁和守护进程日志 | `$XDG_STATE_HOME/cruise/webui/`（`instance.json`、`instance.lock`、`webui.log`，以及可选的 `webui.log.1`） |
| 使用记录 | `$XDG_STATE_HOME/cruise/usage.ndjson`（默认值：`~/.local/state/cruise/usage.ndjson`） |

在 Windows 上，如果未设置 `XDG_*` 变量，则默认值为 `%APPDATA%\cruise`（配置）、`%LOCALAPPDATA%\cruise\data`（数据）和 `%LOCALAPPDATA%\cruise\state`（状态）。如果这些变量不存在，则使用上文的 `~/` 默认值，并且 `~\` 会像 `~/` 一样展开。

> **从 `~/.cruise/` 迁移？** 早期版本将所有内容都存储在 `~/.cruise/` 下。请将 `*.yaml`/`*.yml` 移至 `~/.config/cruise/workflows/`，将 `config.json` 移至 `~/.config/cruise/`，将 `sessions/` 和 `worktrees/` 移至 `~/.local/share/cruise/`，并将 `history.json`/`new_session_draft.json` 移至 `~/.local/state/cruise/`。移动工作树目录时，请使用 `git worktree move`（或 `git worktree repair`）。
>
> **工作流配置以前放在 `~/.config/cruise/*.yaml` / `*.yml` 中？** 自动发现用户配置现在只会在 `workflows/` 子目录中查找工作流 YAML。请将它们移至 `~/.config/cruise/workflows/`；如果在 `~/.config/cruise/` 中直接发现遗留工作流 YAML 文件，cruise 会发出警告。应用设置 `config.yaml` 和工作流默认值 `default.yaml` 仍放在该目录中，不会触发警告。

<a id="session-lifecycle"></a>
### 会话生命周期

1. **`cruise plan "task"`** —— 在隔离的计划工作树中运行内置计划步骤以生成实施计划；当 stdin 是交互式 TTY 时，随后显示计划审批菜单；如果 stdin 不是 TTY，则自动批准计划。
2. **`cruise --plan "task"`** —— 立即创建会话，并在后台生成计划。之后可通过 `cruise list` 查看。
3. **`cruise draft "task"`** —— 记录任务为 `Draft` 会话，不运行计划步骤。准备好开始规划时，可从 `cruise list` 选择 **Generate Plan**。
4. **计划审批菜单** —— 可选择以下操作之一：
   - **Approve** —— 将会话标记为可运行。
   - **Fix** —— 提供反馈；计划步骤会根据你的输入重新运行。
   - **Ask** —— 提出问题；答案会被捕获但不会显示，然后重新显示菜单。
   - **Verify** —— 生成 Quint 和 Alloy 规范、运行检查，并显示报告及其解释，但不更改已保存的计划。使用 **Fix** 修改计划。
   - **Execute now** —— 跳过审批并立即运行。
   - **Publish as Issue** —— 将 `plan.md` 发布为 GitHub issue（或 GitLab issue），然后删除本地会话。

   在 TUI 中，**Fix Plan**、**Ask About Plan** 和 **Verify Spec**，以及在 WebUI 中的 **Fix**、**Ask** 和 **Verify**，只要计划可用，在 **Awaiting Approval** 或 **Planned** 状态下均可使用。Verify 是菜单操作，而不是 Ask 中的 `/verify` 命令。它需要 PATH 中有 `quint`（建议使用 0.33.0），并且 Alloy 需要 Java 17 或更高版本。如果提供了 `CRUISE_ALLOY_JAR`，Alloy 会使用该文件；否则 Cruise 会将 Alloy 6.2.0 下载到数据目录并校验其校验和。报告和日志会保留在会话的 `formal/<run-id>/` 目录中，直到会话被删除；它们不会放入仓库。每次 Quint 解析、类型检查和模拟都限时 60 秒。模型检查使用 `formal_verify.model_check_timeout_seconds`（默认值为 900 秒）。Alloy 命令会依次运行，因此 N 条命令最多可能耗时该限时的 N 倍，再加上设置和其他检查所需的时间。有限范围检查通过，并不代表超出其建模范围的内容也得到证明。

   选择 **Approve** 或 **Execute now** 后，如果未提供 `-f/--flag`，会先选择已声明的工作流标志。接着，**step skip selector** 只列出由这些值启用的步骤（分组步骤会以父项及其子项显示）；如果没有候选项，则不显示选择器。切换要在本次运行中跳过的步骤。只有可见候选项的选择才会按配置文件保存到 `$XDG_STATE_HOME/cruise/history.json`，并在下次使用同一配置创建会话时预先选中。取消任一选择器都会返回计划审批菜单，不保存未完成的标志更改，也不会批准或执行会话。Execute now 会将审批时的标志值作为显式赋值传递，因此执行开始时不会再次提示选择标志。选择 Approve 后，之后的 `cruise run` 会使用已保存的标志和 CLI 覆盖值，不会显示标志选择器，也不会再次询问要跳过哪些步骤（参见[工作流标志](cli-reference.md#cruise-run)）。

5. **`cruise run`** —— 未提供 ID 时，会选择待处理会话（`Planned`、`Running`、`Failed` 或 `Suspended`；符合条件的会话有多个时会提示选择），复用（或在 `$XDG_DATA_HOME/cruise/worktrees/<session-id>/` 下创建）git 工作树，并执行工作流。默认情况下，如果运行产生了分支提交，就会创建 PR，随后运行已配置的 `after-pr` 步骤。在内置工作流中，`-f after_pr` 会启用该前置流程但不合并；`-f merge` 会启用同一前置流程一次，并在最后执行原生同步 squash 合并。如果成功运行后分支没有新提交，则会跳过 PR 创建且不报错（已有分支提交仍符合条件），并说明跳过已选择启用的 after-PR 操作的原因。工作流级别的 `create_pr: false` 会跳过自动提交、推送、PR 创建、`after-pr` 和基于 PR 的清理，同时保留正常会话、工件和工作树；已选择启用的操作会附原因跳过，且 merge 绝不会覆盖不创建 PR 的设置。显式配置的 `if.no-file-changes` 失败/重试行为保持不变。仓库会话和本地工作树会话的发布失败都会进入 **PR Failed**；使用 **Create PR** 可在不重新运行工作流的情况下重试发布。

对于已确认的 GitHub 当前头冲突，只要验证提交和汇总结果后 CI 未报告失败，`wait-ci` 就会迅速退出。CI 失败时会保留其诊断和修复流程，包括等待仍在运行的 Actions 套件。过时的 PR 头仍会遵守传播宽限期。GitHub 内置 `wait-ci` 步骤会固定本地 `HEAD`。就绪条件要求 PR 头和 CI 汇总提交与该预期提交一致、存在 `statusCheckRollup.state` 且其值为 `SUCCESS`，并且该提交上的每个带有 `workflowRun` 的 GitHub Actions 检查套件都为 `COMPLETED`。已完成套件的结论若为失败，则 CI 判定失败；不带 `workflowRun` 的套件（例如来自第三方应用的套件）不会造成阻塞。就绪状态必须连续两次轮询都成立；任何未就绪的轮询都会重置连续计数。PR 头不匹配时会等待 90 秒宽限期，以便 API 数据传播；如果及时追上，等待可成功；如果在宽限期或整体期限过后仍不匹配，则会以现有的 `head_mismatch` 诊断失败。内置轮询间隔为 5 秒，最长等待 55 分钟（3300 秒），步骤超时为 60 分钟；尚未注册或仍待处理的检查不算成功。现有快照会话会继续使用其已保存的等待命令和超时，直到显式更新；更改内置配置不会改写它们。

共享的 after-PR CI 等待最长可达 60 分钟。如果截止时检查仍在运行且没有已知失败的汇总结果，则会报告待处理，不会将其视为代码失败，也不会启动修复；原生合并会独立拒绝未就绪的 CI。如果 GitHub 汇总结果已经失败，`wait-ci` 会继续轮询已关联的 Actions 套件，直到其完成或到达截止时间；到达截止时间时，会保留失败状态并使用可用诊断调用修复。

共享修复提示会收到 `wait-ci` 的 stderr，枚举当前头上所有非成功检查，并读取作业日志（包括已取消或超时的作业）。如果诊断为空或被截断，则会直接查询提供方。它会复现 CI 命令，在一次修复中解决所有范围内的问题，并在提交前运行相应的格式化工具、linter 和测试。无关或不稳定的作业会重跑一次，或清楚地报告；没有改动的修复不会循环重试。

原生 GitHub squash 合并采用相同的 CI 就绪门槛：PR 头和现有的汇总提交必须与预期的本地 `HEAD` 一致，汇总状态必须为 `SUCCESS`，并且该提交上每个带有 `workflowRun` 的 GitHub Actions 检查套件都必须为 `COMPLETED`；已完成套件的结论若为失败，则 CI 判定失败，而不带 `workflowRun` 的套件不会造成阻塞。就绪状态必须连续两次轮询都成立；任何未就绪的轮询都会重置连续计数。PR 头不匹配时会为 API 数据传播等待最多 90 秒；如果暂时不匹配的情况及时追上，操作可成功；如果不匹配持续超过宽限期或整体就绪期限，则操作失败。内置 `wait-ci` 会保留现有的 `head_mismatch` 诊断。合并还会等待 GitHub 的 `UNKNOWN` 可合并状态得到解析，每 2 秒轮询一次，最长 10 分钟（较短的步骤超时仍然适用）。此运行时检查可保护已有的工作流快照，不会改写它们。冲突、格式错误的响应以及不满足策略或审查要求的情况仍会拒绝合并；最终合并请求仍然只发送一次，并固定 SHA。

- 上一段中的冲突拒绝不包括已确认的 GitHub 落后/分叉祖先关系以及 `CONFLICTING`/`DIRTY` 可合并性；只有这些情况属于可重试的就绪结果。在合并时，GitHub compare API 会检查 PR 是否包含当前 base。内置 squash-merge 步骤会将这些情况路由到 `follow-up/sync-base`，接着依次经过 `sync-base` → `resolve-conflict` → `push-after-sync` → `wait-ci` → `fix-ci-error`，然后重试。只有从 `squash-merge` 到其 `if.fail` 目标的原生 `ResyncRequired` 路由会启动新的 after-PR 轮次：它会重置除 `squash-merge` → target 之外所有 after-PR 边的 `budgeted_traversals`，并清除组调用重试次数，同时保留边遍历历史。重新同步边仍受 `max_retries` 限制；通用 `if.fail` 仍共享预算。最坏情况下，最多会重新同步 `max_retries` 次，每轮允许最多 `max_retries - 1` 次 CI 修复迭代；重试预算为零时不能重新同步。其他就绪失败仍会导致失败；此祖先关系检查和恢复流程不会改变 GitLab 行为。

拒绝最终合并时，在确认 PR 仍处于预期头的打开状态后，只会记录固定原因以及可用的 HTTP 状态和命令退出码。已确认的合并及其他确认错误优先于拒绝诊断。

只有在固定提交的 CI 仍待处理或尚未注册/更新时，`MERGEABLE/UNSTABLE` 才是暂时状态；它绝不代表已满足合并条件。当前头的 CI 失败仍会立即拒绝合并；即使 CI 已通过，只要仍为 `UNSTABLE`，也会拒绝合并。

有 PR 的会话会保留在 `$XDG_DATA_HOME/cruise/sessions/` 中，直到 PR 关闭或合并；之后可由 `cruise clean` 删除。配置了 `create_pr: false` 的普通会话会保留其会话、工件和工作树，不必等待 PR。

没有 PR 的符合条件的本地工作树会话，可以暂存 squash 合并并保留会话；也可以显式合并并清理工作树、分支和会话。

<a id="model-and-fallback-notices"></a>
### 模型和回退通知

如果有可见的状态或信息通道，每次提示执行都会在该通道中报告 Cruise 选择的模型；每条通知也会写入会话的 `run.log`：

```text
Model: provider/model:free:xhigh
```

重试策略切换模型时，Cruise 会报告切换情况和替代模型：

```text
Warning: Fallback: provider/primary -> provider/fallback (503, attempt 1/5)
  Error: API Error: 503 service temporarily unavailable
Model: provider/fallback
```

括号中的详细信息说明了该模型被弃用的原因——如果失败文本中提到了提供方返回的 HTTP 状态（`503`、`429`、`400`、`404` 等），则显示该状态；否则显示失败类别（`rate limit`、`server error`、`client error`、`missing model`、`network error`，或后端在发送任何请求前就拒绝该引用时的 `unusable model`）。

原始后端错误消息会以 `Error:` 的形式跟在后面，保留其中的换行，即使替代模型最终成功也一样。

完整模型引用会保留，包括其提供方、路径和 effort 后缀。Claude 和外部命令后端仍可在没有 Cruise 模型的情况下运行，并会在通知中标明后端。如果 jcode 提示没有生效的 Cruise 模型，则会以设置错误停止，而不是将模型选择委托给 jcode，因此不会发出 `Model: default model (jcode)`。状态输出启用时，通知会发送到前台 CLI 的 stderr、TUI 现有的信息日志以及会话的 `run.log`。分离运行的 `cruise --plan` 工作进程和 CLI 交互式 `cruise run --all` 仪表板会抑制终端通知，因此必须从会话的 `run.log` 中读取这些通知。通知不会混入 stdout 或计划/Ask 响应内容。现有的会话生命周期和日志保留规则保持不变。特别是，临时 `cruise exec` 会话仍会在完成后删除。

> **`cruise exec`** 是独立的临时生命周期路径：它会在当前目录中执行，不进行规划、不创建工作树或 PR，并在终止状态完成后删除会话。暂停或中断的 exec 会话仍可通过 ID 恢复。`force_exec: true` 会为直接计划入口启用相同路径；使用 `--no-force-exec` 可单次退出此模式。参见 [`cruise exec`](cli-reference.md#cruise-exec)。

<a id="cruise-list-actions"></a>
### `cruise list` 操作

交互式会话列表会根据会话阶段显示相应的操作菜单：

| 阶段 | 可用操作 |
|-------|-------------------|
| **Draft** | Generate Plan, Delete, Back |
| **AwaitingInput** | Generate Plan, Delete, Back |
| **AwaitingApproval** | Approve, Publish as Issue, Edit Settings, Delete, Back |
| **Planned** | Run, Publish as Issue, Edit Settings, Replan, Delete, Back |
| **Running** | Resume, Tell*, Reset to Planned, Delete, Back |
| **Suspended** | Resume, Edit Settings, Reset to Planned, Delete, Back |
| **Failed** | Run, Edit Settings, Reset to Planned, Delete, Back |
| **PR Failed** | Create PR, Open PR*, Reset to Planned, Delete, Back |
| **Completed** | Open PR*, Merge PR*, Stage local merge**, Merge locally & cleanup**, Reset to Planned, Delete, Back |

`cruise list` 不提供 Verify，这与它目前不提供 Ask 的情况一致。

\* 仅当会话有 PR URL 时才会显示 Open PR 和 Merge PR。
只有当运行中会话的 runner 仍处于活动状态时，才会显示 Tell。

\** 本地合并操作要求会话是符合条件的本地工作树会话、没有 PR 且记录了目标分支。记录了合并意图或提交后，Stage 将不可用；没有记录目标分支的旧会话不符合条件。

只要会话有工作树分支，任何阶段还会额外提供 **View Diff**。分支可能已被删除，此时查看 diff 会报告错误。

当后台规划仍在运行时，`cruise list` 也可能显示 `Planning`；如果后台规划写入了持久化的 `plan_error`，则可能显示 `Plan Failed`。这些显示状态在底层都对应 `AwaitingApproval`：始终提供 `Edit Settings`、`Delete` 和 `Back`；只有当存在非空的 `plan.md` 且没有 `plan_error` 时，才会显示 `Approve` 和 `Publish as Issue`。

- **Generate Plan** —— 为 `Draft` 或 `AwaitingInput` 会话生成计划（将其转换为 `AwaitingApproval`）。
- **Approve** —— 批准计划，并将会话转换为 `Planned` 阶段。
- **Publish as Issue** —— 将未作更改的 `plan.md` 发布为解析后仓库中的 GitHub issue（或 GitLab issue），然后删除本地会话。系统会询问是否还要发布后续 `@cruise run` 评论，以便 `@cruise` GitHub Action 处理该 issue（默认情况下，`AwaitingApproval` 时关闭，`Planned` 时开启）。如果 issue 已创建但评论发布失败，会保留会话以便重试（复用现有 issue，不会重复创建）或手动评论（在 GitLab 上会跳过 `@cruise run` 评论提示）。
- **Run / Resume** —— 执行（或继续）会话。
  致命的 after-PR 失败会从其检查点步骤恢复，不会重新运行主工作流或再次发布 PR。已保存的标志会与显式 `--flag` 覆盖值一同生效。没有 after-PR 阶段标记的旧会话仍会按旧行为重新开始 after-PR 步骤。
- **Tell** —— 向运行中会话的下一个代理提示提交指令。TUI 和 WebUI 也会为正在运行的任务提供 Tell。
- **Create PR** —— 为 **PR Failed** 会话重试发布，而不重新运行工作流。Cruise 会提交剩余更改、推送，并复用该分支已有的 PR 或新建 PR。请先检查显示的错误和 forge 身份验证（对于 GitHub HTTPS 凭据，可使用 `gh auth setup-git` 配置 Git）。发布失败或取消时，会话会保持 **PR Failed**，并保留其工作树和任何现有 PR URL。致命的 after-PR 错误会使会话进入 **Failed**。成功时会完成会话；只有创建或复用了 PR 才会运行已配置的 after-PR 步骤。由于没有提交而跳过发布时，会话完成但不运行这些步骤。选择 Reset to Planned 则会重新启动工作流；选择 Delete 则会删除会话及其工作区。
- **View Diff** —— 查看会话分支的更改，然后返回菜单。有关比较和清理限制，请参见 [`cruise diff`](cli-reference.md#cruise-diff)。
- **Replan** —— 提供反馈以重新生成计划；会话仍处于 `Planned` 阶段。
- **Open PR** —— 通过 `gh pr view --web` 在浏览器中打开会话的 pull request。
- **Merge PR** —— 由用户发起合并会话的 pull request（TUI 和 WebUI 中也可使用）。Cruise 首先显示 PR 状态、可合并性、审查决定和每项 CI 检查。随后选择 `Squash`（默认值）、`Merge` 或 `Rebase` 并确认；确认提示会注明所选方法。Cruise 会运行 `gh pr merge <url> --squash|--merge|--rebase`，绝不会添加 `--auto`、`--admin` 或 `--delete-branch`。如果 GitHub 随后报告 PR 已关闭或已合并，则只会像 `cruise clean` 一样清理该会话及其工作树（和任何 `--repo` 克隆）。如果 PR 仍处于打开状态（例如正在合并队列中），则会保留会话，之后可由 `cruise clean` 删除。已经关闭或合并的 PR 不会再次合并：请运行 `cruise clean`。此路径仍是需要人工确认、仅适用于 GitHub 的方式；内置 `-f merge` 是另一条原生同步 squash 路径。关于检查和限制，请参见 [after-pr](../../skills/cruise-config/references/after-pr.md)。
- **Stage local merge** —— 将会话分支 squash 合并到基础检出目录的索引中，但不提交，也不删除会话。未提交的工作树更改会先提交到会话分支。
- **Merge locally & cleanup** —— squash 为一个基础提交，然后删除工作树、分支和会话。如果清理失败，会保留会话以便重试，而不会重复 squash。首次 squash 要求记录的目标分支当前在本地检出、基础工作区已跟踪文件干净且没有落后于上游；Cruise 不会 fetch 或 pull。重试清理时会检查记录的源提交是否可用、分支是否已更改，以及任何保留的工作树是否仍为预期检出且没有已跟踪或未跟踪的更改。冲突会报告，并尝试回滚；回滚失败也会报告；AI 不会解决冲突。这些操作不适用于 `--repo` 和 current-branch 会话，也不受 `cleanup_after_pr` 或 `merge` 标志影响。
- **Reset to Planned** —— 将会话重置回 `Planned` 阶段，清除当前步骤，以便从头重新运行。
- **Delete** —— 永久删除会话。
- **Back** —— 返回会话列表。

<a id="workspace-mode"></a>
## 工作区模式

从 `cruise plan` 或 `cruise "task"` 选择 Execute now 时，除非已提供 `-f/--flag`，否则会在审批菜单中先选择标志，再选择要跳过的步骤，并传递这些值而不在执行时再次提示。之后运行 `cruise run [id]`（包括从 `cruise list` 选择 Run）时，会使用已保存的标志以及任何 CLI 覆盖值，不显示标志选择器。例如，执行 `cruise plan -f merge "task"` 并选择 Approve 后，`cruise run <id>` 会使用已保存的 `merge` 值而不再提示。此外，在 TTY stdin 上，若声明了标志且未提供 `-f/--flag`，`cruise exec` 和 `force_exec` 入口会显示[工作流标志选择器](cli-reference.md#cruise-run)；dry run 和非 TTY 执行不会提示。

当 `cruise run` 开始新会话时，会提示你选择工作区模式：

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| 模式 | 说明 |
|------|-------------|
| **Worktree**（默认） | 在 `$XDG_DATA_HOME/cruise/worktrees/<session-id>/`（默认值：`~/.local/share/cruise/worktrees/<session-id>/`）创建隔离的 git 工作树。它会从 HEAD 或下文所述配置的 `base_branch` 检出新分支 `cruise/<session-id>-<sanitized-input>`。创建 PR 需要 `gh` CLI（GitLab 仓库使用 `glab`）。 |
| **Current branch** | 直接在当前仓库的活动分支上执行。不自动创建工作树或 PR，也不执行自动合并。如果选择启用了 `after_pr` 或 `merge`，则会说明原因并跳过不可用的 after-PR 操作。 |

使用 `--all` 时始终使用 worktree 模式；单次 `cruise run` 会保留会话中已记录的工作区模式；在非交互环境（stdin 通过管道传入）中，新会话默认采用 worktree 模式。通过 `--repo`（或 WebUI 仓库选择器）创建的会话始终固定为 Worktree 模式——会跳过提示，且不能使用 current-branch 模式，因为只有通过 PR 才能将工作从临时克隆目录带出去。

<a id="current-branch-mode-constraints"></a>
### 当前分支模式限制

- 对于新的 `cruise run` current-branch 会话，要求工作树干净（没有对已跟踪文件的未提交更改；忽略未跟踪文件）。`cruise exec` 和 `force_exec` 会话可以在工作区有更改时启动，但会发出警告；cruise 不会 stash、提交或重置现有更改。
- 必须关联到一个分支（不能处于 detached HEAD 状态）。
- 恢复会话时，活动分支必须与会话开始时记录的分支一致。

<a id="worktree-isolation"></a>
### 工作树隔离

- 对于有 PR 的会话，工作树会保留到 PR 关闭或合并；运行 `cruise clean` 可将其删除。没有 PR 的符合条件的会话可以改用本地合并清理。
- 在配置中设置 `cleanup_after_pr: true`（或运行时传入 `--cleanup-after-pr`），可在创建 PR 后立即自动删除本地工作树和分支。使用 `--no-cleanup-after-pr` 可覆盖配置并保留它们。
- 显式选择 **Merge locally & cleanup** 也会在 squash 提交后删除符合条件的本地工作树和分支，且不受 `cleanup_after_pr` 影响。

除非在用户或项目的 `default.yaml` 中设置了 `base_branch`，否则新工作树会从仓库当前的 HEAD 开始。有配置的 base 时，Cruise 优先使用本地分支，其次使用 `origin/<base>`；若设置了 `auto_fetch: true`，则只从 `origin` 获取该分支，并优先使用更新后的远程跟踪引用。获取失败会发出警告并回退到本地引用；找不到 base 会报错。配置的 base 会随会话保存，并用作 PR 或 MR 的目标；如果没有配置，forge CLI 会选择其默认目标。已有工作树会被复用，不会再次获取或解析 base。

<a id="copying-files-into-the-worktree"></a>
### 将文件复制到工作树

在仓库根目录创建 `.worktreeinclude` 文件，即可在工作流开始前将文件或目录复制到新工作树中：

```
# .worktreeinclude
.env
.cruise/
secrets/config.yaml
```

每行都是一个相对路径（文件或目录）。出于安全考虑，绝对路径和 `..` 路径遍历会被忽略。

<a id="stale-session-detection"></a>
## 过期会话检测

当 `cruise list`、TUI（`cruise`）或 WebUI 会话详情视图加载会话时，会检查处于 `Running` 阶段的会话是否仍然存活。如果 runner 进程（通过 PID 和启动时间识别）已不再运行，会话会自动转换为 `Suspended` 阶段。这样可防止进程崩溃或被强制终止后，会话无限期停留在 `Running` 状态。

可从 `cruise list` 或 TUI 恢复 `Suspended` 会话，也可以将其重置为 Planned。`run --all` 命令也会像处理 Planned 会话一样处理 Suspended 会话。

执行使用固定大小的有向 **Graph**，每个编译后的步骤对应一个稳定节点（并行块仍算一个单元）。增加 `max_retries` 不会扩大图。带版本的 `dag.json` 检查点是下一步骤、已接受的边遍历、预算计数的遍历、组调用重试次数、`{prev.*}` 变量和文件快照的权威记录。首次步骤开始前，以及接受转换、正常完成和循环保护后，都会以原子方式写入检查点。持久化失败会在下一个副作用发生前停止执行。步骤中途被中断时会保留步骤前的检查点，因此外部命令产生的效果不保证恰好执行一次。

正常恢复以及提示/模型重新加载都会保留计数器。`--max-retries` 会为已保存的计数设置新的上限，即使调低也一样。被拒绝的转换仍保持待处理状态，不能通过恢复补充其预算。更改恢复位置也会保留计数器。只有显式重启才会创建新的执行并归档之前的检查点。热重载期间移除当前步骤时，会保留旧工作流，而不会静默回到起点。

主工作流使用兼容文件名 `dag.json`；after-PR 使用独立的 `after-pr-dag.json` 检查点和计数器。检查点记录的 after-PR 失败可以恢复，而无需重新运行主工作流或再次发布。IPC 名称 `get_session_dag` 保持不变。无版本的旧版图可以查看，但无法在不猜测计数器的情况下恢复。缺少、损坏或版本未知的检查点会阻止恢复，但不会删除数据。请显式重启会话，以便从头运行。

<a id="parallel-session-execution"></a>
## 并行会话执行

Run All 可以在同一工作进程中从失败步骤开始，自动重试符合条件的失败。可在 WebUI 设置中设置非负上限，也可在 TUI Run All 视图中按 `e` 设置，或运行 `cruise config --set-auto-requeue-max-attempts <N>` 设置。默认值为 `0`，即禁用重试。未分类的命令错误、符合条件的提供方错误以及无文件更改错误可以重试；取消、配置、身份验证和授权错误不能重试。每次尝试仍会正常显示失败通知。包含失败步骤的已完成运行不会被重新归类为失败。

每个会话持久化的计数是跨批次的生命周期预算。手动 Run 或 Retry 不会重置该计数，但 Reset to Planned 会重置。批次开始时不会选取 Failed 会话；单会话运行和 exec 会话不会自动重试；降低上限不会清除计数。会话详情、CLI 列表 JSON 和批次摘要中都会显示计数。

WebUI、TUI 和 CLI 支持在 `run --all` 期间并发运行多个会话。

- **WebUI/TUI**：并行度由所选应用设置文件（`$XDG_CONFIG_HOME/cruise/` 下的 `config.json` 或 `config.yaml`）中的 `run_all_parallelism` 控制；可通过 `cruise config --set-parallelism <N>` 设置，默认值为 `1`。
- **CLI**：传入 `cruise run --all --parallelism <N>` 可指定本次运行的覆盖值（默认值为 `1`，即顺序运行）。不会读取或修改持久化的 WebUI/TUI 设置。在交互式终端中，如果未设置 `--dry-run`，CLI 还会显示实时仪表板，列出每个已调度会话的标题、当前步骤、状态和耗时；详细代理输出会保存在 `sessions/{id}/run.log` 中。非 TTY 和 dry run 调用仍使用常规日志输出并在最后显示摘要。

批次调度器：
- 从 Planned 和 Suspended 会话开始。
- 最多并发启动 `N` 个会话。
- 当仍有空闲工作槽时，每 200ms 重新扫描新加入的、符合条件的 Planned 或 Suspended 会话，因此批次运行期间创建的会话会自动加入。
- 无论完成顺序如何，都会按调度顺序返回结果。

<a id="new-session-form-persistence"></a>
## 新会话表单持久化

WebUI 和 TUI 会跨会话持久化两项状态：

- **Draft**（`$XDG_STATE_HOME/cruise/new_session_draft.json`）：New Session 表单当前的内容（任务描述、配置路径——包括内置 `tdd` 工作流的 `__builtin__` 哨兵值——、工作目录、仓库、跳过的步骤）。TUI 会自动保存更改。WebUI 会在点击 **Save draft** 时保存。重新打开表单时会恢复草稿，因此已保存的未发送输入会保留。
- **History**（`$XDG_STATE_HOME/cruise/history.json`）：过去 New Session 选择的记录。用于按配置文件将最近的选择预填到步骤跳过选择器中，并回想先前的工作目录/配置组合。
