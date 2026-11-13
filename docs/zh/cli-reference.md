[English](../en/cli-reference.md) | [日本語](../ja/cli-reference.md) | [简体中文](../zh/cli-reference.md) | [索引](README.md)

<a id="cli-reference"></a>
# CLI 命令参考

```
cruise [OPTIONS] [INPUT] [COMMAND]

Commands:
  plan         Create an implementation plan for a task
  draft        Save a task description as a draft without generating a plan
  run          Execute a planned session
  tell         Send an instruction to a running session
  list         List and manage sessions interactively
  clean        Remove sessions with closed/merged PRs
  usage        Summarize locally recorded token usage
  config       Show or update application-level configuration (`config.json` or `config.yaml`)
  exec         Execute a workflow or an instant task in the current directory (no plan, no worktree, no PR)
  ssh          Run a cruise command on a remote host through OpenSSH
  webui        Serve or manage the local browser UI (start, stop, restart, status)
  install      Install/update a provider CLI to its latest stable version or inspect managed status
  update       Update a managed native installation; `--status` lists recorded transactions read-only
  workflow     Manage workflows: list/eject built-ins, generate, interactively make or check YAML (`doctor`), install GitHub packages (`add`, `remove`, `update`)

Arguments:
  [INPUT]  Initial input (legacy: positional input without a subcommand uses `plan`)

Options:
      --plan <INPUT>           Create a plan in the background and return immediately (pass `stdin` to read piped stdin explicitly)
      --skip-planning          Use the input directly as the plan, skipping LLM-based plan generation
      --no-force-exec          Ignore `force_exec: true` in the workflow config and plan as usual
      --repo <OWNER/REPO>      Repository (owner/repository, group/sub/project, or full URL; GitHub or GitLab) to clone into a temporary directory for planning and execution
      --image <PATH>           Attach an image file (png/jpg/jpeg/webp/gif) to the planning input; can be repeated
      -f, --flag <NAME[,NAME...]> Enable named workflow flags; NAME=on|off|true|false also works (repeatable)
```

<a id="cruise-install"></a>
## `cruise install`

