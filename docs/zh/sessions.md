<a id="sessions-and-workspaces"></a>
# 会话与工作区

[English](../en/sessions.md) | [日本語](../ja/sessions.md) | [简体中文](../zh/sessions.md) | [索引](README.md)

## 实验性 Manager 记录

`cruise manager` 需要本地 Git 仓库。记录保存在 `$XDG_DATA_HOME/cruise/manager/`（通常为 `~/.local/share/cruise/manager/`），以规范化的 Git 公共目录为键，因此关联工作树共享同一记录。运行 `cruise manager status` 时会显示已保存的消息以及任何提案或目标。对话使用从 `builtin:tdd` 默认配置解析出的交互式 SDK 和规划模型，不支持 command 后端。用户输入在调用提供商之前保存，如果调用未完成则保留为中断状态。SDK 被要求在临时目录中使用只读权限，但这不是严格的沙箱。

运行 `cruise manager` 或 `cruise manager status` 查看仓库快照；脚本可为 `status` 加上 `--json`。`cruise manager say [TEXT]` 省略文本参数时，从终端读取多行输入，或从管道读取标准输入。`say` 和 `propose` 默认等待完成；指定 `--no-wait` 会输出轮次 ID 并立即退出。按 Ctrl-C 只会停止等待，守护进程仍会继续处理该轮次。

`cruise manager propose` 请求经过验证的提案预览；目标分支必须能解析为本地 `origin` 跟踪引用，必要时请先 fetch。`cruise manager approve` 显示包含可执行检查的提案及其契约。在终端中未指定 `--yes` 时，必须准确输入 `approve` 来确认。非交互式批准必须同时指定 `--yes` 和 `--revision N`；指定的版本号必须与审查过的提案匹配。批准会为该仓库和目标分支注册一个持久化的活动 Goal，并授权守护进程协调器异步调度符合条件的子会话；审批操作本身不会同步启动执行。每个子会话在分派时固定到精确的已集成 Goal tip；成功结果按顺序集成，随后先验证合并后的精确 SHA，再由 Cruise 发布或恢复该 Goal 唯一的 PR/MR。Manager 不会合并；它会等待人工将经过验证的 head 合并到已批准的目标分支。`cruise manager pause GOAL` 和 `cruise manager resume GOAL` 记录目标状态，`cruise manager answer` 可回答匹配的活动子会话问题或选项提示；`cruise manager retry GOAL TASK` 在满足安全等条件时，可为活动目标中之前失败或暂停的子会话，或阻塞目标中已安全确认存在集成冲突的已完成子会话预留新的尝试。此外还需通过依赖关系、所有权、仓库等检查。`cruise manager resume GOAL TASK` 在原工作树中继续符合条件的子会话。`cruise manager retry GOAL` 可为所有任务均已集成且符合条件的阻塞目标重新运行验证，并保留先前证据；它不会重试结果不确定的发布。`cruise manager corrective GOAL TASK` 提示输入人工编写的指令，为符合条件且已确定失败的检查保存修正预览，此时不会授权执行。`cruise manager approve-corrective GOAL` 会刷新并显示目标和预览，并遵循与 `cruise manager approve` 相同的确认和版本规则。批准仅适用于该预览及集成后的 tip，会添加任务并保留先前的验证证据和已批准的检查。预览或 tip 发生变化时需要重新审查。执行需要 Git 2.38 或更高版本，以及经过身份验证的 GitHub 或 GitLab `origin`。

回答开放式问题时，使用 `cruise manager answer GOAL PROMPT TEXT`，或在终端中输入多行文本。对于选项提示，标准输入为终端时使用菜单。标准输入不是终端时，必须用 `--choice LABEL` 指定完全匹配的标签，并且仅在选项需要文本输入时提供 `--text TEXT`。纠正任务说明可通过 `--instructions TEXT`、终端提示或管道传入。

<a id="runtime-file-layout"></a>
## 运行时文件布局

