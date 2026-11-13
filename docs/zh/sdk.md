[English](../en/sdk.md) | [日本語](../ja/sdk.md) | [简体中文](../zh/sdk.md) | [返回目录](README.md)

# SDK 后端与权限参考

<a id="sdk-mode"></a>
## SDK 模式

提示步骤可以通过设置顶层 `sdk` 字段，交由 SDK 后端驱动，而不必通过 `command` 启动外部 CLI。`command` 和 `sdk` 互斥；只有在应用了适用的 `default.yaml` 层之后，解析得到的工作流中两者都未设置时，才会使用默认的 `jcode` 后端。接受三个值：`jcode`、`claude` 和 `opencode`。

<a id="startup-oauth-expiration-checks"></a>
### 启动时的 OAuth 过期检查

在启动智能体前，Cruise 会检查适用 SDK 的本地 OAuth 凭据文件。仅当访问令牌已过期，且刷新令牌不存在或其记录的过期时间早于或等于当前时间时，才会停止。重新登录请根据需要使用 `claude auth login`、`jcode login claude` 或 `jcode login openai`。

这是过期预检，并不能证明身份验证会成功。要绕过误报，请设置进程环境变量 `CRUISE_SKIP_AGENT_AUTH_CHECK=1`；只有精确值 `1` 才会跳过检查。

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```

在所有 SDK 后端中，`model` / `plan_model` / 每步的 `model` 都是**模型引用**，其覆盖优先级与命令模式相同（步骤 `model` > 顶层 `model` / `plan_model`）。对于 jcode 和 Claude，可选的 `:effort` 后缀用于选择推理强度级别（`low` / `medium` / `high` / `xhigh` / `max`，另有别名 `minimal` / `min` / `med` 以及数字写法 `1`..`4`）；`off` / `none` / `0` / `5` 也会被解析并移除，但不会设置推理强度，其他任何 `:` 后缀（例如 OpenRouter 的 `:free` 变体）则仍是模型 ID 的一部分。对于 jcode/Claude，如果提供商或模型不支持某个推理强度，该强度设置会被忽略。OpenCode 则会依据其目录校验推理强度，并在不支持时明确报错（见下文）。

在工作流级别，`model` 和 `plan_model` 可以是数组。在 SDK 模式下，第一个条目是主模型，后续条目构成隐式回退链。包含回退条目的模型数组会自动启用模型回退；针对主模型显式设置的 `retry.fallback_chains` 条目优先于数组中的后续条目。

- `"provider/model[:effort]"`（例如 `provider/model:xhigh`） -- `sdk: jcode` 会分别解析提供商和模型；任意一侧为空（`"/model"`、`"provider/"`）都会在运行提示时报错。`sdk: claude` 会将 `:effort` 后缀之外的全部内容视为模型名称。
- `"model"`（不含 `/`），适用于 jcode/Claude -- 不显式提供提供商。配置的 OpenCode 引用必须包含提供商。
- `"provider/model[:effort]"` 或 `"provider/model#variant"`，适用于 `sdk: opencode`。嵌入的 `/` 和未知的冒号后缀（如 `:free` 或 `:2026`）仍保留在模型 ID 中。可识别的末尾推理强度后缀属于保留语法，不能与 `#variant` 组合使用。
- 未设置 -- 不显式提供模型。OpenCode 使用其配置的默认值。如果 jcode 提示仍没有实际生效的模型，则会因配置错误而失败；
  Claude 和 `command:` 保留现有的未指定模型时的行为。

<a id="subagent-model"></a>
### 子智能体模型

在工作流级别或提示步骤上设置 `subagent_model`，可为其启动的智能体选择模型。
步骤值会覆盖工作流值。它接受单个非空白字符串或 `{sdk: claude, model: haiku}`（也支持 `sdk: jcode`），
不接受数组或回退链。字符串形式继承工作流的 SDK，默认为 jcode，
而不是继承某次回退尝试的 SDK。只有 SDK 与解析出的子智能体 SDK 匹配的尝试才会接收此设置；
其他尝试保留后端默认值。
验证会拒绝没有适用模型候选项的设置。

```yaml
sdk: claude
model: sonnet
subagent_model: haiku
steps:
  review:
    prompt: Review the implementation, delegating checks as needed.
    subagent_model: {sdk: claude, model: sonnet}
```

- 不支持 OpenCode 子智能体模型和 `command:` 后端。混合 SDK 工作流可以通过显式 SDK 对象选择 Claude/jcode 候选项。可识别的 `:effort` 后缀会被拒绝，即使是 `:off` 也不例外；`:free` 等未知后缀仍是模型 ID 的一部分。jcode 要求模型有效且非空。

<a id="sdk-jcode--the-jcode-sdk-default"></a>
### `sdk: jcode` — jcode SDK（默认）