`cruise install <jcode|claude|opencode> [--force]` 会安装 provider 的最新稳定版 CLI（`claude` 使用 stable channel）。每次调用都会查询发布元数据，复用版本相同且状态正常的托管安装，并更新其他版本；`--force` 会重新下载所选的最新版本。`cruise install --status` 为只读、离线操作，仅报告记录的已安装版本，不声称它是最新版本。Provider CLI 按环境变量覆盖项、Cruise 托管安装、PATH 的顺序解析。身份验证由各 provider 管理。详情请参阅[托管的 provider CLI](installation.md#managed-provider-clis)。

<a id="cruise-workflow-generate"></a>
## `cruise workflow generate`

```
cruise workflow generate <DESCRIPTION> --name <NAME> [--user] [--config <PATH>]
```

根据描述草拟新的 workflow YAML。文件保存到 `./.cruise/<NAME>.yaml`；使用 `--user` 时则保存到用户 workflow 目录（`~/.config/cruise/workflows/`）。`NAME` 只能包含字母、数字、`-` 和 `_`。

- 通过验证的 YAML 会按返回内容原样保存。同名文件绝不会被覆盖；若文件已存在，则不会调用后端。
- 生成的 workflow 不会运行，也不会创建 session。
- 验证不能证明命令安全或 workflow 一定成功。所选 agent 后端仍保留其常规权限，因此生成过程中可能修改仓库。运行前请检查 YAML。

如需交互式起草和修改，请改用 `cruise workflow make`。

<a id="cruise-workflow-make"></a>
## `cruise workflow make`

```
cruise workflow make --name <NAME> [--user] [--config <PATH>] [--from <SELECTOR>] [DESCRIPTION]
```

通过对话创建单个 workflow YAML。stdin 和 stdout 都必须是终端；非交互式起草请使用 `workflow generate`。可选描述会作为第一条请求提交。`--config` 用于选择后端配置，`--from` 用于提供只读参考 workflow，例如 `builtin:tdd` 或文件路径。Cruise 读取参考文件但不会写入它，不过未沙箱化的命令后端可能会修改它。目标路径在启动时即确定：`./.cruise/<NAME>.yaml`，或者使用 `--user` 时的用户 workflow 目录。现有目标不会被覆盖，保留的项目名称 `default` 不能使用。

输入请求以创建或修改草稿。助手可以提问，但不会因此修改草稿。使用 `/show` 检查当前草稿，使用 `/accept` 选择 **写入 workflow**、**继续编辑**或**取消**。只有选择“写入”才会创建目标文件。workflow 不会运行，也不会创建 session。

每次请求都会验证候选项，并最多尝试修复三次。如果所有尝试都失败，之前的草稿会保持不变。验证不能保证命令安全，因此运行前请检查结果。

写入或取消后，草稿和 `conversation.jsonl` 会保留在 `$XDG_DATA_HOME/cruise/workflow-make/<id>/`（通常为 `~/.local/share/cruise/workflow-make/<id>/`）下。日志包含请求内容。`cruise clean` 不会删除这些目录；不再需要时请手动删除。

命令后端没有沙箱。如果后端可能改动了仓库，请检查仓库状态。

<a id="cruise-workflow-doctor"></a>
## `cruise workflow doctor`

```sh
cruise workflow doctor [--json] [--strict] [-f NAME[=on|off]]... [TARGET]...
cruise workflow doctor --all [--json] [--strict]
```

Doctor 对 workflow YAML 执行静态检查，不会执行它，也不会创建 session、worktree 或项目配置。未指定目标时，它会检查 `cruise.yaml`、`cruise.yml`、`.cruise.yaml`、`.cruise.yml` 以及 `.cruise/` 中的 YAML 文件，但排除 `.cruise/default.yaml`。`--all` 还会检查用户 workflows 和所有内置 workflow，且不能与目标或 `-f` 同时使用。

目标可以是路径、`builtin:<name>`、`__builtin__`（默认内置 workflow），或不带前缀的用户 workflow / 内置 workflow 名称。若某个不带前缀的名称同时匹配用户 workflow 和内置 workflow，则会产生歧义：请使用显式路径或 `builtin:<name>`。可重复指定目标以检查多个 workflow。可重复使用 `-f`，也可以用逗号分隔多个 assignment，为每个目标选择 flags，例如 `-f review,merge=off`。目标未声明的 flag 会导致该目标报错。未使用 `-f` 时，flags 均为关闭状态。

检查项包括本地 prompt 和 workflow 引用、配置、路由目标、重试预算、flag 名称以及图可达性。解析使用与执行相同的默认值和环境变量覆盖。GitHub workflow 引用会正常解析，可能需要网络访问。Doctor 不会测试后端、模型、身份验证、provider 连通性、命令安全性或依赖运行时的模板变量。

在所选 flags 下，不可达的 step 是警告，不一定是缺陷。如果从起点无法到达正常出口，则无法到达正常出口的可达环路属于错误。

人类可读报告写入 stdout。`--json` 输出单个对象，包含 `schema_version: 1`、`ok`、`exit_code`、`summary`（`checked`、`passed`、`failed`、`errors`、`warnings`）以及有序的 `targets`。每个 target 包含自身、类型（`project`、`user`、`builtin` 或 `file`）、package 状态、路径、状态（`ok`、`warning`、`error` 或 `unresolved`）、step 数量、重试上限、已声明 flags 和有序诊断。诊断包含 `level`、`code`、`stage`、可空的 `step` 和 `message`。自动化应使用 `code` 和 `step`，不要依赖 message 的措辞。

检查通过（允许警告）时退出状态为 `0`；存在错误，或使用 `--strict` 时存在警告，退出状态为 `1`；存在未解析目标或没有目标时退出状态为 `2`。状态 `2` 优先于 `1`；参数用法错误也以 `2` 退出。

<a id="cruise-ssh"></a>
## `cruise ssh`

```
cruise ssh [OPTIONS] <DESTINATION> [-- <CRUISE_ARGS>...]
```

`cruise ssh` 会通过 OpenSSH 在远程 `cruise` 上运行命令。未转发参数时，如果有 PTY，远程 `cruise` 会启动 TUI。目标可以是 OpenSSH 的 `[user@]host`，也可以是 `~/.ssh/config` 别名；端口、身份文件、agent 和 ProxyJump 请通过该配置指定。

选项：

```
      --cwd <PATH>                 Change to this remote directory before starting cruise
      --cruise-bin <PATH>          Remote cruise executable or path [default: cruise]
      --tty <auto|always|never>    PTY policy: follow local TTYs, force a PTY, or disable one [default: auto]
```

`--` 后的所有内容都会按相同顺序传给远程 cruise，并在远端解析。在 `auto` 模式下，本地 TTY 会请求 PTY；管道或重定向流不会请求，因此 `cruise ssh host -- list --json` 之类的命令适用于脚本。`--tty always` 和 `--tty never` 会覆盖该选择。

远程主机是执行边界：其 session、配置、jcode 身份验证、worktree 和仓库克隆均存储在远端的 XDG 目录下，不与本地 `cruise` 命令共享。`--cwd`、`--config`、`--image` 和其他路径参数都在远端主机上解释。诸如 `#12` 的 issue 输入使用远程主机上的 `gh` 及其身份验证解析。Cruise 不会上传本地文件、复制凭据或环境变量、安装远程二进制文件，也不会同步仓库。WebUI 仍是本地 session 客户端，不会浏览远程 session。

<a id="cruise-1"></a>
## `cruise`

```
cruise
```

不带参数运行 `cruise` 会打开仅支持键盘操作的 TUI。它要求在 macOS/Linux 上使用交互式 TTY；有关屏幕、workflow、按键映射、自适应布局和日志行为，请参阅 [TUI（交互式键盘客户端）](usage.md#tui-interactive-keyboard-client)。自动化、JSON 和 CI 场景请使用 CLI 子命令。

<a id="cruise-plan"></a>
## `cruise plan`

```
cruise plan [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description

Options:
      --issue <N>                  Use GitHub issue N as the task
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
      --dry-run                    Print the plan step without executing it
      --no-force-exec              Ignore force_exec: true and plan as usual
      --skip-planning              Use the input directly as the plan, skipping LLM-based plan generation
      --grill                      Interview-style planning: the agent asks one question at a time, then writes the plan (requires the SDK backend and a TTY; conflicts with --skip-planning)
      --no-interactive-planning    Disable interactive planning tools for this session; the agent writes plan.md directly (conflicts with --grill)
      --repo <OWNER/REPO>          Repository (owner/repository, group/sub/project, or full URL; GitHub or GitLab) to clone into a temporary directory for planning and execution
      --pr <NUMBER>                Address reviews on an existing GitHub pull request
      --rate-limit-retries <N>     Maximum number of retries per LLM call (SDK fallback policies also use it for retryable 4xx, 5xx, and network failures and fallback switching) [default: 5]
      --image <PATH>               Attach an image file to the planning input; can be repeated
      -f, --flag <NAME[,NAME...]> Enable named workflow flags; NAME=on|off|true|false also works (repeatable)
```

**GitHub issue 输入。** 可使用 `cruise plan '#12'`、`cruise plan --issue 12 'additional instructions'`，或将 `#12` 放在第一行、后续行写附加指令。第一行必须只包含 issue 引用。根命令的位置参数、`--plan`、`draft` 以及 TUI 和 WebUI 的 New Session 任务字段也接受这种语法。只有 `plan` 和 `draft` 接受 `--issue`，根命令和 `exec` 不接受。Issue 查询使用 `gh` 和当前仓库 origin；若提供 `--repo`，则使用其指定仓库。不支持 GitLab issue 输入。Issue 标题、正文、标签和评论会成为任务输入；受支持的 GitHub 图片会作为 session 附件保存，并排在用户提供的图片之前。Issue 编号会写入分支名称和 PR 的关闭引用。Issue 输入会退出 `force_exec` 模式。检索到的 issue 内容是不可信输入，可能包含 prompt injection。

非 git 目录会发出警告并在原位置规划，但 `cruise run` 仍需要 git 仓库。

指定 `--skip-planning` 时不会调用 LLM：输入会直接写入 `plan.md`。在交互式 TTY 上前台运行 `cruise plan --skip-planning` 仍会打开计划审批菜单；对于非 TTY stdin 和后台根命令 `cruise --plan --skip-planning` 路径，则会自动审批为 Planned。（如果 `force_exec: true`，除非使用 issue 输入或其他退出方式，否则 workflow 会直接执行；显式传入 `--no-force-exec` 可退出。）空输入或仅含空白的输入会被拒绝。若你已自行编写计划，只想让 Cruise 执行它，可使用此选项。

指定 `--grill` 后，SDK 规划会进入访谈模式：agent 一次提出一个问题，并在写入 `plan.md` 前推荐回答。它要求使用 SDK 后端（包括默认的 `jcode` 后端）、交互式终端以及 `interactive_planning: true`。它与 `--skip-planning` 冲突，且仅用于初始规划。

即使 workflow 配置中的 `interactive_planning: true`，传入 `--no-interactive-planning` 后，SDK 规划也会采用直接写入文件的流程。该 flag 与 `--grill` 冲突。

**内置 workflow flags。** 六个通用内置 workflow（`tdd`、`refactor`、`add-tests`、`add-formal-specs`、`research` 和 `security`）声明了 `after_pr` 和 `merge`；除非显式启用，否则所有 flags 都为关闭状态。`pr-review-fix` 不声明任何 flags。`tdd` 和 `refactor` 还声明了 `browser_e2e`、`app_e2e` 和 `formal_methods`。例如，`cruise plan -c builtin:tdd -f browser_e2e,app_e2e "task"` 仅启用两个 E2E flags；`cruise plan -c builtin:tdd -f merge "task"` 则选择执行 after-PR 自动化以及原生同步 squash merge。`-f browser_e2e,app_e2e,merge` 可组合这些选项。`merge` 会执行与 `after_pr` 相同的 after-PR 前置流程，且只执行一次；仅启用 `after_pr` 绝不会合并。

- `browser_e2e` 启用实际的浏览器操作、断言和失败修复。
- `app_e2e` 启用适用的移动端和桌面端原生应用操作，并使用仓库/平台原生 runner。
- `after_pr` 启用 after-PR 自动化，但不执行合并。
- `merge` 选择相同的 after-PR 前置流程，并在最后执行原生同步 squash merge。在 GitHub 上，合并前会自动同步并恢复落后于 base 或存在冲突的 PR。GitLab 不执行此恢复。它不会启用自动合并、绕过策略或使用合并队列。
- `formal_methods` 启用 workflow 级可执行 Quint、Alloy 和 Gherkin 规范及 CI；它不是初始计划模式。

`research` 仍为只读且 `create_pr: false`；`merge` 不会创建 PR，也不会覆盖该禁止创建 PR 的策略。如果因为未创建 PR 而无法执行已启用的 after-PR 操作（包括 `research`、`exec`、Current-branch 运行或没有提交的运行），Cruise 会跳过该操作并打印原因。

使用 `--repo <owner/repository>` 时，session 的目标会从当前目录切换为 GitHub 或 GitLab 仓库。Repo session 始终以 Worktree 模式运行，并且仍需要 `gh`/`glab` 访问仓库。若 `create_pr: false`，Cruise 会跳过自动提交、推送、PR 创建、`after-pr` 和基于 PR 的清理。成功运行若没有新的分支提交，会在不尝试创建 PR 的情况下完成；现有分支提交仍可用于创建 PR。Repo 或本地 worktree session 的发布失败都会进入 **PR Failed**，并保留 workspace 以便通过 **Create PR** 恢复，而不会重新运行 workflow。`--repo` 也适用于后台规划（`cruise --plan "task" --repo owner/repository`）。自由输入的 `owner/repository` 也受支持。

**修复现有 GitHub PR 的评审。** 在仓库中运行 `cruise plan --pr 12`，或运行 `cruise plan --repo owner/repo --pr 12 "focus on tests"`；如出现提示，先审批生成的计划，再运行 `cruise run <session-id>`。可选的位置文本是附加指令；此模式不会读取 stdin，也不会调用 LLM 生成计划。TUI 和 WebUI 的新建 session 表单也接受 GitHub 仓库的 PR 编号。此功能需要已通过身份验证的 `gh`。PR 必须处于开放状态，并且在同一仓库中具有非空 head 分支；支持 draft PR，但不支持 fork PR 和 GitLab merge request。`--grill` 和 `--image` 不能与 `--pr` 同时使用。

默认 workflow 是 `builtin:pr-review-fix`，而不是项目 workflow。自定义 `-c` workflow 必须启用 PR 处理，并包含一个设置了 `reviewers: all` 的 after-PR `github-review` step。内置 workflow 不声明 flags，不创建或合并 PR，仅处理评审。评审循环会提交并推送修复、回复或解决目标讨论串；失败的 session 可以重试。PR session 不提供“发布为 Issue”和“重新规划”。

<a id="cruise-draft"></a>
## `cruise draft`

```
cruise draft [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description (omit to prompt interactively; reads from stdin when piped)

Options:
      --issue <N>                  Use GitHub issue N as the task
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
```

将输入保存为 `Draft` session，不调用 LLM。稍后可在 `cruise list` 中选择 **Generate Plan** 来生成计划。适合立即记录想法、但暂时不想开始（或支付）规划费用的场景。

<a id="cruise-run"></a>
## `cruise run`

```
cruise run [OPTIONS] [SESSION]

Arguments:
  [SESSION]  Session ID to execute (if omitted, picks from pending sessions)

Options:
      --all                        Run all planned or suspended sessions (live dashboard on interactive terminals for non-dry runs)
      --parallelism <N>            Max number of sessions `--all` executes concurrently (no flag default; 1 when omitted, must be >= 1, requires `--all`)
      --auto-requeue-max-attempts <N>  Maximum automatic retries per session (requires `--all`)
      --max-retries <N>            Maximum number of times a budgeted graph transition may be traversed (no flag default; falls back to the workflow config's top-level `max_retries`, else 3)
      --rate-limit-retries <N>     Maximum number of retries per step (SDK fallback policies also use it for retryable 4xx, 5xx, and network failures and fallback switching) [default: 5]
      --dry-run                    Print the workflow flow without executing it
      --cleanup-after-pr           Delete local worktree and branch after PR creation
      --no-cleanup-after-pr        Keep local worktree and branch after PR creation
      -f, --flag <NAME[,NAME...]> Enable named workflow flags; NAME=on|off|true|false also works (repeatable; conflicts with --all)
```

**Workflow flags。** Workflow 配置可以声明 `flags:`，并通过 `flag: <name>` 将它们附加到 steps。只有有效值为 on 时，带 flag 的 step 才会运行；未设置的 flags 默认为 off。使用 `--flag NAME` 启用一个，使用 `--flag browser_e2e,app_e2e` 启用多个，也可以重复使用 `--flag`；`-f` 是 `--flag` 的别名。仍支持显式的 `NAME=on|off|true|false` assignment，因此 `--flag NAME=off` 可覆盖已保存的启用值。每次 `cruise run` 都会将保存的 flag 值与显式 CLI 覆盖合并，且不会显示 flag 选择器；这包括全新运行、恢复运行、`run --all` 和从 `cruise list` 选择 Run。未知名称会被拒绝，且 `--flag` 与 `--all` 冲突。若未提供 `-f/--flag`，Approve 和 Execute now 会先选择 flags，再选择要跳过的 steps。已保存的启用 flags 会预先选中；按 Space 切换，按 Enter 确认。若未选择任何项，所有已声明 flags 均为 off。

`--all` 默认按顺序运行所有 Planned 或 Suspended session。无论 session 最初以何种模式创建，都会强制使用 Worktree 模式。所有 session 完成后会打印汇总表，显示每个 session 的结果和 PR 链接。`--all` 和 `[SESSION]` 不能同时使用。

`--parallelism <N>` 是仅对当前调用生效的覆盖设置，用于在 `--all` 期间并发运行最多 `N` 个 session。省略时默认为 `1`（顺序运行），必须至少为 `1`，且只有指定 `--all` 时才可用。每个 session 都在自己的 worktree 中运行；一个 session 失败不会停止其他 worker；Ctrl+C 会暂停正在运行的 session，并停止安排新的 session。

`--auto-requeue-max-attempts <N>` 仅覆盖本批次的自动重新排队上限。默认值为 `0`（禁用）。

当 stderr 是交互式终端且未设置 `--dry-run` 时，`run --all` 会显示实时仪表板，列出每个已安排 session 的标题、当前 step、状态和已用时间。在 CI 等非 TTY 环境或执行 `--dry-run` 时，会保留常规日志输出和最终汇总。

<a id="cruise-exec"></a>
## `cruise exec`

```
cruise exec [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description bound to {input} (optional if your config doesn't reference {input})

Options:
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
      --instant                    Build a single-agent workflow without a workflow YAML (requires INPUT; conflicts with --config and --flag)
      --sdk <NAME>                 Instant backend: jcode, claude, or opencode (requires --instant)
      --model <MODEL>              Instant model (requires --instant)
      --review                     Add a bounded review loop (requires --instant)
      --review-rounds <N>          Maximum review passes, at least 1 (requires --review; default: 2)
      --verify <COMMAND>           Add a verification command (requires --instant; repeatable)
      --max-retries <N>            Maximum number of times a budgeted graph transition may be traversed (no flag default; falls back to the workflow config's top-level `max_retries`, else 3)
      --rate-limit-retries <N>     Maximum number of retries per step (SDK fallback policies also use it for retryable 4xx, 5xx, and network failures and fallback switching) [default: 5]
      --dry-run                    Print the workflow flow without executing it
  -f, --flag <NAME[,NAME...]>     Enable named workflow flags; NAME=on|off|true|false also works (repeatable; see Workflow flags)
```

直接在当前目录运行 workflow steps：不生成计划、不创建 git worktree，也不会自动创建 PR。因等待输入而暂停的 session（`Running`），或因 Ctrl+C 中断的 session（`Suspended`）会保留，并可通过 `cruise run <id>` 恢复；exec session 不会被 `cruise run` 自动选中，也不会纳入 `cruise run --all`。允许存在未提交的更改；Cruise 会在这些更改之上运行，不会 stash、提交或重置工作树，并会警告 workflow 生成的文件可能与现有更改混在一起。仍然必须处于一个已附加的分支上。

`--instant` 会在内存中组装一个包含单个 implement step 的 workflow，不解析 workflow YAML。未指定时，`cruise exec` 仍使用常规配置解析。没有配置文件时，交互式终端会在分类选择器中提供内置 workflow；非交互执行会自动选择 `tdd`。Instant 要求非空白任务。Workflow 默认值可以在开头插入 command pre-steps；workflow 默认值和环境变量覆盖仍然生效，且 `CRUISE_SDK` / `CRUISE_MODEL` 优先于 `--sdk` / `--model`。

```sh
cruise exec --instant --sdk jcode --model my-model --review --review-rounds 2 \
  --verify 'cargo test' --verify 'cargo clippy' "implement the feature"
cruise exec --instant --dry-run "implement the feature"
```

Review 会修复已确认的缺陷；当 review 轮次更改了文件时，会在指定轮数内重复执行（默认 2 轮）。某轮未产生更改时循环结束。验证命令按提供顺序在实现和 review 之后运行；命令失败会使运行失败，不会启动自动修复循环。`--max-retries`、`--rate-limit-retries` 和 `--dry-run` 也适用；若 `--max-retries` 预算小于 review 轮数，则无效。Instant 不声明 workflow flags，且不能与 `-c` 或 `-f` 同时使用。它要求在已附加分支的 git 仓库中运行。远程使用方式为 `cruise ssh <host> -- exec --instant "task"`。

当 workflow 配置中设置了 `force_exec: true` 时，传统的位置参数形式 `cruise "task"`、`cruise plan "task"` 和 `cruise --plan "task"` 都会使用当前目录执行路径，不进行规划、不创建 worktree，也不创建 PR。`--no-force-exec`、`--repo`、`--grill`、issue 输入和图片附件会退出该模式；`--skip-planning` 和 `--no-interactive-planning` 不会退出。

<a id="cruise---plan"></a>
## `cruise --plan`

```
cruise --plan <INPUT|stdin> [--skip-planning] [--no-force-exec] [--repo <OWNER/REPO>] [--image <PATH>]... [--flag <VALUE>]...
```

立即创建 session，在分离的 worker 中启动计划生成，然后返回新 session ID。Worker 运行期间，`cruise list` 会将该 session 显示为 `Planning`。若生成失败，`cruise list` 会显示 `Plan Failed`；规划成功前审批会保持禁用状态。

添加 `--skip-planning` 会跳过后台 worker：输入会直接写入 `plan.md`，session 创建时即处于 `Planned` 状态，无需审批（若 `force_exec: true`，除非使用 issue 输入或其他退出方式，否则 workflow 会直接执行；显式传入 `--no-force-exec` 可退出）。不带 `--plan` 时也可使用此 flag（例如 `cruise --skip-planning "task"`），此时行为与 `cruise plan --skip-planning "task"` 相同。

此处也接受 `--repo <owner>/<repository>`，并按 [`cruise plan`](#cruise-plan) 中所述将该仓库作为 session 目标。此路径不支持 `--grill`，因为后台规划期间没有交互用户可接受访谈。

没有子命令、没有 `--plan` 且没有位置输入时，若 stdin 不是 TTY，则会读取到结尾并将内容作为位置输入。因此，`echo "task" | cruise` 会使用传统的位置参数路径，按 `cruise plan "task"` 规划（仍适用 `--skip-planning`、`--no-force-exec`、`--repo` 和 `--image`）。stdin 为交互式时，不带参数的调用仍会打开 TUI。若要在后台规划路径上显式读取管道输入，请传入 `--plan stdin`。

<a id="cruise-tell"></a>
## `cruise tell`

```sh
cruise tell <session-id> "Also cover the timeout case"
printf '%s\n' 'Check the logs' | cruise tell <session-id> -
cruise tell <session-id> --log
cruise ssh devbox --cwd /srv/project -- tell <session-id> "Check the logs"
```

只能向正在运行的 Running session 提交指令。省略指令可从非交互式 stdin 读取；`--log` 会读取指令历史，不会提交指令。被接受不代表已送达：当前正在运行的 agent step 无法看到新指令。指令会追加到下一个 agent prompt 中，包括并行块中的所有 prompt 子步骤，但不包括 command 或 option steps。它不能更改权限、提交策略、输出格式或 workflow 转换。

每条指令都会去除首尾空白，长度上限为 8,192 字节；最多可有 20 条待处理指令，总计最多 65,536 字节。运行完成时，未送达的指令会过期；运行失败或暂停时会保留这些指令，供之后的运行使用；重置为 planned 时会使它们过期。不要在指令中放入机密。

<a id="cruise-list"></a>
## `cruise list`

```
cruise list [OPTIONS]

Options:
      --json   Print all sessions as a JSON array to stdout instead of opening the interactive selector
```

不带 flags 时，会打开交互式 session 浏览器，其菜单取决于每个 session 所处阶段（参见 [`cruise list` 操作](sessions.md#cruise-list-actions)）。使用 `--json` 时，会将所有 session 以 JSON 数组输出，然后退出。

<a id="cruise-diff"></a>
## `cruise diff`

```
cruise diff <SESSION_ID> [--stat]
```

查看 session 分支的补丁；使用 `--stat` 可查看摘要。Worktree 存在时，diff 包含已提交和未提交的受跟踪更改及未跟踪文件。若 worktree 不存在，则只显示分支中已提交的更改。

当终端可用时，完整补丁会通过 `less -R` 显示；否则会直接写入 stdout。`--stat` 会直接写入 stdout。

此功能要求 session 有 worktree 分支。不支持 Current-branch session；创建 PR 后的清理期间可能会删除该分支（包括默认的 `builtin:tdd` workflow）。清理后请使用 **Open PR** 查看更改。Diff 输出是文本，不是 JSON。

<a id="cruise-config"></a>
## `cruise config`

```
cruise config [OPTIONS]

Options:
      --set-parallelism <N>
          Set the maximum number of sessions the WebUI and TUI run concurrently in `run --all` mode.

          Must be >= 1. Omit to show the current configuration. The CLI's one-shot `run --all --parallelism` override is independent.
      --set-language <LANG|auto>
          Set the UI language, or clear the saved setting with auto.
      --set-auto-requeue-max-attempts <N>
          Set the automatic retry limit for Run All (0 disables it).
```

显示或更新应用级设置，设置存储在 `$XDG_CONFIG_HOME/cruise/` 下的 `config.json` 或 `config.yaml` 中（默认：`~/.config/cruise/`）；这与每个 workflow 使用的 YAML 配置不同。不带 flags 时会打印当前配置。`--set-parallelism <N>` 设置 `run_all_parallelism`（默认 `1`），控制 **WebUI 和 TUI** 在 `run --all` 期间并行执行多少个 session。CLI 不读取该持久化值；请改用单次运行的 `cruise run --all --parallelism <N>` flag。

`cruise config --set-language ja` 会保存应用设置 `uiLanguage`（读取时也接受 `ui_language`）。使用 `--set-language auto` 可清除该设置，并恢复自动选择。支持的显式标签为 `en`、`zh-CN`、`es`、`fr`、`pt-BR`、`ja` 和 `ko`。大小写不敏感，且可用下划线替代连字符；但不接受 `pt` 和 `en-US` 等其他标签。`cruise config` 会报告生效的 `ui_language` 及其来源：`env`、`config`、`system` 或 `default`。

CLI 和 TUI 的 UI 语言优先级为 `CRUISE_UI_LANGUAGE` > 应用设置 `uiLanguage` > 系统区域设置 > `en`。环境值为空或为 `auto`，以及配置值未设置或为 `auto`，都会使该层保持未指定。无效的环境值会以英语警告并忽略；CLI setter 和应用配置验证会拒绝无效值。

`cruise config --set-auto-requeue-max-attempts <N>` 会为 CLI、TUI 和 WebUI Run All 设置 `run_all_auto_requeue_max_attempts`。CLI flag `--auto-requeue-max-attempts <N>` 会在单个批次中覆盖该值。

在机器级 `config.yaml` 中设置 `prevent_sleep: true`（或在 `config.json` 中设置 `"prevent_sleep": true`），可在 macOS 运行期间防止系统休眠；默认值为 `false`。这适用于 CLI、TUI 和 WebUI 运行，但不适用于单独规划。其他操作系统不执行任何操作。更改会在下一次运行时生效。项目 `default.yaml` 不支持此设置。

如果两个文件都存在，则 `config.json` 优先；否则使用现有的 `config.yaml`。文件不会合并。更新会按所选文件格式写回；YAML 注释不会保留。

<a id="notifications"></a>
### 通知

请在应用设置文件（而非 workflow YAML）中设置 `notifications`。每个渠道都有主开关，并可选配事件级开关：

```yaml
notifications:
  desktop: true   # default: true
  sound: true     # default: false
  webhook: true   # default: true, but requires the environment variable below
  events:
    actionRequired: { sound: false }
    planReady: { desktop: false }
    completed: { webhook: false }
    failed: { desktop: true, sound: true, webhook: true }
```

事件键为 `actionRequired`、`planReady`、`completed` 和 `failed`；也接受 `action_required` 和 `plan_ready`。每个事件都可以指定 `desktop`、`sound` 和 `webhook`。省略事件开关时默认为 true，但不能覆盖已禁用的渠道主开关。声音与桌面横幅相互独立。Windows 不发送桌面横幅或声音，但支持 webhook。

在 Cruise 进程环境中设置 `CRUISE_NOTIFY_WEBHOOK`，其值为 Slack Incoming Webhook URL。不要将机密 URL 放入设置文件。若该 URL 未设置、为空或不是有效的 HTTP(S) URL，则 webhook 投递会被禁用。请将目标端视为可能会接收到敏感任务详情。

投递尽最大努力进行。非默认通知设置也会显示在 `cruise config` 中，但绝不会显示 webhook URL。

WebUI session 使用浏览器通知代替服务器端桌面横幅，并使用服务器端声音和 webhook 投递。`desktop` 主开关及事件开关也会控制浏览器通知，但不会控制页面内 toast。`CRUISE_DISABLE_NOTIFICATIONS=1` 会禁用桌面通知、声音和 webhook 投递，但不会抑制 WebUI 浏览器通知。其他取值不会启用此全局开关。

使用 `cruise config --set-language <LANG>` 保存 UI 语言，例如：

```sh
cruise config --set-language ja
cruise config --set-language auto
```

TUI 还可按 `L` 打开语言选择器。WebUI 的选择保存在浏览器 cookie 中，而非应用设置中。有关区域设置优先级以及 UI 语言与 prompt 语言的区别，请参阅[应用程序配置](configuration.md#ui-language)。

`model` 和 `plan_model` 可在任一格式中提供可选的全局模型默认值。每项接受字符串或有序数组：第一项是主模型，其余项是 SDK fallback。两种格式使用相同的字段名和别名。`config.yaml` 示例：

```yaml
runAllParallelism: 3
model:
  - openai/gpt-6-luna:max
  - openai/gpt-6.1-sol:xhigh
plan_model:
  - openai/gpt-6.1-sol:xhigh
  - openai/gpt-6-luna:max
```

Workflow 和 `default.yaml` 中的模型设置会覆盖这些默认值；环境变量覆盖的优先级仍然最高。如果没有任何层设置 `plan_model`，规划会使用生效的 `model`。SDK 数组使用现有的 [fallback 行为](sdk.md#sdk-mode)；command 模式只使用主模型。`planModel` 是 `plan_model` 的别名。parallelism 未设置时默认为 `1`，因此只配置模型也是有效的。通过 CLI、WebUI 或 TUI 更新 parallelism 时，会保留这两个模型设置。

同一应用设置文件还可用于组织交互式 workflow 选择器：

```yaml
workflow_categories:
  Development:
    workflows:
      - builtin:tdd
      - refactor: Refactor an existing codebase
    Backend:
      workflows: [api]
show_others_category: true
others_category_name: Others
```

`workflow_categories` 是有序的类别映射树。每个类别都可以包含 `workflows` 列表和嵌套类别。条目可以是名称，也可以是单键映射，其值会覆盖选择器中的说明。`show_others_category` 默认值为 `true`，`others_category_name` 默认值为 `Others`。这些设置应放在应用级 `config.yaml` 或 `config.json` 中，而不是 workflow 文件或 `default.yaml` 中。有关匹配和回退行为，请参阅[配置文件解析](configuration.md#config-file-resolution)。

Workflow 和应用设置都支持在 `model` 和 `plan_model` 数组中使用带 SDK 的条目：

```yaml
model:
  - openai/gpt-6.1-sol:medium
  - sdk: claude
    model: claude-sonnet-5-5:medium
  - openrouter/z-ai/glm-5.3-flash:high
```

对应的 JSON 为 `{"model":["openai/gpt-6.1-sol:medium",{"sdk":"claude","model":"claude-sonnet-5-5:medium"},"openrouter/z-ai/glm-5.3-flash:high"]}`。字符串会继承 workflow 的有效 SDK；若未设置 `sdk` 或 `command`，默认使用 `jcode`。`openai/` 和 `openrouter/` 等 provider 前缀是模型引用，不是 SDK 名称。也支持现有的 `opencode` SDK 选择。

对象必须包含非空白 `model`，并且至少包含 `sdk` 或非空的 `disabled_during`；对象只能出现在数组中。不接受未知键和显式的 null SDK。省略 `sdk` 时会继承 workflow SDK，而不是前一条目的 SDK。对象不能与 `command` 组合使用，即使是在 fallback 位置也不行。仅含字符串的旧版 command 数组仍只使用第一个模型。环境模型覆盖会替换整个列表。能力检查会覆盖所有适用候选项：computer use 要求 jcode；远程 HTTP/SSE MCP 不能使用 jcode；OpenCode 不支持旧版 SSE。

<a id="candidate-unavailable-windows-disabled_during"></a>
## 候选模型不可用时间窗口（`disabled_during`）

可在 `model` 或 `plan_model` 数组中为每个候选模型声明不可用时间。该功能适用于 workflow、分层的 `default.yaml` 和应用 JSON/YAML 设置（包括 `planModel` 别名）。这些是由用户维护的排除规则，不是按价格路由，也不是 provider 预设。

```yaml
sdk: opencode
model:
  - model: opencode-go/deepseek-v4.1-flash
    disabled_during: ["mon-fri 01:00-04:00", "mon-fri 06:00-10:00"]
  - opencode-go/kimi-k2.6
```

将相同窗口表示为固定的 `+09:00` 偏移量：

```yaml
sdk: opencode
model:
  - model: opencode-go/deepseek-v4.1-flash
    disabled_during: ["mon-fri 10:00-13:00 +09:00", "mon-fri 15:00-19:00 +09:00"]
  - opencode-go/kimi-k2.6
```

窗口语法为 `[days ]HH:MM-HH:MM[ offset]`。星期名称 `mon` 至 `sun` 不区分大小写；支持逗号列表（`sat,sun`）以及包含端点且可跨周回绕的范围（`mon,wed-fri`、`fri-mon`）。省略星期表示每天。时间使用两位数的小时和分钟：开始时间范围为 `00:00` 至 `23:59`，结束时间范围为 `00:01` 至 `24:00`。开始和结束相同为无效值；若要表示全天，请使用 `00:00-24:00`。时间段包含开始时刻，但不包含结束时刻。结束时间早于开始时间时，窗口会跨越午夜，星期表示**开始日**：`fri 22:00-02:00` 包含星期六 01:00，但不包含星期日 01:00。

偏移量默认为 UTC；接受 `UTC`、`Z` 或固定的 `+HH:MM` / `-HH:MM`，绝对值最大为 `14:00`。不支持 IANA 名称、夏令时转换或系统本地区域设置。空列表、空白窗口和格式错误的窗口都会导致验证失败。

Cruise 会在每次 prompt 开始时以及重试期间切换候选项时检查窗口。它不会中断已经开始输出的 turn。只要与窗口匹配，无论重试策略或重试预算如何，该候选项都会被排除。如果所有候选项都处于禁用窗口或冷却状态，prompt 会失败，且不会启动后端。

能力验证仍会检查禁用候选项。只有解析后的 SDK 和基础模型都与主候选项匹配时，覆盖值才会继承 workflow 列表及其窗口。若 SDK 也匹配，只更改 effort 会保留继承关系。其他覆盖值不会继承 workflow 列表。环境模型覆盖会替换整个列表，包括其中的窗口。

<a id="cruise-clean"></a>
## `cruise clean`

```
cruise clean
```

通过 `gh pr view`（GitLab 上为 `glab mr view`）检查每个 Completed session 的 PR 状态。PR 已关闭或已合并的 session 会连同其 worktree（以及任何残留的 `--repo` 克隆）一起删除。若 Planned current-branch session 从未启动，且其 `plan.md` 缺失或为空，也会被删除。Suspended session 和其他 planned session 会继续保留以供恢复。

> **注意：** 若 `gh pr create` 失败或未执行，session 可能没有 PR URL。没有 PR URL 的 PR-backed session 会保留；请检查 session 日志，或手动使用 `gh pr create` 重新创建 PR。

<a id="cruise-usage"></a>
## `cruise usage`

`cruise usage [--by session,step,purpose,backend,model] [--session ID] [--format text|json|csv]` 汇总本地记录的调用尝试。默认分组方式为 `step,backend,model`，默认格式为文本。缺失的 provider 值视为 unknown，而不是零；总量只包括输入和输出。成本使用 provider 报告值，不是估算值。
