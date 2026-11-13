[English](../en/sdk.md) | [日本語](../ja/sdk.md) | [简体中文](../zh/sdk.md) | [返回目录](README.md)

# SDK 后端与权限参考

<a id="sdk-mode"></a>
## SDK 模式

提示步骤可以通过设置顶层 `sdk` 字段，交由 SDK 后端驱动，而不必通过 `command` 启动外部 CLI。`command` 和 `sdk` 互斥；只有在应用了适用的 `default.yaml` 层之后，解析得到的工作流中两者都未设置时，才会使用默认的 `jcode` 后端。接受三个值：`jcode`、`claude` 和 `opencode`。

<a id="startup-oauth-expiration-checks"></a>
### 启动时的 OAuth 过期检查

在为 `cruise plan`、`cruise run` 或 `cruise exec` 启动智能体之前，以及在 TUI 或 WebUI 中进行会话规划/执行之前，Cruise 仅检查适用 SDK 候选项的本地 OAuth 凭据文件。规划使用实际生效的 `plan_model`（回退到 `model`）；执行时的检查涵盖工作流模型和步骤级覆盖，包括 `after-pr`、组和并行子步骤。模型数组、显式指定 SDK 的条目以及实际生效的重试回退链均包含在内，即使某个回退项最终并未使用。这不是在 TUI/WebUI 启动时执行的全局检查。命令后端、试运行、`cruise workflow generate` 和 `cruise workflow make` 均不在此检查范围内。

- **Claude SDK：** 读取 `$CLAUDE_CONFIG_DIR/.credentials.json`，未设置时则读取 `~/.claude/.credentials.json`，使用其中的 `claudeAiOauth.expiresAt`、`refreshToken` 和可选的 `refreshTokenExpiresAt`。
- **Jcode SDK：** 仅检查源主目录下 `auth.json` 中显式指定的提供商 `claude`、`claude-oauth` 和 `claude-subprocess`，以及 `openai-auth.json` 中的 `openai` 和 `openai-oauth`。源主目录由进程的 `JCODE_HOME` 指定，未设置时为 `~/.jcode`。存在多个账户时，只有在每个账户都被确定已过期的情况下才会停止。
- **跳过检查的情况：** OpenCode、未指定提供商的 jcode 裸模型、`anthropic-api` / `openai-api` 等 API 密钥提供商，以及其他所有提供商。文件缺失/不可读、JSON 无效或过期数据不足时，均会放行，不会阻断。对于 Claude，如果进程环境或工作流级 `env` 中的 `ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN`、`CLAUDE_CODE_OAUTH_TOKEN`、`CLAUDE_CODE_USE_BEDROCK`、`CLAUDE_CODE_USE_VERTEX` 或 `CLAUDE_CODE_USE_FOUNDRY` 的值非空，也会跳过文件检查。

例外：前台 CLI 审批期间通过 SDK 生成标题时，包括 `cruise plan --skip-planning`，仅使用继承的进程环境和进程工作目录。因此，其过期预检会忽略工作流级 `env`，包括 `CLAUDE_CONFIG_DIR` 和替代身份验证变量。仅在工作流中设置的 API 密钥不会绕过此标题生成检查；如果继承的 OAuth 凭据被确定已过期，审批就会停止。

只有在访问令牌已过期，并且刷新令牌不存在或其记录的过期时间早于或等于当前时间时，Cruise 才会停止。如果访问令牌已过期，但存在刷新令牌且刷新令牌的过期时间未知，则会放行且不发出警告，将自动刷新交由后端处理。此检查仅读取文件：不会调用 auth-status CLI、联系提供商、刷新令牌、写入凭据或获取凭据锁。要重新登录，请根据具体情况使用 `claude auth login`、`jcode login claude` 或 `jcode login openai`。