`sdk: jcode` 使用官方 jcode Rust SDK，要求 jcode v0.88.0 或更高版本。启动时，Cruise 会验证 SDK 握手和所需能力（`sessions`，以及在注册了自定义工具时所需的 `session_tools`）；不会探测 CLI 版本。如果缺少所需能力，请升级 jcode。`model`、`plan_model` 和步骤级 `model` 字段接受 `provider/model[:effort]` 或裸模型。每次实际执行的 jcode 提示都必须解析出模型：普通提示先使用步骤 `model`，然后回退到工作流/默认 `model`；规划调用先使用 `plan_model`，然后回退到 `model`。已配置的 jcode 默认值、`JCODE_MODEL` 或恢复的会话都不能替代缺失的 Cruise 模型。缺少模型时，会创建或保留用户的 `default.yaml` 模板，并停止该请求。推理强度后缀通过 SDK 应用。

<a id="computer-use"></a>
#### 计算机使用

工作流级 `computer_use` 默认为 `false`。将其设为 `true`，可允许提示轮次在 macOS 上使用 jcode 的 `macos_computer_use` 桌面控制工具；提示步骤可以覆盖工作流默认值。内置的 plan、fix-plan 和 ask-plan 轮次遵循工作流值；标题和 PR 描述生成始终关闭计算机使用功能。该工具仅支持 macOS，在 Linux 上无效。所有适用的模型候选项都必须使用 jcode，且不得设置 `command:` 后端。显式限定为 jcode 的候选项也可用于 Claude 或 OpenCode 工作流。用户必须授予 macOS 辅助功能和屏幕录制权限。

<a id="permission-modes"></a>
### 权限模式

工作流级的 `permission` 默认为 `full`。提示步骤可以通过设置 `permission: read-only | edit | full` 来覆盖此默认值。`full` 保持当前行为。`edit` 禁用 shell 执行（Claude 的 `Bash`、jcode 的 `bash`），但保留直接编辑、写入和打补丁的工具。`read-only` 还会禁用文件修改工具（Claude 的 `Edit`/`Write`/`NotebookEdit`，jcode 的 `edit`/`write`/`apply_patch`/`replace`）。受限模式会强制关闭 `macos_computer_use`，即使设置了 `computer_use: true` 也是如此；Claude 会以 `dontAsk` 运行这些模式，因此不会有权限提示阻塞无人值守的运行。Jcode 中按模式禁用的工具会合并到选定的 `JCODE_DISABLED_TOOLS` 列表中。

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

这些值与具体 SDK 无关，并非 Claude 的 `Bash(...)` 语法。若要全面限制 shell 执行或编辑操作，请使用 `permission`；单独使用 `shell` 和 `edit` 是无效的。shell 前缀会被去除首尾空白，连续空白也会被合并。前缀不能为空，且只能使用 ASCII 字母、数字、空格以及 `._/:=@+%-`。通配符、引号、逗号、圆括号、shell 运算符、反斜杠、换行符和控制字符均会被拒绝。如果最后一个单词以 `=` 结尾，则会将其作为单词前缀进行匹配，例如 `shell:git -c core.hooksPath=`。

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

MCP 限制仅在后端启动前筛选由工作流提供的条目。后端的用户级 MCP 配置和 Cruise 自身的会话工具仍保持启用。Claude 的 shell 前缀禁止规则不会规范化 Git 全局选项，因此 `git -C path commit` 可能无法匹配 `git commit`。在 jcode 中，工作流或进程环境中的 `JCODE_HOOKS_DISABLED` 会禁用 shell 检查关卡，钩子执行失败时也会放行。这些控制属于纵深防御，并非沙箱；提示警告、reference-transaction 钩子和 CAS 回滚仍然保留。

<a id="sdk-claude--the-claude-cli-in-process"></a>
### `sdk: claude` — 在进程内运行的 claude CLI

`sdk: claude` 通过 claude-agent-sdk 在进程内驱动 `claude` CLI。模型引用使用普通的 `claude --model` 名称，可带有通过 `--effort` 转发的 `:effort` 后缀。不支持该选项的 CLI 会因 `unknown option '--effort'` 使步骤失败；此错误为永久性错误，绝不重试。Cruise 已针对 2.1.250 完成验证。身份认证使用 claude CLI 自身保存的凭据或 `ANTHROPIC_API_KEY`，与 `jcode login` 无关。由于 Cruise 工作流无人值守且没有控制台可响应权限提示，CLI 会绕过权限检查。由于 Claude Code 的计算机使用功能仅支持交互式会话，`computer_use: true` 会在配置加载时被拒绝。

<a id="tool-less-non-interactive-planning"></a>
### 无工具（非交互式）规划

默认情况下，SDK 模式下的规划会使用会话工具来提问，以及提交或修订计划。

