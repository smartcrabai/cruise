[English](../en/sdk.md) | [日本語](../ja/sdk.md) | [简体中文](../zh/sdk.md) | [返回目录](README.md)

# SDK 后端与权限参考

<a id="sdk-mode"></a>
## SDK 模式

提示步骤可以通过设置顶层 `sdk` 字段，使用 jcode SDK 驱动，而不必通过 `command` 启动外部 CLI。`command` 和 `sdk` 互斥；只有在应用适用的 `default.yaml` 层后，两者都未设置时，才会使用默认的 `jcode` 后端。唯一受支持的 SDK 值是 `jcode`；其他值都会导致配置验证错误。

<a id="startup-oauth-expiration-checks"></a>
### 启动时的 OAuth 过期检查

启动智能体前，Cruise 会检查适用的显式 jcode Claude 和 OpenAI OAuth 候选项的本地凭据文件。访问令牌过期且没有刷新令牌或已知的刷新令牌也已过期时，Cruise 才会停止。根据需要使用 `jcode login claude` 或 `jcode login openai` 重新登录。

这是过期预检，并不能证明身份验证会成功。要绕过误报，请设置进程环境变量 `CRUISE_SKIP_AGENT_AUTH_CHECK=1`；只有精确值 `1` 才会跳过检查。

对于显式指定 jcode `openai` 和 `claude` 的模型，当剩余有效期不足两小时时，Cruise 会在每次尝试前于用户的 jcode home 中刷新登录，因此每次尝试的私有运行时不会自行轮换共享的刷新令牌。如果刷新失败且访问令牌将在十分钟内过期，该尝试会失败并提示使用 `jcode login`。

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```

在 jcode 模式下，`model`、`plan_model` 和每步的 `model` 都是模型引用，其覆盖优先级与命令模式相同（步骤 `model` > 顶层 `model` / `plan_model`）。可使用 `provider/model[:effort]`（例如 `claude/claude-opus-4-6`）或裸模型。可选的 `:effort` 后缀用于选择推理强度级别：`low` / `medium` / `high` / `xhigh` / `max`，以及别名 `minimal` / `min` / `med` 和数字写法 `1`..`4`。`off` / `none` / `0` / `5` 会被解析但不设置推理强度；其他后缀（例如 OpenRouter 的 `:free`）仍是模型 ID 的一部分。提供商或模型不支持某个推理强度时，该设置会被忽略。

工作流级别的 `model` 和 `plan_model` 可以是字符串，也可以是由字符串和 `{model, disabled_during}` 对象组成的数组。第一个条目是主模型，后续条目构成回退链。包含回退条目的数组会自动启用模型回退；显式的字符串型 `retry.fallback_chains` 条目会优先决定回退顺序。带有 `sdk` 的对象不受支持，并会导致配置验证错误：`SDK-qualified model objects are unsupported; use a plain model string`。

- `"provider/model[:effort]"`（例如 `provider/model:xhigh`）— jcode 会分别解析提供商和模型；任意一侧为空（`"/model"`、`"provider/"`）都会在运行提示时报错。
- `"model"`（不含 `/`）— 不显式提供提供商。
- 未设置 — 如果 jcode 提示仍没有实际生效的模型，则会因配置错误而失败；`command:` 保留现有的未指定模型时的行为。
<a id="subagent-model"></a>
### 子智能体模型

在工作流级别或提示步骤上设置 `subagent_model`，可为其启动的智能体选择模型。步骤值会覆盖工作流值。只接受单个非空白模型字符串，不接受对象、数组或回退链。该设置用于 jcode 提示步骤，`command:` 后端不支持。

```yaml
sdk: jcode
model: openai/gpt-6.1-sol
subagent_model: openai/gpt-6.1-sol
steps:
  review:
    prompt: Review the implementation, delegating checks as needed.
    subagent_model: anthropic/claude-sonnet-4-6