这是过期预检，并不能证明身份验证一定会成功。它无法检测刷新令牌在服务端被撤销的情况，也不会读取 jcode 的刷新失败状态或 macOS Keychain 凭据。不会考虑步骤级 `env` 中的 API 密钥。要绕过误报，请设置进程环境变量 `CRUISE_SKIP_AGENT_AUTH_CHECK=1`；只有精确值 `1` 才会跳过检查，`true` 或前后带有空白字符的值均无效。

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```


在所有 SDK 后端中，`model` / `plan_model` / 每步的 `model` 都是**模型引用**，其覆盖优先级与命令模式相同（步骤 `model` > 顶层 `model` / `plan_model`）。对于 jcode 和 Claude，可选的 `:effort` 后缀用于选择推理强度级别（`low` / `medium` / `high` / `xhigh` / `max`，另有别名 `minimal` / `min` / `med` 以及数字写法 `1`..`4`）；`off` / `none` / `0` / `5` 也会被解析并移除，但不会设置推理强度，其他任何 `:` 后缀（例如 OpenRouter 的 `:free` 变体）则仍是模型 ID 的一部分。对于 jcode/Claude，如果提供商或模型不支持某个推理强度，该强度设置会被忽略。OpenCode 则会依据其目录校验推理强度，并在不支持时明确报错（见下文）。

在工作流级别，`model` 和 `plan_model` 也可以是数组。在 SDK 模式下，第一个条目是主模型，后续条目构成隐式回退链。包含回退条目的模型数组会自动启用模型回退；针对主模型显式设置的 `retry.fallback_chains` 条目优先于数组中的后续条目。当 `--rate-limit-retries` 大于零且尚未流式输出任何可见文本时，遇到 429、5xx、除 429、401、403、407 和 408 之外的 HTTP 4xx，或网络故障，都会立即切换到可用的回退模型（只有在没有剩余可用回退模型时，429 才会在同一模型上重试）；遇到此类 4xx 时，绝不会向同一模型重发请求，因为完全相同的请求仍会以相同方式失败。401、403 和 407 表示身份验证或权限失败，换用其他任何模型也无法解决，因此会立即使该步骤失败，而不会沿回退链逐一尝试。408 是传输超时，归类为网络故障，因此采用与 5xx 相同的立即切换路径：先切换到可用的回退模型，只有在没有剩余可用回退模型、已经流式输出了可见文本，或 `--rate-limit-retries` 为 `0` 时，才会向同一模型重发请求。因 429、5xx 或网络故障（包括 408）而跳过的模型，会在当前进程中进入 30 分钟的冷却期；被提供商报告为缺失的模型（`400 model_not_supported`、`404 model_not_found`）也会如此，因为在这种情况下有问题的是模型，而不是请求；只有因普通客户端错误切换而被换掉的模型不会进入冷却期，因为该状态描述的是请求，而不是模型的健康状况。在命令模式下，仅使用第一个条目，原有的同模型重试行为保持不变。

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


- Claude 会设置 `CLAUDE_CODE_SUBAGENT_MODEL` 和
  `CLAUDE_CODE_SUBAGENT_MODEL_FORCE=1`，将所有子智能体（包括 Explore 和 Plan）的模型固定下来。
  注入的这些值会覆盖工作流/步骤 `env:` 中的同名键。
  未设置 `subagent_model` 时，Cruise 不会改动这些环境键。
- jcode 会在提示执行前设置会话的 swarm 默认值。工作智能体显式指定的
  `model` 仍具有更高优先级，因此这是对默认值的覆盖，而非强制设置。
- 不支持 OpenCode 子智能体模型和 `command:` 后端。混合 SDK
  工作流可以通过显式 SDK 对象选择 Claude/jcode 候选项。
  可识别的 `:effort` 后缀会被拒绝，即使是 `:off` 也不例外；`:free` 等未知后缀
  仍是模型 ID 的一部分。jcode 要求模型有效且非空。
- 适用于工作流提示步骤，包括组、`after-pr`、并行
  提示子步骤和 `github-review`，以及规划过程中的 plan/fix/ask 轮次。
  不适用于标题生成、PR 元数据、`workflow generate` 或 `workflow make`。
  非提示步骤以及组/工作流调用/并行包装层均不能设置它。
- 被调用工作流的顶层值会被忽略，改用调用方的
  默认值；被调用方的步骤级覆盖值会保留。
- 分层的 `default.yaml` 和 `CRUISE_SUBAGENT_MODEL` 支持此设置，但应用程序
  `config.json` / `config.yaml` 中的默认值不支持。

<a id="sdk-jcode--the-jcode-sdk-default"></a>
### `sdk: jcode` — jcode SDK（默认）

`sdk: jcode` 使用官方 jcode Rust SDK，要求 jcode v0.88.0 或更高版本。启动时，Cruise 会验证 SDK 握手和所需能力（`sessions`，以及在注册了自定义工具时所需的 `session_tools`）；不会探测 CLI 版本。如果缺少所需能力，请升级 jcode。`model`、`plan_model` 和步骤级 `model` 字段接受 `provider/model[:effort]` 或裸模型。每次实际执行的 jcode 提示都必须解析出模型：普通提示先使用步骤 `model`，然后回退到工作流/默认 `model`；规划调用先使用 `plan_model`，然后回退到 `model`。已配置的 jcode 默认值、`JCODE_MODEL` 或恢复的会话都不能替代缺失的 Cruise 模型。缺少模型时，会创建或保留用户的 `default.yaml` 模板，并停止该请求。推理强度后缀通过 SDK 应用。

每次 SDK 运行都使用一个私有会话主目录，其中包含继承的登录记录和复制的
模型目录。运行时的 `JCODE_HOME` 指向实际的会话目录；
在 Unix 上，较短的符号链接别名仅用于 SDK 套接字路径。这满足了
jcode 的账户存储安全检查要求，同时不会改变已保存会话的位置或
恢复行为。

<a id="computer-use"></a>
#### 计算机使用

工作流级 `computer_use` 默认为 `false`。将其设为 `true`，可允许提示轮次在 macOS 上使用 jcode 的 `macos_computer_use` 桌面控制工具。提示步骤可以设置 `computer_use: true` 或 `false` 来覆盖工作流默认值。内置的 plan、fix-plan 和 ask-plan 轮次遵循工作流值；标题和 PR 描述生成始终在关闭计算机使用功能的情况下运行。该工具仅支持 macOS，此设置在 Linux 上无效。每个实际生效且适用的模型候选项都必须使用 jcode，包括显式指定 SDK 的对象以及由起决定作用的重试链选出的候选项，而且不得设置任何 `command:` 后端。显式限定为仅使用 jcode 的候选项，也可以在工作流 SDK 为 Claude 或 OpenCode 时使用。仅更改工作流 SDK 不会覆盖不兼容的显式指定 SDK 的对象。

关闭计算机使用功能时，Cruise 会将 `macos_computer_use` 添加到 `JCODE_DISABLED_TOOLS`，并与依次从工作流 `env:`、Cruise 进程环境或私有副本 `config.toml` 中的 `[tools].disabled` 获取的首个可用列表合并。此设置会传递到私有运行时的守护进程，包括 swarm 工作智能体和子智能体。当计算机使用功能已开启、拥有完全权限且未显式拒绝 web/subagent 权限时，Cruise 会保持 `JCODE_DISABLED_TOOLS` 不变，绝不会为了强制启用该工具而移除 jcode 级别的禁用设置。用户必须授予 macOS 辅助功能和屏幕录制权限。

<a id="permission-modes"></a>
## 权限模式

工作流级的 `permission` 默认为 `full`。提示步骤可以通过设置 `permission: read-only | edit | full` 来覆盖此默认值。`full` 保持当前行为。`edit` 禁用 shell 执行（Claude 的 `Bash`、jcode 的 `bash`），但保留直接编辑、写入和打补丁的工具。`read-only` 还会禁用文件修改工具（Claude 的 `Edit`/`Write`/`NotebookEdit`，jcode 的 `edit`/`write`/`apply_patch`/`replace`）。受限模式会强制关闭 `macos_computer_use`，即使设置了 `computer_use: true` 也是如此；Claude 会以 `dontAsk` 运行这些模式，因此不会有权限提示阻塞无人值守的运行。Jcode 中按模式禁用的工具会合并到选定的 `JCODE_DISABLED_TOOLS` 列表中。

- 使用 `command:` 后端时，会拒绝 `read-only` 和 `edit`，因为 cruise 无法控制任意 CLI 的工具。
- 将 `read-only` 与 `allow_commit: true` 组合使用属于配置错误。`allow_commit: true` 仅移除 HEAD/ref 提交防护和隐式 Git 禁止规则，不会移除显式限制，也绝不会放宽权限模式。
- 分组调用、`workflow_call` 或 `parallel` 包装层均不能设置 `permission`。请在内部提示步骤或并行提示子步骤上设置它。含有任何只读子步骤的并行块都只能包含只读提示子步骤。
- `workflow_call` 被调用方的顶层 `permission` 会被忽略（采用调用方的默认值），但会保留被调用方内部的逐步骤覆盖设置。
- 如果只读步骤执行期间工作区快照发生变化，该步骤还会失败并报告 `ReadOnlyWorkspaceChanged`。这是变更检测，不是操作系统沙箱或回滚机制，并且会忽略 `.git`、`target` 和 `node_modules`。
- 内置的规划、修复/询问、标题、PR 描述和工作流生成轮次始终以 `full` 运行，不使用 `disallowed_tools` 或隐式 Git 限制。

<a id="tool-restrictions"></a>
## 工具限制

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

后端的强制执行机制与局限：

- **Claude：** 原生禁止规则将 `web` 映射到 `WebFetch`/`WebSearch`，将 `subagent` 映射到 `Agent`，并将 shell 前缀映射到 `Bash(...)`。使用 `bypassPermissions` 时，禁止规则仍然有效。Claude 的原生匹配**不会**规范化 Git 全局选项，因此 `git -C path commit` 可能无法匹配 `git commit`。
- **jcode：** 将 `webfetch`、`websearch`、`browser` 和/或 `swarm` 合并到 `JCODE_DISABLED_TOOLS`，同时保留用户条目。shell 规则使用 Cruise 的工具调用前置检查关卡，该关卡会追加到现有钩子中；现有钩子依次从工作流环境变量、进程环境变量、配置中的 `[hooks].pre_tool` 中选取。工作流或进程环境中的 `JCODE_HOOKS_DISABLED` 会禁用该关卡，并产生警告。关卡设置失败也会产生警告。该关卡通过以 `2` 退出来阻止操作，包括策略/输入格式错误的情况；在 jcode 中，钩子执行失败或返回其他退出码时会放行。较早版本可能不支持组合现有钩子所需的数组形式。
- **OpenCode：** 原生禁止规则与 Cruise 插件/桥接层会在执行前禁止 Web 工具、`task` 委派和匹配的 bash 输入。
- **jcode/OpenCode 的 shell 匹配：** 会拆分复合命令，移除开头的赋值和常见包装命令（`env`、`command`、`builtin`、`exec`、`nohup`、`nice`、`time`、`timeout`、`stdbuf`、`xargs`），检查 `sh`/`bash`/`zsh`/`dash` 的 `-c` 脚本，比较可执行文件的基名，并规范化 Git 全局选项。Heredoc 主体中的行可能导致误拒绝。这不是完整的 shell 解释器，也不是安全边界。

MCP 限制仅在后端启动前筛选由工作流提供的条目。后端的用户级 MCP 配置仍保持启用且不受影响，Cruise 自身的会话工具也是如此。这些控制措施属于纵深防御，并非沙箱；提示中的警告、reference-transaction 钩子和 CAS 回滚仍然保留。

**升级说明：** 在 macOS 上，较早版本的 Cruise 会隐式向每个 jcode 提示提供 `macos_computer_use`。设置工作流级的 `computer_use: true` 即可保留该行为。

GitHub Action 的 shell 引导过程仍保留 `jcode version --json`、`jcode login`、`jcode provider add`，以及用于查看状态的 `jcode auth status`：该引导过程没有 SDK 客户端，而 SDK 既不提供 CLI 版本字符串，也不提供这些账户/配置档案设置操作。Cruise 后端本身则在启动后使用 SDK 能力握手。

Cruise 会将进程的 `JCODE_HOME` 解析为源主目录，回退路径为 `~/.jcode`。每个 Cruise 会话都会获得一个继承凭据和配置的隔离 SDK 主目录；恢复执行的规划轮次会复用该目录。私有运行时会接收 `JCODE_CHECK_UPDATES=0`，而不会改写复制的配置。规划用主目录会一直保留，直到其所属的 Cruise 会话被删除或清理；不可恢复的提示所用的主目录则会在每个轮次后移除。无人认领的主目录只会在 24 小时后被清理，而且必须由跨进程锁证明没有活跃运行。在移除私有主目录之前，Cruise 只会停止仍在运行、且其命令和 `JCODE_HOME`/`JCODE_RUNTIME_DIR`/`JCODE_SOCKET` 能够标识该运行时的 `jcode serve` 进程；它绝不会仅以注册表中的 PID 为依据。源主目录的绝对路径会随可恢复的规划会话一起保存，因此即使从其他目录启动 Cruise，删除操作也仍能正常进行。
模型目录缓存会被复制到该私有主目录中；与路径绑定的外部凭据会被复制为仅所有者可访问的文件，而不是建立链接，且只继承此前为原始路径记录的信任决策。

`ask_user` 会使其待回答的问题在超过 jcode 的 120 秒 SDK 回调时限后仍保持有效。约 110 秒未收到回答后，该工具会返回一条成功状态的继续通知，要求模型以完全相同的问题再次调用 `ask_user`；它不会再次向用户提问，而在两次调用之间收到的回答会在下一次调用时返回。等待时长仍受工作流步骤超时限制。当其私有 jcode 主目录中缺少已保存的规划对话时，Cruise 会启动新的 SDK 会话，因为提示中已经包含当前计划。

`submit_plan` 和 `update_plan` 只会写入计划并返回；Cruise 现有的审批流程会在模型轮次结束后进行，不会让 SDK 工具回调持续保持打开状态。

**升级说明：** 从使用 MCP 桥接的版本升级后，请从源配置文件 `$JCODE_HOME/mcp.json`（未设置 `JCODE_HOME` 时为 `~/.jcode/mcp.json`）中移除过时的 `mcpServers.cruise` 条目。Cruise 会从每个私有会话副本中剔除该条目，并保持源配置不变。在移除该条目之前，Cruise 之外的 jcode 会话仍会继续启动它。Cruise 会向 stderr 输出迁移说明并退出，而不是将其当作旧版计划输入处理，因此 jcode 会报告 `cruise` MCP 服务器失败。

该后端通过 jcode 的 session-tools 支持来提供 Cruise 会话工具。

<a id="sdk-claude--the-claude-cli-in-process"></a>
### `sdk: claude` — 在进程内运行的 claude CLI

`sdk: claude` 通过 claude-agent-sdk 在进程内驱动 `claude` CLI。模型引用使用普通的 `claude --model` 模型名称，可带有可选的 `:effort` 后缀，该后缀会通过 `--effort` 转发。不支持该选项的 `claude` CLI 会导致步骤失败，错误为 `unknown option '--effort'`；此错误被归类为永久性错误，绝不重试。Cruise 已针对 2.1.250 完成验证。身份认证由 claude CLI 自行处理，使用其已保存的凭据或 `ANTHROPIC_API_KEY`，与 `jcode login` 无关。CLI 运行时会绕过权限检查，因为 Cruise 工作流无人值守，没有控制台可用于响应权限提示。Cruise 的工作流工具通过 SDK 暴露。`computer_use: true` 会在加载配置时被拒绝，因为 Claude Code 的计算机使用功能仅在交互式会话中可用，而 Cruise 无法启用此功能。

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

Cruise 内置的是面向工作流所需的 OpenCode V2 API 的 Rust 客户端，而非 Python/Node SDK 依赖，也非特定提供商的 LLM 客户端。请显式使用 `cruise install opencode` 安装 OpenCode CLI，或另行安装，并完成身份认证。Cruise 自行管理一个短生命周期、使用回环地址的 `opencode serve --stdio` 进程，并在每次尝试后停止该进程。工作流执行时不会连接外部服务器、安装 CLI、代你完成身份认证，或回退到 V1。

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


为工作流、规划和步骤指定模型时，请使用 `provider/model[:effort]` 或原生的 `provider/model#variant` 格式，这也适用于数组候选项和恢复的会话。通用 effort 名称及其别名不区分大小写：`low`（`min`/`minimal`/`1`）、`medium`（`med`/`2`）、`high`（`3`）、`xhigh`（`4`）和 `max`。Cruise 会根据 OpenCode 的实时模型目录校验请求的级别，并选择匹配的变体，绝不猜测或降级。不受支持或有歧义的 effort、未列入目录的模型，以及不对应任何级别的后缀 `:off`/`:none`/`:0`/`:5`，都会在发送提示或更改恢复会话的模型之前明确报错，且不会回退。通用 effort 与原生 `#variant` 互斥。对于原生变体（包括 `#none`）、不带后缀的模型，以及未指定模型的情况，均保持现有行为。