Cruise 遵循 [XDG 基本目录规范](https://specifications.freedesktop.org/basedir-spec/latest/)，并将运行时文件分散存储在以下三个目录中：

| 类型 | 路径 |
|------|------|
| 用户工作流 YAML 配置（`workflows/*.yaml` / `*.yml`） | `$XDG_CONFIG_HOME/cruise/workflows/`（默认值：`~/.config/cruise/workflows/`） |
| 用户工作流默认值 | `$XDG_CONFIG_HOME/cruise/default.yaml`（默认值：`~/.config/cruise/default.yaml`） |
| 项目工作流默认值 | `./.cruise/default.yaml` |
| 状态文件、WebUI 实例信息等 | `$XDG_STATE_HOME/cruise/`（默认值：`~/.local/state/cruise/`） |
| daemon registry 和日志（`instance.json`、`daemon.log`） | `$XDG_STATE_HOME/cruise/daemon/`（默认值：`~/.local/state/cruise/daemon/`） |
| 会话、工作树和临时 `--repo` 克隆 | `$XDG_DATA_HOME/cruise/`（默认值：`~/.local/share/cruise/`） |

在 Windows 上，如果未设置 `XDG_*` 变量，则默认值为 `%APPDATA%\cruise`（配置）、`%LOCALAPPDATA%\cruise\data`（数据）和 `%LOCALAPPDATA%\cruise\state`（状态）。如果这些变量未设置，则使用上文的 `~/` 默认值，并且 `~\` 会像 `~/` 一样展开。

> **从 `~/.cruise/` 迁移？** 早期版本将所有内容都存储在 `~/.cruise/` 下。请将 `*.yaml` / `*.yml` 移至 `~/.config/cruise/workflows/`，将 `config.json` 移至 `~/.config/cruise/`，将 `sessions/` 和 `worktrees/` 移至 `~/.local/share/cruise/`，并将 `history.json` 和 `new_session_draft.json` 移至 `~/.local/state/cruise/`。移动工作树目录时，请使用 `git worktree move`（或 `git worktree repair`）。
>
> **工作流配置以前放在 `~/.config/cruise/*.yaml` / `*.yml` 中？** 自动发现用户配置现在只会在 `workflows/` 子目录中查找工作流 YAML 文件。请将它们移至 `~/.config/cruise/workflows/`；如果在 `~/.config/cruise/` 目录下直接发现遗留工作流 YAML 文件，cruise 会发出警告。应用设置 `config.yaml` 和工作流默认值 `default.yaml` 可以留在该目录中，不会触发警告。

<a id="session-lifecycle"></a>
## 会话生命周期

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

   只要计划可用，WebUI 的 **Fix**、**Ask** 和 **Verify** 就可在 **Awaiting Approval** 或 **Planned** 阶段使用。Verify 是菜单操作，不是 Ask 中的 `/verify` 命令。它需要 PATH 中有 `quint`，并且 Alloy 需要 Java 17 或更高版本。

   选择 **Approve** 或 **Execute now** 后，如果未提供 `-f/--flag`，会先选择已声明的工作流标志。接着，**step skip selector** 只列出由这些值启用的步骤。切换要在本次运行中跳过的步骤。选择会按配置文件保存，并在下次使用同一配置创建会话时预先选中。取消任一选择器都会返回计划审批菜单，不保存未完成的标志更改，也不会批准或执行会话。选择 Approve 后，之后的 `cruise run` 会使用已保存的标志和 CLI 覆盖值，不会显示标志选择器，也不会再次询问要跳过哪些步骤（参见[工作流标志](cli-reference.md#cruise-run)）。

5. **`cruise run`** —— 未提供 ID 时，会选择待处理会话（`Planned`、`Running`、`Failed` 或 `Suspended`；符合条件的会话有多个时会提示选择），复用（或在 `$XDG_DATA_HOME/cruise/worktrees/<session-id>/` 下创建）git 工作树，并执行工作流。默认情况下，如果运行后分支上仍有提交，就会创建 PR，随后运行已配置的 `after-pr` 步骤。在内置工作流中，只有 `tdd` 会创建 PR：`-f after_pr` 会启用该 after-PR 流程但不合并；`-f merge` 会运行一次相同流程，然后执行原生同步 squash 合并。`research` 也声明了这些标志，但不会创建 PR；`refactor`、`add-tests`、`add-formal-specs` 和 `security` 两者都不声明。工作流级别的 `create_pr: false` 会跳过自动提交、推送、PR 创建、`after-pr` 和基于 PR 的清理，同时保留普通会话、工件和工作树。如果发布失败，仓库会话和本地工作树会话都会进入 **PR Failed**。使用 **Create PR** 可在不重新运行工作流的情况下重试发布。

   有 PR 的会话会保留到 PR 关闭或合并，之后可由 `cruise clean` 删除。配置了 `create_pr: false` 的普通会话会保留其会话、工件和工作树，不必等待 PR。

   没有 PR 的符合条件的本地工作树会话，可以暂存 squash 合并并保留会话；也可以显式合并并清理工作树、分支和会话。

<a id="model-and-fallback-notices"></a>
## 模型和回退通知

如果有可见的状态或信息通道，每次提示执行都会在该通道中报告 Cruise 选择的模型。每条通知也会写入会话的 `run.log`。

```text
Model: provider/model:free:xhigh
```

重试策略切换模型时，Cruise 会报告切换情况和替代模型。

```text
Warning: Fallback: provider/primary -> provider/fallback (503, attempt 1/5)
  Error: API Error: 503 service temporarily unavailable
Model: provider/fallback
```

`cruise exec` 在当前目录执行，不进行规划、不创建工作树或 PR。成功后会删除会话；失败后会保留原 ID，供显式恢复或删除。暂停或中断的 exec 会话仍可按 ID 恢复。`--instant` 和 `force_exec` 也遵循此规则。请参阅 [exec 命令参考](cli-reference.md#cruise-exec)。使用 `--no-force-exec` 可单次退出 `force_exec` 模式。

失败的 exec 会话会显示在 `cruise list` 中，但不会出现在 WebUI 会话列表中，也不会被不带 ID 的 `cruise run` 或 `cruise run --all` 选中。菜单提供 **Resume**、**Delete** 和 **Back**，而不是普通 Failed 会话的操作。`cruise list --json` 会为这些条目新增 `"exec": true`，其 `"plan_available"` 为 `false`。普通条目的格式不变。

WebUI 的 Directory 来源提供 **Exec** 按钮，启用 `force_exec` 时则显示 **Execute**。如需规划，请选择 **Create draft**。Running 和 Suspended 状态的 exec 会话会显示在侧边栏中，Suspended 会话可以从那里恢复。

<a id="cruise-list-actions"></a>
## `cruise list` 操作

即使没有会话，交互式列表也会保持打开。以下操作无需先选择会话：

- **新建会话**使用当前目录及其工作流配置。选择 **交互式规划** 可输入任务并审阅生成的计划；选择 **后台规划** 会让守护进程生成计划，无需等待即可返回列表；选择 **保存草稿** 只会记录任务，不会启动规划。取消任务输入不会创建会话。即使配置启用了 `force_exec`，两种规划方式也不会自动执行工作流。
- **清理**遵循与 `cruise clean` 相同的清理规则，确认提示默认为否。操作后会刷新并返回列表，即使刚刚移除了最后一个可见会话。
- **批量运行**使用与 `cruise run --all` 相同的默认设置，执行符合条件的 Planned 和 Suspended 会话。如果已有批处理在运行，则会询问是否连接到该批处理。两种确认提示均默认为否。如果没有运行中的批处理，也没有符合条件的会话，则会显示提示并返回列表。
- 在主菜单中选择 **退出**，或按 Esc 或 Ctrl+C，即可退出。取消子菜单会返回主菜单。

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

对于 Manager 管理的子会话，列表仅提供 **View Diff**（如可用）和 **Back**。请使用 `cruise manager` 子命令操作这些子会话。

`cruise list` 不提供 Verify，这与它目前不提供 Ask 的情况一致。

* 仅当会话有 PR URL 时才会显示 Open PR 和 Merge PR。只有当运行中会话的 runner 仍处于活动状态时，才会显示 Tell。

** 本地合并操作要求会话是符合条件的本地工作树会话、没有 PR 且记录了目标分支。记录了合并意图或提交后，Stage 将不可用；没有记录目标分支的旧会话不符合条件。

只要会话有工作树分支，任何阶段还会额外提供 **View Diff**。分支可能已被删除，此时查看 diff 会报告错误。

- **Generate Plan** —— 为 `Draft` 或 `AwaitingInput` 会话生成计划（将其转换为 `AwaitingApproval`）。
- **Approve** —— 批准计划，并将会话转换为 `Planned` 阶段。
- **Publish as Issue** —— 将未作更改的 `plan.md` 发布为解析后仓库中的 GitHub issue（或 GitLab issue），然后删除本地会话。
- **Run / Resume** —— 执行（或继续）会话。
- **Tell** —— 向运行中会话的下一个代理提示提交指令。WebUI 也会在运行期间提供 Tell。
- **Create PR** —— 为 **PR Failed** 会话重试发布，而不重新运行工作流。请先检查显示的错误和 forge 身份验证（对于 GitHub HTTPS 凭据，可使用 `gh auth setup-git` 配置 Git）。发布失败或取消后，会话仍保持 **PR Failed**。致命的 after-PR 错误会使其进入 **Failed**。成功后会完成会话；只有创建或复用了 PR 才会运行已配置的 after-PR 步骤。
- **View Diff** —— 查看会话分支的更改，然后返回菜单。有关比较和清理限制，请参见 [`cruise diff`](cli-reference.md#cruise-diff)。
- **Replan** —— 提供反馈以重新生成计划；会话仍处于 `Planned` 阶段。
- **Open PR** —— 通过 `gh pr view --web` 在浏览器中打开会话的 pull request。
- **Merge PR** —— 由用户发起合并会话的 pull request（WebUI 中也可使用）。Cruise 首先显示 PR 状态、可合并性、审查决定和每项 CI 检查。随后选择 `Squash`（默认值）、`Merge` 或 `Rebase` 并确认。这是需要人工确认的 GitHub 专用方式。内置 `-f merge` 是另一条原生同步 squash 路径。
- **Stage local merge** —— 将会话分支 squash 合并到基础检出目录的索引中，但不提交，也不删除会话。未提交的工作树更改会先提交到会话分支。
- **Merge locally & cleanup** —— squash 为一个基础提交，然后删除工作树、分支和会话。首次 squash 要求记录的目标分支当前在本地检出、基础工作区已跟踪文件干净且没有落后于上游；Cruise 不会 fetch 或 pull。
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

- 对于新的 `cruise run` current-branch 会话，工作树必须干净（没有对已跟踪文件的未提交更改；忽略未跟踪文件）。`cruise exec` 和 `force_exec` 会话可以在工作区有更改时启动，但会发出警告；Cruise 不会 stash、提交或重置现有更改。
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

当 `cruise list` 或 WebUI 会话详情视图加载会话时，会检查处于 `Running` 阶段的会话是否仍然存活。如果 runner 进程已不再运行，会话会自动转换为 `Suspended` 阶段。这样可防止进程崩溃或被强制终止后，会话无限期停留在 `Running` 状态。

可从 `cruise list` 或 WebUI 恢复普通 `Suspended` 会话，也可以将其重置为 Planned。`run --all` 命令也会像处理 Planned 会话一样处理 Suspended 会话。

<a id="parallel-session-execution"></a>
如需恢复 Manager 管理的子会话，请使用上文所述的 `cruise manager` 子命令。

## 并行会话执行

Run All 可以在同一工作进程中从失败步骤开始，自动重试符合条件的失败。可在 WebUI 设置中设置非负上限，或运行 `cruise config --set-auto-requeue-max-attempts <N>`。默认值为 `0`，即禁用重试。未分类的命令错误、符合条件的提供方错误以及无文件更改错误可能会重试；取消、配置、身份验证和授权错误不会重试。每次尝试仍会正常显示失败通知。包含失败步骤的已完成运行不会被重新归类为失败。

每个会话持久化的计数是跨批次的生命周期预算。手动 Run 或 Retry 不会重置该计数，但 Reset to Planned 会重置。批次开始时不会选取 Failed 会话；单会话运行和 exec 会话不会自动重试；降低上限不会清除计数。会话详情、CLI 列表 JSON 和批次摘要中都会显示计数。

WebUI 和 CLI 支持在 `run --all` 期间并发运行多个会话。所有 session 修改都通过常驻 daemon 进行，因此多个 CLI 进程和 WebUI 可以同时操作 session；claim 可防止同一 session 被重复执行。

- **WebUI**：并行度由所选应用设置文件（`$XDG_CONFIG_HOME/cruise/` 下的 `config.json` 或 `config.yaml`）中的 `run_all_parallelism` 控制，可通过 `cruise config --set-parallelism <N>` 设置，默认值为 `1`。此设置仅适用于 WebUI Run All。
- **CLI**：传入 `cruise run --all --parallelism <N>` 可指定本次运行的覆盖值（默认值为 `1`，即顺序运行）。不会读取或修改持久化的 WebUI 设置。在交互式终端中，如果未设置 `--dry-run`，CLI 还会显示实时仪表板，列出每个已调度会话的标题、当前步骤、状态和耗时；详细代理输出会保存在 `sessions/{id}/run.log` 中。非 TTY 和 dry-run 调用仍使用常规日志输出并在最后显示摘要。

批次调度器：
- 从 Planned 和 Suspended 会话开始。
- 最多并发启动 `N` 个会话。

<a id="new-session-form-persistence"></a>
## 新会话表单持久化

WebUI 会跨会话持久化两项状态：

- **Draft**：新建会话表单当前的内容（任务描述、配置路径、工作目录、仓库和跳过的步骤）。WebUI 会在点击 **Save draft** 时保存。重新打开表单时会恢复草稿。
- **History**：过去新建会话选择的记录。用于按配置文件将最近的选择预填到步骤跳过选择器中，并回想先前的工作目录/配置组合。