```

jcode 子智能体模型必须解析为有效的非空模型。

<a id="advisor-steps"></a>
### 顾问步骤

`advisor: true` 只能用于顶层 `steps` 下的普通提示步骤，包括通过 `workflow_call` 内联到此处的步骤。必须设置 `output_file`，且不得设置 `allow_commit: true`。无论工作流的默认权限是什么，该步骤都会强制使用[只读权限](#permission-modes)。请省略步骤的 `permission` 或设为 `read-only`。

模型按步骤 `model` → `advisor_model` → `plan_model` → 工作流 `model` 的顺序选择，空数组视为未设置。与 `plan_model` 一样，`advisor_model` 接受模型字符串，或包含可选 `disabled_during` 项的回退数组。可在工作流或用户／项目的 `default.yaml` 中设置，也可用 `CRUISE_ADVISOR_MODEL`（字符串或 JSON 数组）覆盖。它不是 `cruise config` 的设置项，也不影响其他提示。

启用的顾问步骤需要 jcode。执行前检查（包括试运行）会豁免因标志关闭或被用户明确跳过的顾问步骤。`cruise workflow doctor` 只应用基于标志的跳过。顾问不会消耗 `cruise tell` 排队的指令。

执行前，Cruise 会生成会话产物 `<output_file>.diff`，可通过 `{file:<output_file>.diff}` 读取。文件先列出变更统计，再列出相对于 `HEAD` 的未提交补丁，包括未跟踪且未被忽略的文件，但排除 `.worktreeinclude` 中列出的路径。超过 256 KiB 时会在行边界截断并加上提示。每次尝试前或跳过步骤时，Cruise 都会清除旧建议，仅在成功后写入新建议。如果没有新建议，通过 `{file:<output_file>}` 读取时会报错，不会使用旧内容。

普通错误（如无法生成 diff、后端报错、超时）可通过 `if.fail` 处理。如果 Cruise 检测到工作区变化、无法验证工作区，或用户中断，则无论 `if.fail` 如何设置都会停止运行。建议和 diff 都是不可信参考资料，不能作为指令、流程控制或审批的输入。

<a id="sdk-jcode--the-jcode-sdk-default"></a>
### `sdk: jcode` — jcode SDK（默认）

`sdk: jcode` 使用官方 jcode Rust SDK，要求 jcode v0.88.0 或更高版本。启动时，Cruise 会验证 SDK 握手和所需能力（`sessions`，以及在注册了自定义工具时所需的 `session_tools`）；不会探测 CLI 版本。如果缺少所需能力，请升级 jcode。`model`、`plan_model` 和步骤级 `model` 字段接受 `provider/model[:effort]` 或裸模型。每次实际执行的 jcode 提示都必须解析出模型：普通提示先使用步骤 `model`，然后回退到工作流/默认 `model`；规划调用先使用 `plan_model`，然后回退到 `model`。已配置的 jcode 默认值、`JCODE_MODEL` 或恢复的会话都不能替代缺失的 Cruise 模型。缺少模型时，会创建或保留用户的 `default.yaml` 模板，并停止该请求。推理强度后缀通过 SDK 应用。

<a id="computer-use"></a>
#### 计算机使用

工作流级 `computer_use` 默认为 `false`。将其设为 `true`，可允许提示轮次在 macOS 上使用 jcode 的 `macos_computer_use` 桌面控制工具；提示步骤可以覆盖工作流默认值。内置的 plan、fix-plan 和 ask-plan 轮次遵循工作流值；标题和 PR 描述生成始终关闭计算机使用功能。该工具仅支持 macOS，在 Linux 上无效。所有适用的模型候选项都必须使用 jcode，且不得设置 `command:` 后端。用户必须授予 macOS 辅助功能和屏幕录制权限。

<a id="permission-modes"></a>
### 权限模式

工作流级的 `permission` 默认为 `full`。提示步骤可以设置 `permission: read-only | edit | full` 来覆盖此默认值。`edit` 禁用 shell 执行，但保留直接编辑、写入和打补丁的工具。`read-only` 还会禁用文件修改工具。受限模式会强制关闭 `macos_computer_use`，即使设置了 `computer_use: true` 也是如此；jcode 按模式禁用的工具会合并到选定的 `JCODE_DISABLED_TOOLS` 列表中。

- 使用 `command:` 后端时，会拒绝 `read-only` 和 `edit`，因为 cruise 无法控制任意 CLI 的工具。
- 将 `read-only` 与 `allow_commit: true` 组合使用属于配置错误。`allow_commit: true` 仅移除 HEAD/ref 提交防护和隐式 Git 禁止规则，不会移除显式限制，也绝不会放宽权限模式。
- 分组调用、`workflow_call` 或 `parallel` 包装层均不能设置 `permission`。请在内部提示步骤或并行提示子步骤上设置它。含有任何只读子步骤的并行块都只能包含只读提示子步骤。
- `workflow_call` 被调用方的顶层 `permission` 会被忽略（采用调用方的默认值），但会保留被调用方内部的逐步骤覆盖设置。
- 如果只读步骤执行期间工作区快照发生变化，该步骤还会失败并报告 `ReadOnlyWorkspaceChanged`。这是变更检测，不是操作系统沙箱或回滚机制，并且会忽略 `.git`、`target` 和 `node_modules`。
- 内置的规划、修复/询问、标题、PR 描述和工作流生成轮次始终以 `full` 运行，不使用 `disallowed_tools` 或隐式 Git 限制。

<a id="tool-restrictions"></a>
### 工具限制

`disallowed_tools` 会收窄 SDK 提示的能力范围，但不会授予权限：

```yaml
disallowed_tools: ["shell:npm publish"]
steps:
  review:
    prompt: Review without publishing or delegating.
    disallowed_tools: [web, subagent, "mcp:github", "shell:gh pr merge"]