模型数组沿用现有的失败驱动回退策略。真正的 OpenCode `provider.no-route` 执行失败（包括 `Model unavailable`）会被视为模型缺失，并立即切换到已配置且可用的回退模型，同时沿用现有预算和冷却机制。一般性的不可用提示文本、其他类型的失败、附加的 stderr，以及身份认证/权限失败或无效请求失败，均不适用这种特殊处理。可见输出和重放屏障仍会阻止切换。一旦工具已产生副作用、无法确定提示是否已被接受，或发生不可恢复的事件丢失，即使未显示任何文本，OpenCode 也禁止自动重试或回退。未指定模型时，会交由 OpenCode 使用其配置的默认模型。工作流和步骤的 `env:` 值会连同继承的操作系统环境和提交保护机制一起传递给提示工具。仍会使用现有的 OpenCode 全局/项目配置和身份认证。Cruise 将自己的数据库、插件和可恢复的规划对话保存在私有 XDG 存储中，而不是项目中或用户的 OpenCode 数据库中。它不会改写用户配置。对于无效的 `OPENCODE_CONFIG_CONTENT`，会给出诊断信息，而不是将其丢弃。

Cruise 的规划、提问、标题生成和 PR 元数据功能通过原生 V2 插件使用现有工具，而不是通过 MCP 服务器。明确不支持 `computer_use: true`。`permission: edit` 允许直接编辑文件，但禁止使用 shell、子智能体和未知的执行工具。`read-only` 还会禁止修改操作，同时保留安全的 Cruise 工具和工作区快照检查。在受限模式下，未知的 MCP 和 Code Mode 执行路径均被禁止。这些是工具层面的限制，并非操作系统沙箱。