设置 `interactive_planning: false` 可关闭这一行为。此时，规划过程会将目标计划文件的路径嵌入提示中，并要求智能体直接写入 `plan.md`——与 `command` 后端完全相同。随后会回读生成的 `plan.md`（如果文件未写入，则回退到已捕获的智能体输出）。

```yaml
interactive_planning: false   # tool-less, file-based planning
```

`--grill` 要求使用交互式规划流程，因此在关闭 `interactive_planning` 时会被拒绝。该字段在 `command` 模式下不起作用；该模式始终基于文件。

<a id="sdk-opencode---the-opencode-v2-client"></a>
### `sdk: opencode` - OpenCode V2 客户端

请使用 `cruise install opencode` 或另行安装 OpenCode CLI，并完成身份验证。

```yaml
sdk: opencode
model: "provider/model#variant" # Replace with a model in your OpenCode setup
plan_model: "provider/planning-model"
interactive_planning: true
permission: full
steps:
  implement:
    prompt: "{input}"
```

工作流、规划和步骤的模型可使用 `provider/model[:effort]` 或原生的 `provider/model#variant`。可用的 effort 包括 `low`、`medium`、`high`、`xhigh` 和 `max`；普通 effort 与原生 `#variant` 不能组合使用。

省略模型时，会交由 OpenCode 使用其配置的默认值。工作流和步骤的 `env:` 值会连同继承的操作系统环境和提交防护一起传递给提示工具。继续使用现有的 OpenCode 全局/项目配置和身份验证。

Cruise 的规划、提问、标题生成和 PR 元数据通过原生 V2 插件使用现有工具，而不是 MCP 服务器。明确不支持 `computer_use: true`。`permission: edit` 允许直接编辑文件，但禁止 shell、子智能体和未知执行工具；`read-only` 还会禁止修改操作，同时保留安全的 Cruise 工具和工作区快照检查。受限模式会禁止未知的 MCP 和 Code Mode 执行路径。这些是工具限制，并非操作系统沙箱。

工作流 MCP 条目支持本地 `stdio` 和远程 Streamable HTTP（`type: http`）。旧版 `type: sse` 会被拒绝，并给出指明服务器名称的诊断信息，绝不会静默转换。无人值守运行期间不会启动 OAuth 登录流程。工作流条目会以叠加方式应用到 OpenCode 配置中，不会更改源文件。

参见 [OpenCode 工作流示例](../../skills/cruise-config/examples/opencode-flow.yaml)。

<a id="mcp-servers-mcp_servers"></a>
### MCP 服务器（`mcp_servers`）

可选的顶层 `mcp_servers` 映射使 MCP 服务器可用于每一次 SDK 提示执行：工作流提示步骤（包括并行子步骤）、`after-pr` 步骤、规划轮次、会话标题生成和 PR 描述生成。它适用于 `sdk: jcode`；对于默认的 `jcode` 后端，只有在应用适用的 `default.yaml` 层后，解析得到的工作流中既未设置 `sdk`、也未设置 `command` 时才适用；也适用于 `sdk: claude` 或 `sdk: opencode`。不支持逐步骤覆盖，但提示中的 `disallowed_tools: [mcp]` 或 `["mcp:<name>"]` 可以为某个步骤排除工作流条目；用户级 MCP 配置不受影响。被调用工作流的顶层 `mcp_servers` 会被忽略；调用方的设置继续生效。

条目采用 Claude Code 的 `mcpServers` 服务器结构。对于本地服务器，可省略 `type`，或设置 `type: stdio`。每个实际生效且适用的候选项，包括指定了 SDK 的对象和具有权威性的重试链条目，都必须支持相应传输方式：stdio 支持所有 SDK，HTTP 要求使用 Claude 或 OpenCode，旧版 SSE 则要求使用 Claude。更改工作流 SDK 不会覆盖指定了 SDK 的对象。`command` 后端无法接收 MCP 服务器：

```yaml
sdk: claude
mcp_servers:
  local_search:
    command: npx
    args: ["-y", "@example/search"]
    env:
      SEARCH_ROOT: ./docs
  remote_docs:
    type: http
    url: https://mcp.example.test/server
    headers:
      Authorization: "Bearer replace-with-your-token"
```

MCP 条目的值会直接透传，不会经过 Cruise 的 `{variable}` 模板解析。名称必须非空，且只能包含 ASCII 字母、数字、`_` 或 `-`；`cruise` 为保留名称。Stdio 条目要求 `command` 非空白，且不能设置 `url`，也不能提供非空的 `headers`。`http` 和 `sse` 条目要求提供以 `http://` 或 `https://` 开头的 URL，且不能设置 `command`，也不能提供非空的 `args` 或 `env`。未知的条目字段会被拒绝。在 `command` 后端中使用非空的 `mcp_servers` 映射会导致验证错误。