```

| 值 | 禁止的能力 |
|-------|-------------------|
| `web` | Web 获取/搜索，以及可用时的后端浏览器工具 |
| `subagent` | 子代理或 swarm 委派 |
| `mcp` | 工作流中的所有 `mcp_servers` 条目 |
| `mcp:<name>` | 一个工作流 MCP 服务器；未知名称会导致错误 |
| `shell:<prefix>` | 起始单词与该前缀匹配的简单 shell 命令 |

这些值与具体 SDK 无关。若要全面限制 shell 执行或编辑操作，请使用 `permission`；单独使用 `shell` 和 `edit` 是无效的。shell 前缀会被去除首尾空白，连续空白也会被合并。前缀不能为空，且只能使用 ASCII 字母、数字、空格以及 `._/:=@+%-`。通配符、引号、逗号、圆括号、shell 运算符、反斜杠、换行符和控制字符均会被拒绝。如果最后一个单词以 `=` 结尾，则会将其作为单词前缀进行匹配，例如 `shell:git -c core.hooksPath=`。

生效列表是经过去重的**工作流默认值、步骤限制和隐式 Git 规则的并集**。步骤级的 `[]` 无法撤销工作流限制。在用户/项目 `default.yaml` 和工作流这些层级之间，优先级最高的显式工作流级列表会替换较低优先级的列表；工作流中的 `disallowed_tools: []` 会清除继承的默认值。在 `edit` 或 `read-only` 模式下，shell 规则会被移除，因为 shell 已被禁用。

非空列表仅适用于提示步骤，包括并行提示子步骤、分组或展开后的工作流中的提示步骤、`after-pr`，以及 `github-review` 循环。命令/选项步骤、并行包装层、分组和 `workflow_call` 的调用点，以及原生 `squash-merge` 均会拒绝非空列表。被调用的工作流会保留步骤级列表，但其顶层列表会被忽略，改用调用方的列表。`command:` 提示后端会拒绝显式限制，也不会强制执行隐式 shell 规则；其现有的提交防护仍然有效。

除非设置了 `allow_commit: true`，否则 SDK 提示会隐式禁止以下 shell 前缀：

```text
git commit                 git merge                  git rebase
git cherry-pick            git revert                 git am
git pull                   git push                   git reset
git switch                 git checkout -b            git checkout -B
git update-ref             git config core.hooksPath  git -c core.hooksPath=
```

这也会阻止 `git reset HEAD file`；请使用 `git restore --staged` 来取消暂存。这不会阻止 `git checkout -- file`、`git stash create`、使用 `git branch` 列出分支，或运行 `git status`/`diff`/`log`。前缀匹配无法区分所有安全与不安全的 Git 操作，因此提交防护仍是最后一道防线。与作用范围限定在仓库内的提交防护不同，shell 前缀禁止规则还适用于针对临时仓库或无关仓库的匹配命令。

MCP 限制仅在后端启动前筛选由工作流提供的条目。后端的用户级 MCP 配置和 Cruise 自身的会话工具仍保持启用。在 jcode 中，工作流或进程环境中的 `JCODE_HOOKS_DISABLED` 会禁用 shell 检查关卡，钩子执行失败时也会放行。这些控制属于纵深防御，并非沙箱；提示警告、reference-transaction 钩子和 CAS 回滚仍然保留。

只有使用 `jcode-bubblewrap` feature 编译的 Linux 构建才提供 Bubblewrap 隔离。普通 jcode 尝试仅在守护进程启动时设置 `CRUISE_JCODE_BUBBLEWRAP=1` 才会请求隔离；工作流中的 `require_bubblewrap: true` 无论该环境变量如何设置都会请求隔离。Cruise 每个进程只探测一次可用性。如果构建未启用该 feature、宿主不是 Linux，或探测失败，请求隔离的尝试会在未隔离的情况下运行并显示警告。`command:` 后端获准运行，会在未隔离的情况下运行并显示警告。成功探测后 Bubblewrap 启动失败，仍会使本次尝试失败。每次隔离尝试都会启动独立 daemon，并以只读方式挂载 Git 元数据。`GIT_OPTIONAL_LOCKS=0` 允许 `git status`、`git diff` 和 `git log`；暂存、提交和推送由宿主执行。宿主替换索引后，下次尝试才能看到新索引。此模式不连接共享的 `jcode serve`。


<a id="tool-less-non-interactive-planning"></a>
### 无工具（非交互式）规划

默认情况下，SDK 模式下的规划会使用会话工具来提问，以及提交或修订计划。

设置 `interactive_planning: false` 可关闭这一行为。此时，规划过程会将目标计划文件的路径嵌入提示中，并要求智能体直接写入 `plan.md`——与 `command` 后端完全相同。随后会回读生成的 `plan.md`（如果文件未写入，则回退到已捕获的智能体输出）。

```yaml
interactive_planning: false   # tool-less, file-based planning
```

`--grill` 要求使用交互式规划流程，因此在关闭 `interactive_planning` 时会被拒绝。该字段在 `command` 模式下不起作用；该模式始终基于文件。


<a id="mcp-servers-mcp_servers"></a>
### MCP 服务器（`mcp_servers`）

可选的顶层 `mcp_servers` 映射使 MCP 服务器可用于每一次 jcode 提示执行：工作流提示步骤（包括并行子步骤）、`after-pr` 步骤、规划轮次、会话标题生成和 PR 描述生成。不支持逐步骤覆盖，但提示中的 `disallowed_tools: [mcp]` 或 `["mcp:<name>"]` 可以为某个步骤排除工作流条目；用户级 MCP 配置不受影响。被调用工作流的顶层 `mcp_servers` 会被忽略；调用方的设置继续生效。

条目使用 MCP 服务器格式。对于本地服务器，可省略 `type`，或设置 `type: stdio`。HTTP 和 SSE 传输方式不受支持，会导致配置验证错误。`command` 后端无法接收 MCP 服务器：

```yaml
sdk: jcode
mcp_servers:
  local_search:
    command: npx
    args: ["-y", "@example/search"]
```

MCP 条目的值会直接透传，不会经过 Cruise 的 `{variable}` 模板解析。名称必须非空，且只能包含 ASCII 字母、数字、`_` 或 `-`；`cruise` 为保留名称。Stdio 条目要求 `command` 非空白，且不能设置 `url`，也不能提供非空的 `headers`。未知的条目字段会被拒绝。在 `command` 后端中使用非空的 `mcp_servers` 映射会导致验证错误。
