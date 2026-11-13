[English](../en/github-actions.md) | [日本語](../ja/github-actions.md) | [简体中文](../zh/github-actions.md) | [索引](README.md)

<a id="github-actions"></a><a id="cruise-github-action"></a>
# cruise GitHub Actions

在 GitHub Issue 中提及 `@cruise`（可配置），即可让 cruise 在 GitHub Actions 中规划并实现任务；如果成功运行留下了分支提交且启用了自动创建 PR，Action 就会打开草稿 PR。这与 [`anthropics/claude-code-action`](https://github.com/anthropics/claude-code-action) 的理念类似，但它通过 `sdk: jcode`（除非解析后的工作流选择了其他后端）驱动 cruise 自己的 plan -> implement -> optional PR 工作流；Action 会安装 `jcode` CLI 并为其配置凭证。默认配置无需额外设置即可使用 `sdk: jcode`，但 Action 不会强制指定后端：如果仓库配置设置了 `command:`、`sdk: claude` 或 `sdk: opencode`，仍以该设置为准，并且你需要自行提供该后端的 CLI 和凭证。

此 Action **不支持 PR 模式**：它只响应 `issues`（opened）和 `issue_comment`（created）事件。PR 上的评论始终会被忽略（PR 在 GitHub API 层面属于“issue”，但此 Action 会明确排除它们）。

> 注意：受限提示词步骤（`permission: read-only | edit`）会在无人值守模式下运行，工具会被拒绝而不是弹出确认提示；只读步骤如果更改了工作区就会失败。

<a id="quickstart"></a>
## 快速开始

1. 在仓库中安装 [`cruise-agent` GitHub App](https://github.com/apps/cruise-agent/installations/new)（可选；跳过后将回退到工作流的 `GITHUB_TOKEN`，参见[身份验证的工作方式](#how-authentication-works)）。
2. 在仓库密钥中添加 `ANTHROPIC_API_KEY` 和/或 `OPENAI_API_KEY`（Settings -> Secrets and variables -> Actions）。jcode 至少需要其中一个。
3. 将 [`examples/cruise.yml`](../../examples/cruise.yml) 复制到 `.github/workflows/cruise.yml`。

然后打开一个 Issue，或在现有 Issue 下评论 `@cruise plan <你想做的事>`。完整的 plan -> fix -> run -> review 循环参见[典型工作流](#typical-workflow)；所有提及格式参见下方[命令](#commands)；若要使用 Anthropic/OpenAI 以外的提供方，请参见[提供方](#providers)；详细配置流程（完整工作流 YAML、`permissions:`、版本固定）参见[设置](#setup)。

<a id="commands"></a>
## 命令

触发短语之后，解析器会跳过开头的空白和标点，然后由第一个词元（可带或不带开头的 `/`，不区分大小写；匹配前会去掉 `.`/`,`/`:` 等结尾标点）决定执行什么操作。词元可以位于下一行。`plan`、`fix`、`exec` 和 `run` 会选择对应命令；`/tell` 必须带斜杠。没有词元或词元未知时，默认执行 `run`。对于 `issues` 事件，触发短语会在拼接后的 Issue 标题和正文中匹配（标题在前），因此短语若出现在标题末尾，命令词元可能取自正文第一行。

| 提及格式 | 命令 | 执行内容 |
|---|---|---|
| `@cruise`、`@cruise run <request>`、`@cruise /run` | **run** | 解析计划（见下文；`run` 后输入的任何文本都会作为额外指令追加），按原样从计划创建会话（不调用 LLM 进行规划），并在 worktree 中执行。默认情况下，如果成功运行留下了分支提交，就会打开草稿 PR；成功运行但没有提交时，则结束而不创建 PR。若工作流设置了 `create_pr: false`，运行也会结束，但不会自动提交、推送、创建 PR 或运行 `after-pr`。只有在 `cruise run` 成功退出，且所选会话状态为 `Completed` 并带有 Cruise 记录的跳过原因（`disabled` 或 `no-commits`）时，Action 才会将空 PR 结果视为成功；没有明确原因而缺少 PR URL 仍然属于失败。已配置的 `if.no-file-changes` 失败/重试行为仍然生效。 |
| `@cruise exec <request>`、`@cruise /exec` | **exec** | 解析计划（与 `run` 相同，也会追加额外指令），然后直接在已检出的默认分支上运行，并**直接推送到该分支**（不创建 PR）。这是高级/选择启用的功能，参见 [`exec` 注意事项](#exec-caveats)。 |
| `@cruise plan <request>`、`@cruise /plan` | **plan** | 针对 Issue 标题和正文运行 LLM 规划调用（`cruise plan`），并将结果作为新的**计划跟踪评论**发布。不执行任何操作。（目前，触发评论中 `plan` 后输入的文本不会被纳入计划；只有 Issue 标题和正文会作为计划输入。） |
| `@cruise fix <feedback>`、`@cruise /fix <feedback>` | **fix** | 使用 `<feedback>` 修改最近一条*可信*的计划跟踪评论，然后**在原评论中直接更新**修改后的计划。如果不存在已有计划评论，会显示明确错误并失败。 |
| `@cruise /tell <instruction>` | **tell** | 向此 Issue 上正在运行的 `run` 作业发送指令。仅适用于 Issue 评论；不会启动运行。 |

**计划解析**（`run` 和 `exec` 使用）：Action 会检查 Issue 的所有评论，查找最新一条包含 `<!-- cruise:plan -->` 标记、且由 **bot 账号**（`user.type == "Bot"`）发布的评论；该账号的登录名还必须属于固定的允许列表（`cruise-agent[bot]` 或 `github-actions[bot]`）。由任何其他身份（包括 PAT 或自定义 App token）发布的评论都不会被复用，解析会回退到 Issue 自身的标题和正文。如果存在可信的计划评论，就使用其中的计划内容，否则使用 Issue 自身的标题和正文。无论哪种情况，在触发本次运行的评论/Issue 中命令词之后输入的文本，都会作为“## Additional instructions from the triggering comment”一节追加。

计划跟踪**评论正文**最多为 60,000 个字符（GitHub 自身的上限为 65,536）。只有计划内容会被截短——末尾会替换为截断说明，并且始终会重新附加回复提示页脚，不会悄悄丢弃。该上限只应用于已发布的评论：执行时读取的是未截短的 `plan.md`，所以截断不会改变 agent 实际运行的内容。

<a id="command-grammar"></a>
### 命令语法

命令解析刻意采用严格、机械的规则，而不是自然语言理解：

- 跳过开头空白和 `.,!?;:` 标点后，会检查**第一个以空白分隔的词**是否为 `run`/`exec`/`plan`/`fix`（可带 `/` 前缀）或 `/tell`（必须带斜杠）；不区分大小写，并会去掉结尾的 `.,!?;:`。开头空白可以包括换行。为保持向后兼容，单独使用 `@cruise tell ...` 仍会被视为 `run` 请求。
- 如果正文中有**多个 `@cruise` 提及**（例如引用回复中包含此前的消息），只使用**最后一个**，因此回复旧提及不会重新触发旧命令。
- 由于匹配完全基于词面，提及后恰好以命令词开头的普通英语句子也会被解析成该命令。例如，`@cruise fix the flaky test` 会被解析为带反馈 `the flaky test` 的 **`fix` 命令**，而不是自由格式的 `run` 请求。如果还没有现有计划评论，就会失败并提示先运行 `@cruise plan`，而不会悄悄改做其他事情。
- 为避免这种歧义，自由格式请求建议使用明确的斜杠形式（`@cruise /run <request>`、`@cruise /exec <request>`）；只有确实要运行 `plan`/`fix` 命令时，才使用不带斜杠的形式（`@cruise plan ...` / `@cruise fix ...`）。

<a id="typical-workflow"></a>
## 典型工作流

下面是一个先规划再执行的端到端示例：

1. **创建 Issue**描述任务。不要在标题或正文中写入触发短语（`@cruise`）——下方警告会解释原因。

   > **为上传器添加重试逻辑**
   >
   > S3 上传偶尔会在网络不稳定时失败。应采用指数退避重试，而不是让整个作业失败。

2. **请求计划**：评论 `@cruise plan`。Action 会调用 LLM 生成计划并发布新的计划跟踪评论；此时还不会执行任何操作。

3. **按需反复修改计划**：评论 `@cruise fix 另外添加一条变更日志记录，并覆盖超时情况`。Action 会在同一条跟踪评论中直接更新修改后的计划。

4. **执行计划**：评论 `@cruise run`。Action 会根据（可能已修改的）计划创建会话，并在 worktree 中实现。如果成功运行留下了分支提交且启用了自动创建 PR，Action 就会打开草稿 PR；如果运行成功但没有提交，或者设置了 `create_pr: false`，则不会创建 PR。

5. **审查并标记为可审查**：若创建了 PR，无论由哪个命令创建，Cruise 始终会将其作为**草稿**打开。打开 PR 并审查差异，然后将其标记为“Ready for review”（或运行 `gh pr ready <number>`）再合并。成功但没有提交或没有创建 PR 的运行无需审查 PR。

> [!WARNING]
> 如果 Issue 自身的标题或正文已经包含触发短语，`issues: [opened]` 事件会立即触发。开头空白和标点之后的第一个词元（即使位于换行之后）会选择 `plan`、`fix`、`exec` 或 `run`；只有缺少词元或词元未知时才默认执行 `run`。如果希望先审查计划，请勿在 Issue 自身内容中写入触发短语，只在后续评论中输入。

如果不需要审查步骤，这种立即运行行为也可作为快捷方式：当 Issue 标题/正文中的触发短语后面跟着 `run`（或没有词元，或词元未知而默认执行 `run`）时，会完全跳过规划调用——Issue 的标题和正文会直接作为计划，并且触发短语之后的任何文本都会作为额外指令追加。

同一 Issue 上的多条提及会排队执行，而不是互相竞争：先后评论 `plan`、`fix`、`run` 时，仍会按顺序一次执行一个（参见 [`examples/cruise.yml`](../../examples/cruise.yml) 中的 `concurrency:` 块），因此无需等每次运行结束就可以继续评论。

`run` 作业活动期间，其作业内的轮询器每约 15 秒检查一次已获授权的 Issue 评论，这与排队等待触发的作业相互独立。被接受或拒绝的评论会收到眼睛表情回应；被接受的评论文本会排队供下一个 agent 提示使用，而不会进入当前正在运行的提示。作业关闭前还会进行最后一次扫描。`exec`、规划和修改作业不会运行此轮询器。协作者权限和 `allowed_bots` 规则同样适用，来自 GitHub 的文本会经过清理。消息投递遵循 [`cruise tell`](cli-reference.md#cruise-tell) 的相同限制和至少一次投递注意事项。

**tell 没有响应？** 检查评论是否使用了 `/tell`、作者是否有权限，以及该 Issue 是否有正在运行的 `run` 作业。请等待轮询间隔，并检查作业日志和表情回应。排队中的触发作业本身无法访问另一个作业的进程；运行结束后到达的评论可能会被拒绝。眼睛表情表示评论已处理，不一定意味着它已投递给 agent 提示。

<a id="setup"></a>
## 设置

1. **在仓库中安装 `cruise-agent` GitHub App**：[github.com/apps/cruise-agent/installations/new](https://github.com/apps/cruise-agent/installations/new)。这样 Action 就能以受限 bot 身份（`cruise-agent[bot]`）进行身份验证，而不是使用工作流自身的 `GITHUB_TOKEN`——详情参见下文[身份验证的工作方式](#how-authentication-works)。你可以跳过此步骤；Action 仍会运行，但会回退到 `GITHUB_TOKEN`，并受该部分所述限制影响。
2. 在仓库密钥中添加 `ANTHROPIC_API_KEY` 和/或 `OPENAI_API_KEY`（Settings -> Secrets and variables -> Actions）。至少需要其中一个。
3. 将 [`examples/cruise.yml`](../../examples/cruise.yml) 复制到 `.github/workflows/cruise.yml`（下方是精简摘录；`examples/` 中的文件会在每个部分上方用一行注释解释其用途）：

   ```yaml
   name: Cruise

   on:
     issue_comment:
       types: [created]
     issues:
       types: [opened]

   jobs:
     cruise:
       if: |
         (github.event_name == 'issue_comment' && !github.event.issue.pull_request && contains(github.event.comment.body, '@cruise')) ||
         (github.event_name == 'issues' && (contains(github.event.issue.title, '@cruise') || contains(github.event.issue.body, '@cruise')))
       runs-on: ubuntu-latest
       # write-tests -> implement each run a full verification pass; 30
       # minutes (a common default) is too tight for larger repositories --
       # see the Troubleshooting FAQ below.
       timeout-minutes: 60
       permissions:
         contents: write
         pull-requests: write
         issues: write
         id-token: write # needed for the cruise-agent App token exchange; optional
       steps:
         - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7
           with:
             fetch-depth: 0
         - uses: smartcrabai/cruise@v1
           with:
             anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
   ```

4. 打开一个提及 `@cruise` 的 Issue，或在现有 Issue 上评论 `@cruise <command> ...`。

工作流级别的 `if:` 只是粗略的预筛选（避免无关事件启动 runner）；Action 自身还会使用严格的单词边界匹配重新检查触发短语、验证评论者权限，并在执行任何操作之前拒绝 PR 评论。因此，即使移除此预筛选条件，仍然是安全的。

**最低 cruise 版本：v0.2.0**（第一个默认后端为 `sdk: jcode` 的版本；此 Action 生成的配置和凭证配置依赖这一点）；安装步骤会在二进制版本过旧时以明确错误拒绝继续。Action 也会安装 `jcode` CLI 本身（输入项 `jcode_version`，默认值为 `latest`）；Action 的凭证配置步骤会检查 v0.88.0 这一最低版本，而 Cruise 会在运行时验证 SDK API 握手和所需能力。默认的 `cruise_version: latest` 已满足此要求；若要确保安装可复现，请固定明确的标签版本。

<a id="providers"></a>
## 提供方

此 Action 会配置 cruise 的默认后端 `sdk: jcode`（见下文[执行方式](#sdk-jcode----how-execution-works)）；如果仓库配置指定了其他后端，则继续使用该后端。jcode 内置数十种提供方——每种都只需提供 API 密钥，可通过以下三种方式之一传入：

- **专用输入项**——`anthropic_api_key` / `openai_api_key` 会变成 jcode 的 `anthropic-api` / `openai-api` 凭证（通过 `jcode login <provider>`，并从 stdin 传入密钥），存入 Action 为此作业指定的 jcode 主目录。
- **`env`**——其他提供方的密钥（例如 `KIMI_API_KEY`、`GROQ_API_KEY`、`OPENROUTER_API_KEY`）会导出到运行环境中，由 jcode 从环境变量读取。
- **`providers` + `provider_api_keys`**——对于 jcode 尚不认识的 OpenAI 兼容端点（自托管网关、代理等），每个 `providers` 条目都会通过 `jcode provider add` 写入同一个主目录，成为 `[providers.<name>]` 配置档案：

```yaml
with:
  providers: |
    {
      "my-gateway": {
        "base_url": "https://gateway.example/v1",
        "model": "my-custom-model",
        "context_window": 200000,
        "default": true
      }
    }
  provider_api_keys: |
    my-gateway=${{ secrets.MY_GATEWAY_KEY }}
```

配置档案的密钥会传给 `jcode provider add --api-key-stdin`；该命令会将密钥存储在作业 jcode 主目录下仅所有者可读的环境文件中。`config.toml` 只通过环境变量名称引用密钥，不会将任何密钥写入 cruise 配置文件。

<a id="provider-profile-schema"></a>
### 提供方配置档案的结构

`providers` 的每个键都是一个配置档案名称（由 ASCII 字母、数字、`-` 和 `_` 组成，以字母或数字开头，最多 64 个字符），映射到包含以下字段的对象：

- **`base_url`**（必需）——端点 URL。
- **`model`**（必需）——配置档案的默认模型 ID。
- **`context_window`**（可选）——数值。
- **`auth`**（可选）——`bearer`（默认值）或 `api-key`。
- **`auth_header`**（可选）——请求头名称；要求设置 `auth: api-key`。
- **`provider_routing`**（可选，布尔值）——适用于 OpenRouter 风格的网关。
- **`no_auth`**（可选，布尔值）——端点完全不需要凭证（例如本地代理、开发网关）。`no_auth` 条目不得出现在 `provider_api_keys` 中；如果所有 `providers` 条目都是 `no_auth`，则可以完全不设置 `provider_api_keys`。
- **`default`**（可选，布尔值）——将此配置档案设为 jcode 启动时默认使用的提供方/模型。最多只能有一个条目设置此项。

任何不在列表中的键都会被拒绝，并报告该键的名称——否则拼写错误可能会被悄悄丢弃。值的取值范围（URL scheme、`auth`/`auth_header` 配对等）由 `jcode provider add` 自身验证。

<a id="built-in-providers"></a>
### 内置提供方

| 提供方 | jcode CLI 凭证提供方 | 环境变量 |
|---|---|---|
| Anthropic | `anthropic-api` | `ANTHROPIC_API_KEY`（专用输入项 `anthropic_api_key`） |
| OpenAI | `openai-api` | `OPENAI_API_KEY`（专用输入项 `openai_api_key`） |
| Kimi for Coding | `kimi` | `KIMI_API_KEY`（通过 `env`） |
| Google Gemini | `gemini-api` | `GEMINI_API_KEY`（通过 `env`） |
| Groq | `groq` | `GROQ_API_KEY`（通过 `env`） |
| Mistral AI | `mistral` | `MISTRAL_API_KEY`（通过 `env`） |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY`（通过 `env`） |
| xAI (Grok) | `xai` | `XAI_API_KEY`（通过 `env`） |
| Moonshot AI | `moonshot-ai` | `MOONSHOT_API_KEY`（通过 `env`） |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY`（通过 `env`） |

此表中的标识符用于 CLI 凭证配置；`jcode login --help` 会列出接受的 ID。自定义端点配置档案通过 `providers` / `provider_api_keys` 和 `jcode provider add` 配置。

<a id="kimi-for-coding-example"></a>
### Kimi for Coding 示例

[Kimi for Coding](https://api.kimi.com/coding/) 使用 jcode CLI 凭证提供方 `kimi`，并通过 `KIMI_API_KEY` 进行身份验证。此示例通过 Action 配置该凭证。请通过 Action 的 `model` 输入项或仓库工作流 / `.cruise/default.yaml` 提供兼容的 Cruise 模型；仅设置 jcode 主目录的默认模型并不会选择 Cruise 模型。

```yaml
- uses: smartcrabai/cruise@v1
  with:
    # Set model to a route available for this credential, or configure it in
    # the repository workflow / .cruise/default.yaml.
    env: |
      KIMI_API_KEY=${{ secrets.KIMI_API_KEY }}
```

<a id="sdk-jcode----how-execution-works"></a>
## sdk: jcode -- 执行方式

此 Action 不设置 `CRUISE_SDK`，因此 Cruise 会从仓库工作流配置和适用的检出项目默认值中解析后端。生成的默认配置不会写入 `sdk`；应用 `.cruise/default.yaml` 后，仅当 `command` 和 `sdk` 均未设置时，Cruise 才会回退到 `sdk: jcode`。如果仓库工作流配置设置了 `command:` 或 `sdk: claude`，则继续使用该配置；必要时请提供相应后端自己的 CLI 和凭证。

此 Action 会安装 `cruise` 二进制（`cruise_version`）和 `jcode` CLI（`jcode_version`）。它会使用该 CLI 完成设置和凭证配置：`anthropic_api_key` 和 `openai_api_key` 会通过 `jcode login` 保存，每个 `providers` 条目都会通过 `jcode provider add` 写入。Action 会将源 `JCODE_HOME` 固定在 `$RUNNER_TEMP/cruise/jcode-home` 下。

执行 SDK 时，Cruise 会根据该源主目录创建独立的会话主目录，并继承其中的凭证和配置。计划主目录会一直保留，直到 Cruise 会话被删除或清理；普通的不可恢复提示词主目录则会在本轮结束后删除。崩溃后遗留的会话前主目录会在 24 小时后清理，并首先停止其所属守护进程。Action 的临时源主目录可避免将已配置的凭证放入持久化 runner 主目录。单独的 jcode 安装器主目录也会隔离在 `$RUNNER_TEMP` 下，包括安装器进程的 `HOME` 和 shell 配置。

- **后端**：只有在应用适用的检出项目默认值后，`command` 和 `sdk` 都仍未设置时，才使用 `sdk: jcode`。
- **身份验证**：可以使用专用密钥输入项、`providers` / `provider_api_keys`，或通过 `env` 提供的提供方密钥。如果所有受支持的凭证输入项都为空，Action 的前置检查就会失败；`no_auth` 配置档案不需要密钥。
- **OpenAI 服务层级**：如果工作流和进程环境均未定义 `JCODE_OPENAI_SERVICE_TIER`，默认值为 `off`；显式设置的值会被保留。
- **模型输入项**（`model` / `plan_model`）会覆盖对应的 Cruise 设置。每个实际的 jcode 提示都必须解析出有效的 Cruise 模型，格式为 `provider/model[:effort]` 或裸模型名。`model` 可来自 Action 输入项、仓库工作流或检出目录中的 `.cruise/default.yaml`；jcode 主目录默认模型和 `JCODE_MODEL` 不能替代它。`plan_model` 用于规划调用；未设置时回退到 `model`。可选的 effort 后缀支持 `low|medium|high|xhigh|max`，以及[SDK 模式](sdk.md#sdk-mode)中记录的别名。
- **自定义提供方**：通过 CLI 配置为 jcode 配置档案；其输入结构和凭证说明参见[提供方](#providers)。

<a id="zero-config-default"></a>
### 零配置默认值

如果 `config` 为空、没有根工作流配置，并且 `.cruise/` 下没有直接放置的 YAML 文件，此 Action 会生成一个 `write-tests -> implement` 配置，其中不包含 `sdk`、`model` 或 `plan_model` 字段。适用的检出项目默认值和 Action 模型输入项必须为 jcode 提示提供模型；只有当 `command` 和 `sdk` 都仍未设置时，Cruise 才会使用 `sdk: jcode`。

<a id="config-resolution"></a>
## config 解析

- **已设置 `config` 输入项**——解析为绝对路径并导出为 `CRUISE_CONFIG`。`run`/`plan`/`fix` 命令会使用它。
- **`config` 输入项为空，且仓库已有自己的配置**（检出根目录下的 `cruise.yaml`/`cruise.yml`/`.cruise.yaml`/`.cruise.yml`，或 `.cruise/` 下除顶层 `.cruise/default.yaml` 以外的工作流 YAML）——完全不设置 `CRUISE_CONFIG`，由 cruise 自己的解析器选取该文件，包括其中指定的任何后端（参见[上文](#sdk-jcode----how-execution-works)）。参见 [`examples/repo-cruise.yaml`](../../examples/repo-cruise.yaml)，了解可作为自有 `cruise.yaml` 提交的配置。
- **`config` 输入项为空，且没有根工作流配置，`.cruise/` 下也没有直接放置的 YAML 文件**——此 Action 会生成默认配置（`write-tests -> implement` 步骤，提示词逐字内嵌自此 Action 的 `prompts/write-test-first.md`/`prompts/implement-after-tests.md`，且不设置 `sdk`/`model`/`plan_model`），并将其导出为 `CRUISE_CONFIG`；应用适用的检出项目默认值后，只有当 `command` 和 `sdk` 均未设置时，Cruise 才会使用 `sdk: jcode`。参见[零配置默认值](#zero-config-default)。
- **`.cruise/default.yaml` 是 `.cruise/` 下唯一直接放置的 YAML，且没有根工作流配置**——此 Action 不设置 `CRUISE_CONFIG`。Cruise 会使用内置 `tdd` 工作流（`builtin/tdd.yaml`），并将 `.cruise/default.yaml` 用作项目默认值。这与前一种情况中 Action 生成的两步配置不同。
- **`exec` 始终使用自己的生成配置**，不受 `config` 或上述两种情况影响：该配置最精简，只包含一个 `implement` 步骤，其提示词为 `"{input}"`（同样不包含 `sdk`/`model`/`plan_model`）。`cruise exec` 会将完整计划文本绑定到 `{input}`，且不会运行规划步骤（`plan.md` 保持为空），因此基于 `{plan}` 的配置会悄悄收到空提示词。
- Action 始终导出 `CRUISE_FORCE_EXEC=false`，因此仓库的 `force_exec: true` 不会改变 Action 明确指定的 `run`/`plan`/`fix` 命令流程。

Action 只会使用检出目录 `.cruise/default.yaml` 中的项目默认值。隔离后的 XDG 配置不会加载用户的 `~/.config/cruise/default.yaml`。

无论哪种情况，`model`/`plan_model` 输入项（由 cruise 自身应用的 `CRUISE_MODEL`/`CRUISE_PLAN_MODEL` 环境变量覆盖）都优先于配置文件是否设置了这些字段。

不要在 CI 使用的配置中设置 `option:` 步骤——它会交互式提示，而 Actions 中没有连接的终端。

<a id="env-input"></a>
## `env` 输入项

使用 `env` 输入项将额外的环境变量传给 cruise 进程，每行一个 `KEY=VALUE`：

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    env: |
      MY_TOOL_TOKEN=${{ secrets.MY_TOOL_TOKEN }}
      FEATURE_FLAG=true
```

空行和以 `#` 开头的行会被忽略。每个值在导出前都会被遮蔽（`::add-mask::`）。以下保留名称会被跳过并输出 `::warning::`，而不是被覆盖，因为 Action 自身会管理它们：`GITHUB_TOKEN`、`GH_TOKEN`、`PATH`、`HOME`、`SHELL`、git 身份变量（`GIT_AUTHOR_*`/`GIT_COMMITTER_*`）、`JCODE_HOME`、`XDG_DATA_HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`，以及所有带有 `CRUISE_`/`GITHUB_`/`ACTIONS_`/`RUNNER_` 前缀的名称。只有上述四个前缀按前缀规则匹配；`JCODE_HOME` 和 XDG 条目是精确名称列表，因此 `XDG_CACHE_HOME` 或 `JCODE_OPENAI_SERVICE_TIER` 等无关变量仍会正常导出。（`CRUISE_*` 是前缀规则，并不只限于 `CRUISE_SDK`/`CRUISE_CONFIG`：请使用专用的 `model`/`plan_model`/`config` 输入项，而不要传入原始 `CRUISE_*` 变量。）`ANTHROPIC_API_KEY`/`OPENAI_API_KEY` **不是**这里的保留名称——可以通过 `env`（替代或同时搭配专用的 `anthropic_api_key`/`openai_api_key` 输入项）传入它们，以满足凭证检查。

<a id="exec-caveats"></a>
## `exec` 注意事项

`exec` 会**直接推送到默认分支**，不创建 PR，也没有审查步骤。这意味着：

- **可能与分支保护规则冲突。** 如果默认分支要求通过 PR、状态检查或审查后才能合并，cruise 的直接推送就会失败（并显示为失败运行）。`exec` 适用于有意允许直接推送到默认分支的仓库，或为 cruise 使用的 actor/token 配置了绕过规则权限的仓库。
- **这是高级/选择启用的功能。** 除了 `git revert` 之外没有撤销方法。除非你明确需要无人值守的直接推送，否则建议使用 `run`（它会打开可供审查的草稿 PR）。
- 如果 `cruise exec` 没有产生文件更改，则会完全跳过提交和推送步骤，并报告成功。

Action 始终导出 `CRUISE_FORCE_EXEC=false`；仓库配置无法悄悄将 `run`、`plan` 或 `fix` 改成直接执行。只有确定需要直接推送时才使用显式的 `exec` 命令。

<a id="how-authentication-works"></a>
## 身份验证的工作方式

默认情况下（`github_token` 输入项留空时），Action 会尝试使用 `cruise-agent` GitHub App 进行身份验证，而不是使用工作流自身的 `GITHUB_TOKEN`：

1. 授予 `permissions: id-token: write` 后，GitHub Actions 会向作业提供一个短期 OIDC token，用于标识工作流、仓库和运行。
2. `token` 步骤会调用 `token_exchange_url` 服务，通过该服务将 OIDC token 交换为**限定在单个仓库范围内的短期 cruise-agent App 安装 token**（使用 `POST`，请求头为 `Authorization: Bearer <OIDC token>`，无请求正文）。交换服务会在服务器端验证 OIDC token 的 `repository` claim，并且只会签发限定到该仓库安装范围的 token——无法为调用工作流不所属的仓库签发 token。
3. cruise 使用该 token，以 `cruise-agent[bot]` 身份运行并推送提交。Action 创建的提交还会使用 GitHub 提供的 noreply 地址，将触发提及的用户添加为 `Co-authored-by` trailer。
4. 运行结束后（无论成功或失败），Action 都会撤销 token（`DELETE /installation/token`），使其无法在作业生命周期结束后被重用。

如果仓库没有安装该 App（交换服务返回 404）、无法取得 OIDC token（例如未授予 `id-token: write`），或交换服务无法访问，Action 会**回退到工作流的 `GITHUB_TOKEN`**，并记录原因（“未安装”时显示带有 App 安装链接的 `::notice::`，其他情况则显示 `::warning::`）。在这种情况下 cruise 仍会运行，但会受到两项 `GITHUB_TOKEN` 特有限制：它创建的草稿 PR 不会触发其他 `on: pull_request` 工作流（参见[安全](#security)），提交作者/提交者会是 `github-actions[bot]`，而不是 `cruise-agent[bot]`。

<a id="bring-your-own-token"></a>
## 使用自己的 token

显式设置 `github_token`（例如设置为 PAT 或你自己的 GitHub App token）即可完全跳过 OIDC 交换——Action 会使用该 token 执行每个 GitHub API 调用和推送，并且不会尝试交换或撤销 token：

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    github_token: ${{ secrets.MY_PAT }}
```

<a id="self-hosting-the-token-exchange"></a>
## 自托管 token 交换服务

token-exchange 服务是一个小型 Cloudflare Worker，用于验证 GitHub Actions OIDC token 并签发 GitHub App 安装 token；其源代码和部署说明见 [`token-exchange/README.md`](../../token-exchange/README.md)。如果你运行自己的实例（使用自己的 GitHub App 和 Worker），可通过 `token_exchange_url` 将 Action 指向该实例：

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    token_exchange_url: https://your-worker.your-subdomain.workers.dev/token
```

将 `token_exchange_url: ""` 设为空字符串即可完全禁用交换，并始终使用 `github_token`/`GITHUB_TOKEN`。

在上述两种情况下，Action 都会在启动时发布跟踪评论，并在结束时用结果重写该评论——成功时附带链接（`run` 对应 PR，`exec` 对应提交，`plan`/`fix` 对应计划评论），失败时则附带运行链接。运行日志会被刻意**完全排除**在评论之外（参见[安全](#security)）。

<a id="inputs"></a>
## 输入项

| 输入项 | 默认值 | 描述 |
|---|---|---|
| `anthropic_api_key` | *（空）* | Anthropic API 密钥——作为 jcode 的 `anthropic-api` 凭证，存储在 Action 为此作业指定的 jcode 主目录中。`anthropic_api_key`、`openai_api_key`、`provider_api_keys`、`providers` 或 `env` 中至少有一项必须非空。 |
| `openai_api_key` | *（空）* | OpenAI API 密钥——以同样方式作为 jcode 的 `openai-api` 凭证存储。`anthropic_api_key`、`openai_api_key`、`provider_api_keys`、`providers` 或 `env` 中至少有一项必须非空。 |
| `github_token` | *（空）* | 用于 GitHub API 调用（权限检查、评论、PR、推送）的 token。留空（默认）时会先尝试 cruise-agent App OIDC token 交换，失败后回退到工作流的 `GITHUB_TOKEN`；显式设置后会跳过交换并改用该 token。参见[身份验证的工作方式](#how-authentication-works)。 |
| `token_exchange_url` | `https://cruise-token-exchange.smartcrab.ai/token` | token-exchange 服务的 URL。留空会禁用交换（始终回退到 `github_token`/`GITHUB_TOKEN`）。参见[自托管 token 交换服务](#self-hosting-the-token-exchange)。 |
| `trigger_phrase` | `@cruise` | 触发运行所必须出现的短语（按单词边界匹配）。对于 `issues` 事件，会拼接 Issue 标题和正文进行匹配，因此短语可以有效地出现在标题中。 |
| `cruise_version` | `latest` | 要安装的 cruise 版本（`latest` 或类似 `v0.2.0` 的标签）。要求 v0.2.0 或更高版本；若二进制版本过旧，安装步骤会以明确错误拒绝继续。 |
| `jcode_version` | `latest` | 通过 jcode 自己的安装程序安装的 jcode 版本（`latest` 或类似 `v0.88.0` 的标签）。若 `jcode` 已在 PATH 中则忽略此项；无论如何，cruise 都要求 jcode v0.88.0 或更高版本。 |
| `config` | *（空）* | 仓库中 cruise 工作流配置 YAML 的路径，由 `run`/`plan`/`fix` 使用（设置 `CRUISE_CONFIG`）。留空时由 cruise 自己的解析器从检出目录中选择配置，或使用其内置的 `tdd` 工作流。对 `exec` 无效。 |
| `model` | *（空）* | 覆盖 `CRUISE_MODEL`；对于 `sdk: jcode`，使用 `provider/model[:effort]`、裸模型名，或不设置此覆盖值。每个实际的 jcode 提示都必须从此输入项、工作流配置、检出目录中的 `.cruise/default.yaml`，或（对于规划提示）`plan_model`/`CRUISE_PLAN_MODEL` 中解析出有效的 Cruise 模型。jcode 配置的默认模型和 `JCODE_MODEL` 不能替代它。 |
| `flags` | *（空）* | `run` 的工作流标志值（忽略空行和以 `#` 开头的行）。其余每行会作为一个 `--flag` 值传入，可以是裸 `NAME`、逗号分隔的列表，或显式的 `NAME=on\|off\|true\|false` 赋值。未出现在输入中的标志均为关闭状态；不要求设置任何标志。对 `exec`、`plan` 或 `fix` 无效。 |
| `plan_model` | *（空）* | 覆盖 `CRUISE_PLAN_MODEL`（`plan`/`fix` 命令的规划步骤），格式与 `model` 相同。 |
| `providers` | *（空）* | 以 jcode 提供方配置档案名称为键的 JSON 对象。每个条目必须有 `base_url` 和 `model`；可选字段为 `context_window`、`auth`、`auth_header`、`provider_routing`、`no_auth`、`default`。每个条目都会通过 `jcode provider add` 变成 `[providers.<name>]` 配置档案。完整结构参见[提供方](#providers)。 |
| `provider_api_keys` | *（空）* | 为 `providers` 中的配置档案每行提供一个 `profile-name=API key`；忽略空行/注释行，并以第一个 `=` 分隔密钥。每个密钥都会传给 `jcode provider add --api-key-stdin`。每个非 `no_auth: true` 的 `providers` 条目都必须提供密钥；如果所有条目均为 `no_auth`，则可以留空。 |
| `env` | *（空）* | 导出（并遮蔽）到 cruise 进程中的额外 `KEY=VALUE` 行。保留名称会被跳过并输出警告。 |
| `allowed_bots` | *（空）* | 允许触发 cruise 的 bot 登录名，以逗号分隔（不带 `[bot]`）；也可设为 `*` 以允许任意 bot。留空时会阻止所有 bot。 |
| `git_user_name` | *（空）* | 此 Action/cruise 创建的提交所用的 git `user.name`。留空时，如果运行使用了 App token，则默认设为 `cruise-agent[bot]`；否则设为 `github-actions[bot]`。 |
| `git_user_email` | *（空）* | 上述提交使用的 git `user.email`。留空时会解析为与 `git_user_name` 默认值对应的邮箱。如果 GitHub 事件载荷包含触发用户的登录名和数字用户 ID，提交仍会将该用户添加为 `Co-authored-by` trailer。 |

<a id="outputs"></a>
## 输出项

| 输出项 | 描述 |
|---|---|
| `command` | 从提及中解析出的命令：`"run"`、`"exec"`、`"plan"` 或 `"fix"`（如果前置检查跳过运行，则为空）。 |
| `session_id` | 此次运行所操作的 cruise 会话 ID——命令运行后，从 cruise 会话列表中解析出的最近会话。如果前置检查跳过运行、运行在到达该步骤前中止，或根本不存在会话，则为空。 |
| `pr_url` | cruise 创建的 PR URL（仅限 `run` 命令）；未创建 PR 时为空。只有在 `cruise run` 成功，且所选会话状态为 `Completed` 并记录了跳过原因（`disabled` 或 `no-commits`）后，空值才有效；否则仍会视为失败。 |
| `commit_url` | cruise 推送到默认分支的提交 URL（仅限 `exec` 命令）。 |
| `plan_comment_url` | cruise 发布或编辑的计划跟踪评论 URL（仅限 `plan`/`fix` 命令）。 |
| `conclusion` | `success`、`failure` 或 `skipped`（提及未匹配，或操作者未获授权）。前置检查级配置错误——runner 缺少 `python3`，或 `anthropic_api_key`/`openai_api_key`/`provider_api_keys`/`providers`/`env` 全部为空——不属于跳过：前置检查会写入非空的 `gate_error`，最终步骤会报告 `conclusion=failure`。 |
| `used_app` | 如果运行通过 cruise-agent App 安装 token 进行身份验证，则为 `"true"`；如果使用 `github_token`/`GITHUB_TOKEN`，则为 `"false"`；如果前置检查在 `token` 步骤运行前跳过了运行（提及未匹配/操作者未获授权），则为空字符串。 |

<a id="testing-the-actions-step-scripts"></a>
## 测试 Action 步骤脚本

组合式 Action 的步骤脚本（`action/scripts/*.sh`）由 `scripts/test_action_*.sh` 中的测试套件直接执行。测试套件会伪造 runner 契约（`GITHUB_ENV`、`GITHUB_OUTPUT`、`RUNNER_TEMP`），并使用 `gh`、`curl`、`cruise` 和 `jcode` 的 PATH stub，因此测试完全独立隔离：所有内容都写入临时目录，不需要网络访问，并且同时兼容 bash 3.2 和 bash 5。

| 测试套件 | 覆盖的步骤脚本 |
|---|---|
| `scripts/test_action_gate.sh` | `action/scripts/gate.sh` |
| `scripts/test_action_token.sh` | `action/scripts/app-token.sh` 和 `action/scripts/revoke-token.sh` |
| `scripts/test_action_config_install.sh` | `action/scripts/resolve-config.sh` 和 `action/scripts/install.sh` |
| `scripts/test_action_comments.sh` | `action/scripts/comment-start.sh` 和 `action/scripts/finalize.sh` |
| `scripts/test_action_run.sh` | `action/scripts/run.sh` 和 `action/scripts/lib/plan.sh` |
| `scripts/test_action_provider_config.sh` | `action/scripts/provision-jcode.sh`、`action/scripts/setup-env.sh`，以及 `gate.sh` 中的凭证检查 |

在本地运行单个套件：

```bash
bash scripts/test_action_<name>.sh
```

若要像 CI 一样运行所有套件，可遍历 `scripts/test_action_*.sh`：

```bash
status=0
for suite in scripts/test_action_*.sh; do
  echo "== $suite"
  bash "$suite" || status=1
done
exit "$status"
```

共享测试框架位于 `scripts/lib/action_test_harness.sh`。任何新建且名称为 `scripts/test_action_*.sh` 的套件都会被 `.github/workflows/ci.yml` 中的 `Test GitHub Action step scripts` 步骤自动发现并执行。

<a id="security"></a>
## 安全

- **只有仓库协作者可以触发 cruise。** Action 会调用 GitHub 协作者权限 API，检查评论/提及用户的权限，并要求其拥有写入权限（API 会将 `maintain` 角色报告为 `write`，因此维护者符合要求；`triage` 和 `read` 不符合）。除非在 `allowed_bots` 中显式添加，否则 bot 操作者会被拒绝。
- **不支持 PR 模式。** 始终拒绝 PR 上的 `issue_comment` 事件（存在 `.issue.pull_request`）。
- **计划评论会进行信任验证，而不只是检查标记。** 只有由 `cruise-agent[bot]`/`github-actions[bot]` 发布**且**包含计划标记的评论，`run`/`exec`/`fix` 才会将其视为权威计划来源。否则，没有写入权限的评论者可以发布伪造的 `<!-- cruise:plan -->` 评论，导致维护者之后的 `@cruise run` 执行未经审查的内容（只有**提及本身**要求发布者是获授权的操作者——公共 Issue 上的其他评论不受此限制）。
- **token 交换只会签发限定仓库范围的 token。** 交换服务会在服务器端验证调用方 GitHub Actions OIDC token 中嵌入的 `repository` claim，然后才签发安装 token，因此工作流只能取得限定到其实际运行仓库的 token，绝不会取得限定到 App 安装所在其他仓库的 token。`permissions: id-token: write` 只允许作业向 GitHub **请求** OIDC token；它本身不会授予任何 GitHub API 访问权限。
- **无人值守执行。** cruise 会通过 jcode 驱动模型（并间接使用工作流配置允许的任何工具），过程中不会逐项弹出确认提示——这是无人值守 CI 所必需的，但也意味着成功的提示词注入（见下文）会产生与工作流自身 GitHub token 和 runner 相同的影响范围。只授予工作流所需的 `permissions:`（`contents: write`、`pull-requests: write`、`issues: write`，以及需要 App token 交换时的 `id-token: write`），并将提供方 API 密钥、`GITHUB_TOKEN` 和此 Action 取得的任何 App token 都视为其他拥有仓库写入权限的 CI 密钥。
- **提示词注入。** Issue 正文和 fix 反馈是由攻击者控制的文本，会嵌入发送给模型的提示词中。在这些原始 GitHub 文本成为规划输入之前，Action 会剥除隐藏指令向量——HTML 注释、`<img>` 标签（alt 文本载荷）以及零宽/双向控制 Unicode 字符。这只是缓解措施，并不能保证安全：以普通可见文本写入的指令无法过滤。不要授予此 Action 超出“agent 按 Issue 文本所说的一切操作”仍可接受范围的密钥或权限。
- **运行日志不会发布回 Issue。** 失败评论会链接到 Actions 运行，而不会引用日志输出。Agent 输出可能包含模型被诱导打印的任何内容（包括环境变量值）；GitHub 的密钥遮蔽只对 Actions 日志查看器生效，不适用于通过 API 重新发布的文本，因此复制日志会形成数据外泄通道。日志只保留在运行页面中，并遵循仓库的访问控制。
- **安装器通过 TLS 获取，但不会进行校验和验证。** Action 通过发布版安装脚本（`curl | sh`）安装 cruise 和 jcode。（jcode 安装器会根据已发布的 SHA256SUMS 验证发布资源的 SHA-256；安装脚本本身不会进行校验。）这与 rustup 等工具采用相同的信任模型，但也意味着下载端点一旦被攻破，攻击者代码就会在具有作业密钥的权限下运行。如果希望避免静默跟踪 `latest`，请将 `cruise_version`/`jcode_version` 固定到标签版本。
- **通过 `GITHUB_TOKEN` 创建的 PR 不会触发其他工作流。** 这是 GitHub Actions 有意设置的防递归规则，只适用于普通的 `GITHUB_TOKEN` 回退路径——通过 cruise-agent App 安装 token 创建的 PR/推送（App 已安装时的默认方式）会正常触发 `on: pull_request` 工作流，因为 GitHub 会将 App 身份验证的 Action 视为来自独立操作者。如果你使用 `GITHUB_TOKEN` 回退并需要在 cruise 的 PR 上运行 CI，可以安装该 App、通过 `github_token` 使用自己的 PAT/GitHub App token，或添加一个关闭后再重新打开 PR 的步骤。
- **`exec` 会直接推送到默认分支。** 参见[`exec` 注意事项](#exec-caveats)。
- **runner 隔离。** 每次运行都会将 `XDG_DATA_HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME` 指向 `$RUNNER_TEMP`，因此 cruise 会话/worktree 状态不会在作业间泄漏，也不会触及持久化 runner 的主目录。`JCODE_HOME` 也会以同样方式导出。

<a id="troubleshooting"></a>
## 故障排除

- **提及 `@cruise` 后什么也没发生。** 检查工作流运行列表，确认是否有被跳过/无操作的运行：前置检查步骤会记录拒绝运行的原因（事件类型、操作、缺少触发短语、PR 评论或操作者权限不足）。
- **“'anthropic_api_key'、'openai_api_key'、'provider_api_keys'、'providers' 和 'env' 全部为空”。** 至少设置一个专用密钥、为生成配置档案设置 `providers`/`provider_api_keys`，或通过 `env` 提供一个提供方密钥（例如 `KIMI_API_KEY=...`）。
- **“actor '&lt;login&gt;' has insufficient permission: '&lt;permission&gt;'”。** 评论者需要对仓库拥有 `write`、`maintain` 或 `admin` 权限。
- **“No existing plan comment found”（fix）。** 请先运行 `@cruise plan`；`fix` 只会编辑现有计划跟踪评论，不会创建评论。
- **“cruise completed but no pull request was created”（run）。** 只有当 `cruise run` 成功退出，且所选会话状态为 `Completed` 并记录了不创建 PR 的跳过原因（`create_pr: false` 对应 `disabled`，或成功运行无提交时对应 `no-commits`），缺少 PR URL 才是有效结果。否则，请检查会话日志中的 PR 创建错误，并确认 `permissions: pull-requests: write` 以及分支保护/仓库规则；无法解释的 PR URL 缺失会导致运行失败。
- **`exec` 推送失败。** 通常是默认分支上的分支保护规则导致，参见[`exec` 注意事项](#exec-caveats)。
- **模型解析错误。** 检查已安装的 jcode CLI 是否为 v0.88.0 或更高版本、其 API bridge 是否支持 Cruise 所需的会话能力，以及每个实际的 jcode 提示是否都能解析出有效的 Cruise 模型。这项要求始终适用：jcode 配置的默认模型和 `JCODE_MODEL` 不能替代它。请根据提示类型设置 `model`/`plan_model` 输入项，或在工作流或检出目录中的 `.cruise/default.yaml` 里进行配置。
- **运行总是回退到 `GITHUB_TOKEN`（`used_app` 输出为 `false`）。** 请依次检查：此仓库是否已安装 `cruise-agent` App（[安装链接](https://github.com/apps/cruise-agent/installations/new)）；工作流是否授予 `permissions: id-token: write`；`token_exchange_url` 是否非空且可访问。`token` 步骤的日志会说明具体失败原因。
- **Windows runner** 受支持：安装步骤会运行 cargo-dist PowerShell 安装程序（`cruise-installer.ps1`），将 x64 `cruise.exe` 放入 `$RUNNER_TEMP`，并将该目录附加到 `GITHUB_PATH`。`command:` 步骤通过 `cmd.exe /C` 运行。
- **自托管 runner** 的 PATH 中需要有 `git`、`curl`、`jq`、`python3` 和 `gh` CLI（GitHub 托管 runner 上均已预装）。

<a id="faq"></a>
### 常见问题

- **为什么新建的 Issue 立刻变成了 PR，而不是先发布计划？** 其标题或正文已经包含触发短语，因此 `issues: [opened]` 事件立即触发。短语后面的第一个词元（同一行）会选择 `plan`、`fix`、`exec` 或 `run`；只有缺少词元或词元未知时才默认执行 `run`——参见[典型工作流](#typical-workflow)中的警告。如果希望先审查计划，请不要在 Issue 自身内容中写入触发短语，而是在之后评论 `@cruise plan`。
- **cruise 打开的 PR 无法合并/没有显示为可审查状态。** `run` 创建的所有 PR（以及 `exec` 的等效路径——不过该路径是直接推送，本身不会创建 PR）都始终作为**草稿**打开。审查差异后，请自行将其标记为“Ready for review”；参见[典型工作流](#typical-workflow)。
- **运行大约 30 分钟后超时。** `write-tests -> implement`（或自有配置中的等效步骤）都会完整运行一次验证（格式化、lint、整个测试套件）；测试期间，在大型仓库中端到端耗时超过 30 分钟。建议从 `timeout-minutes: 60` 开始（如[`examples/cruise.yml`](../../examples/cruise.yml)所示）；如果测试套件较慢，请进一步调高。
- **同一 Issue 上第二条 `@cruise` 评论好像卡住了，没有运行。** 它在排队，并未丢失：示例工作流中的 `concurrency:` 组会按 Issue 序列化运行（不设置 `cancel-in-progress`），因此 `plan` -> `fix` -> `run` 会依次执行。请检查 Actions 运行列表——前一个运行可能仍在进行中。
- **cruise 创建的 PR 没有触发我的 CI 工作流。** 如果正在使用 `GITHUB_TOKEN` 回退（未安装 App），这是预期行为——参见[安全](#security)和[身份验证的工作方式](#how-authentication-works)。