工作流 MCP 条目支持本地 `stdio` 和远程 Streamable HTTP（`type: http`）。旧版 `type: sse` 会被拒绝，并给出指明服务器名称的诊断信息，绝不会静默转换。无人值守运行期间不会启动 OAuth 登录流程。工作流条目会以叠加方式应用到 OpenCode 配置中，不会更改源文件。

协议基线为 OpenCode 提交 `74f8d4860f49c563909e6b039b0cc8b454ebce72`（调研日期为 October 23, 2026）。OpenCode v2.1.3 被确定为首个使用真实 CLI 进行验证的目标版本，这并不是对兼容性的全面保证。仅凭运行时 V2 能力检查和基于测试夹具的测试，无法证明与真实模型或插件的互操作性。

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

使用 jcode 时，Cruise 会将工作流条目合并到 `$JCODE_HOME/mcp.json` 的私有会话副本中（未设置 `JCODE_HOME` 时使用 `~/.jcode/mcp.json`）。工作流条目会替换来自该源文件的同名条目，源文件本身绝不会被修改。随后 jcode 会应用其正常的配置合并顺序，因此来自 `~/.claude.json`、`~/.claude/mcp.json` 或项目本地的 `.jcode/mcp.json`、`.mcp.json` 和 `.claude/mcp.json` 的同名服务器会优先于工作流副本。jcode 会使用运行时环境（包括工作流级 `env:` 值）来展开 `${VAR}` 和 `${VAR:-default}`。

使用 `sdk: claude` 时，工作流 MCP 条目会被写入每次运行专用的私有 JSON 文件，在 Unix 上权限模式为 `0600`（在 Windows 上使用仅允许所有者访问的 ACL），文件路径通过 `--mcp-config` 传递；SDK 会将 Cruise 的进程内工具单独作为其自身的内联 `--mcp-config` 值传递。用户自己的 Claude MCP 配置仍保持启用。不保证会对这些 Claude 条目进行 `${VAR}` 展开。
