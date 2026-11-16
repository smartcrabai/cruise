[English](../en/configuration.md) | [日本語](../ja/configuration.md) | [简体中文](../zh/configuration.md) | [返回目录](README.md)

# 配置参考

<a id="config-file-resolution"></a>
## 配置文件解析

cruise 按以下方式解析工作流配置：

1. **`-c/--config` 标志** -- 优先级最高。指定的文件必须存在，否则 cruise 会报错退出。不会显示提示。`-c __builtin__` 选择内置的 `tdd` 工作流，等同于 `-c builtin:tdd`；`-c builtin:<name>` 选择目录中指定名称的工作流。旧的选择器 `builtin:default`、`builtin:simple` 和 `builtin:review` 不受支持。固定的工作流目录如下：

   | 名称 | 描述 |
   |------|-------------|
   | `tdd` | 采用测试先行的实现方式，随后执行“并行分析 → 汇总 → 应用”验证组和 PR 工作流 |
   | `refactor` | 以差异为起点改进代码、测试和结构，并进行验证 |
   | `add-tests` | 审核行为和用户流程的覆盖情况，添加有意义的测试和 CI，然后独立验证并补齐缺口 |
   | `add-formal-specs` | 添加可执行的 Quint、Alloy 和 Gherkin 规范并接入 CI；修复规范/源代码中的实际缺陷 |
   | `research` | 在限定范围内开展只读调查，提供有来源依据的证据、独立审查和附有引用的报告 |
   | `security` | 限定范围内的安全盘点与并行审计、问题分诊、对已确认缺陷的安全修复，以及独立验证 |
   | `pr-review-fix` | 在现有 GitHub PR 的头分支上处理符合条件的审查讨论串 |

   `refactor`、`add-tests`、`add-formal-specs` 和 `security` 默认设置 `force_exec: true`：本地规划直接在当前目录中运行，不使用外层计划/会话或工作树，也不会自动创建 PR。传入 `--no-force-exec` 可改用常规的计划/会话流程。

   `cruise workflow list` 会输出工作流目录。要自定义内置工作流，请使用 `cruise workflow eject <name> [--to user|project]`，例如 `cruise workflow eject tdd --to project`。导出操作仅接受目录中的名称，并且绝不会覆盖已有文件。

   `refactor` 以仓库中现有的已暂存、未暂存和未跟踪更改为起点，并在存在可信的基准/合并基准时，纳入任务分支相对于该基准/合并基准的更改。它将更改限制在该差异及必要的调用方/依赖项之内；如果没有差异，则将范围设为整个仓库。现有工作会被保留，不会对无关编辑执行重置、贮藏或还原。

   research 工作流以及 security 的盘点/审计/分类评估阶段需要使用 SDK 权限进行只读工作。在解析工作流及适用的用户/项目默认设置后，如果 `command:` 和 `sdk:` 均未设置，Cruise 会使用其默认值 `sdk: jcode`；`command:` 后端无法被限制为只读模式，因此会在权限受限的步骤中被拒绝使用。

   交互式选择器（CLI、TUI、WebUI）会将内置工作流和其他候选项按类别组织。没有配置文件时，非交互式解析会使用 `tdd`。

   `create_pr` 是工作流专用的布尔配置项，默认值为 `true`。将其设为 `false` 会跳过 Cruise 的自动提交、推送、PR 创建、`after-pr` 和基于 PR 的清理，同时保留会话、产物及工作树。用户级和项目级 `default.yaml` 均不接受 `create_pr`；它不是 CLI 选项。

2. **`CRUISE_CONFIG` 环境变量** -- 如果已设置，则直接使用其指定的文件（文件不存在时报错）。不会显示提示。
3. 否则，cruise 会从以下位置收集所有候选项，并将它们列为可选项：
   - `./cruise.yaml` -> `./cruise.yml` -> `./.cruise.yaml` -> `./.cruise.yml`（当前目录）
   - `./.cruise/*.yaml` / `*.yml`（当前目录），按文件名排序；不包括顶层的 `./.cruise/default.yaml`
   - `$XDG_CONFIG_HOME/cruise/workflows/*.yaml` / `*.yml` 以及有效的已锁定捆绑包目录中的成员（默认：`~/.config/cruise/workflows/`），按相对路径排序

   当 stdin 和 stdout 都是 TTY 时，会在分类交互式选择器中显示候选项和内置工作流。在非交互式环境中（通过管道传入 stdin、脚本），会自动选取优先级最高的候选项，而不显示提示。
4. **未找到配置文件** -- 交互式终端会在选择器中提供内置工作流；非交互式解析会自动选择嵌入的 `tdd` 工作流（在源代码树中位于 `builtin/tdd.yaml`）。无需配置文件。

交互式选择器使用配置的 `workflow_categories` 树；如果该树为空或不存在，则使用默认类别：Development（`tdd`、`refactor`）、Quality（`add-tests`、`add-formal-specs`）、Research（`research`）和 Security（`security`）。非空的用户树会替换这些默认设置，而不是与其合并。类别和工作流条目各自保留其声明顺序；CLI 子菜单会先显示子类别，再显示工作流条目。一个工作流可以出现在多个类别中。

未分类的用户工作流和内置工作流会显示在 `Others` 或由 `others_category_name` 配置的类别中。设置 `show_others_category: false` 会将其隐藏。类别仅影响交互式显示，不影响显式选择或非交互式解析的优先级。
<a id="workflow-defaults-defaultyaml"></a>
### 工作流默认值（`default.yaml`）

`$XDG_CONFIG_HOME/cruise/default.yaml` 提供用户默认值，`./.cruise/default.yaml` 提供项目默认值。当未设置 `XDG_CONFIG_HOME` 时，在 macOS/Linux 上默认使用 `~/.config/cruise/default.yaml`；在 Windows 上默认使用 `%APPDATA%\cruise\default.yaml`（如果未设置 `APPDATA`，则使用 `~/.config/cruise/default.yaml`）。这些文件为工作流中未设置的字段提供默认值，不能被选作工作流（使用 `-c` 或 `CRUISE_CONFIG` 指向它们会报错）。

在适用时，优先级从高到低依次为：单次调用的 CLI 标志（包括用于选择工作流的 `-c`） > 配置环境变量覆盖 > 工作流文件 > `./.cruise/default.yaml` > `$XDG_CONFIG_HOME/cruise/default.yaml` > 应用程序模型默认值（`config.json` 或 `config.yaml`） > 内置默认值。对于普通提示，步骤级模型覆盖仍具有更高优先级。

当实际的 jcode 提示没有生效的模型时，Cruise 会创建 `$XDG_CONFIG_HOME/cruise/default.yaml`（或该平台通常使用的用户配置路径），将其内容设为 `model: null`，并在启动 jcode 前停止。请编辑该用户级文件，设置实际的 `provider/model[:effort]` 引用或裸模型。对于新会话，也可以在 `config.json`、`config.yaml`、工作流或 `CRUISE_MODEL` 中设置 `model`；规划调用可以使用 `plan_model` / `CRUISE_PLAN_MODEL`。

更新设置后，请用发起请求的命令重试：运行失败的会话使用 `cruise run <SESSION>`，规划或执行请求则重新运行相应的 `cruise plan` 或 `cruise exec`。基于文件的会话会重新加载默认值，但快照会话不会重新应用后续的默认值或环境变更；要使用更新后的设置，请启动新的快照会话。

- `env` 和 `mcp_servers` 按键合并（较高层级的值优先）。
- `command` 和 `sdk` 构成一对：如果某一层设置了其中任意一项，则忽略所有较低层级中的这一对设置。
- 其他字段（`model`、`plan_model`、`subagent_model`、`max_retries`、`interactive_planning`、`languages`、`cleanup_after_pr`、`force_exec`、`computer_use`、`permission`、`disallowed_tools`、`retry`、`formal_verify`）取自设置了相应字段的最高层级。限制列表在各层级之间采用替换方式；在工作流中显式设置 `disallowed_tools: []` 会清除继承的默认值。
- 如果默认配置指定的后端（`command` / `sdk`）与工作流不同，且会填充未设置的后端专属字段（例如 `model`、`retry`、`mcp_servers`），校验会失败。请在工作流中设置相应字段，或从 `default.yaml` 中移除。

`pre_steps:` 定义仅执行命令的步骤（`command`、`env`、`timeout`、`skip`、`when`），这些步骤会以 `pre/<name>` 的形式插入工作流 `steps` 的开头。它们不会在规划阶段或 `after-pr` 中运行。用户步骤排在前面，项目步骤排在后面；与用户步骤同名的项目步骤会在原位置替换该用户步骤。

```yaml
# ./.cruise/default.yaml
pre_steps:
  mise-trust:
    command: mise trust || true
  nix:
    command: nix develop --command true
```

<a id="config-file-reference"></a>
## 配置文件参考

<a id="basic-structure"></a>
### 基本结构

`formal_verify.model_check_timeout_seconds` 设置 Quint model-check 和 Alloy 的单条命令超时时间，单位为秒。它接受 1 到 86400（含）之间的整数，默认值为 900。可以在工作流 YAML 或分层的 `default.yaml` 中设置：

```yaml
formal_verify:
  model_check_timeout_seconds: 900
```

```yaml
command:                   # LLM invocation command (mutually exclusive with `sdk`)
  - claude
  - --model
  - "{model}"
  - -p

# sdk: jcode              # Use jcode-sdk (the fallback only when neither
                          # `command` nor `sdk` remains set after applicable
                          # `default.yaml` layers; see SDK Mode)
# sdk: claude             # Use claude-agent-sdk to drive the claude CLI

description: |             # one-line summary shown next to the filename in selectors (optional)
  Team-shared review-heavy flow with auto-PR.

model: sonnet             # default model for all prompt steps (optional)
# In SDK mode, arrays use the first model, then fall back in order.
# model: [sonnet, opus, gpt-5.5]
plan_model: opus          # model used for the built-in plan step (optional)
# subagent_model: haiku   # SDK-only: one subagent model, string or {sdk: claude, model: haiku}
languages:                # prompt languages (optional; defaults to English)
  pr: English             # language for auto-generated PR title/body
  plan: English           # language used by built-in planning prompts
# commit_message_template: "{title} [{session_id}]" # host-created commit message
# pr_body_template: "## Summary\n{body}"            # generated PR body wrapper
# force_exec: false       # execute direct plan entry points in place (use --no-force-exec to opt out)
# permission: full        # prompt permission: read-only | edit | full (default full)
# disallowed_tools: [web, subagent, "shell:npm publish"] # SDK prompt deny list
# computer_use: false     # jcode only, macOS: let prompts control the desktop (macos_computer_use)
# Deprecated compatibility fields: pr_language and plan_language

env:                      # environment variables applied to all steps (optional)
  API_KEY: sk-...
  PROJECT: myproject

# MCP servers are available to SDK runs only; omit this block for command mode.
# mcp_servers:
#   local_search:
#     command: npx
#     args: ["-y", "@example/search"]

flags:                    # workflow flags (optional; absent values default off, see Workflow flags)
  # run_review: {}

groups:                   # step group definitions (optional)
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  step_name:
    # step configuration
    # flag: run_review     # run only when the declared flag is on (optional)

after-pr:                # optional: steps that run automatically after PR creation
  step_name:
    # step configuration (same format as `steps`)
```

对未知键的处理方式因层级而异：各个步骤映射（位于 `steps`、`after-pr`、组的 `steps` 和并行子项中）以及 `retry` 块会拒绝未知字段，并报解析错误，因此，拼写错误或已移除的字段（如 `fail-if-no-file-changes`）会立即引发配置错误。未知的**顶层**键会被静默忽略，因此文件顶层拼写错误的 `plan_model` 或 `max_retries` 也会被接受，只是不会产生任何效果。

<a id="dynamic-model-selection"></a>
### 动态模型选择

当 `command` 数组包含 `{model}` 占位符时，cruise 会在运行时根据每个步骤实际生效的模型来解析该占位符：

- **已指定模型**（通过顶层 `model` 或步骤级 `model`）：将 `{model}` 替换为模型名称。
- **未指定模型**：自动移除 `{model}` 参数及其前面紧邻的 `--model` 标志。

步骤级 `model` 字段会覆盖顶层 `model` 默认值，但仅对该步骤生效。

此处同样适用模板变量的花括号转义规则（参见[变量参考](steps.md#variable-reference)）：`{{model}}` 表示字面字符串 `{model}`，而不是占位符。任何其他未转义的 `{name}`、空的 `{}`、未闭合的 `{` 或单独出现的 `}` 都属于模板语法错误。

```yaml
command:
  - claude
  - --model
  - "{model}"      # replaced at runtime, or --model/{model} pair is stripped if no model
  - -p

model: sonnet      # default; steps without model: use this

steps:
  planning:
    model: opus    # overrides the default for this step only
    prompt: "Create a plan for: {input}"
```

<a id="prompt-languages"></a>
### 提示词语言

UI 显示语言独立于提示词语言。可通过应用的 `uiLanguage`、`CRUISE_UI_LANGUAGE` 或 TUI 语言选择器进行配置；WebUI 浏览器可以使用自己的语言选择器覆盖服务器默认值。这些设置不会更改 `languages.pr` 或 `languages.plan`。

`languages.pr` 控制自动生成的 PR 标题和正文所用语言，`languages.plan` 控制 cruise 内置规划提示词所用语言。`CRUISE_LANGUAGE_PR` 和 `CRUISE_LANGUAGE_PLAN` 会覆盖对应的 YAML 值；空白环境变量值会被忽略。否则，会对 `LC_ALL`、`LC_MESSAGES`、`LANG`、`LANGUAGE` 中第一个非空值进行一次映射（不受支持的值不会继续回退），最后使用 `English`。

```yaml
languages:
  pr: Japanese             # PR title/body will be generated in Japanese
  plan: Japanese           # generated/updated plans and plan answers will be in Japanese
```

内置模板可通过 `{pr.language}` 和 `{plan.language}` 访问实际生效的值。

<a id="commit-and-pr-templates"></a>
### 提交和 PR 模板

可选的顶层字符串键 `commit_message_template` 和 `pr_body_template` 用于自定义工作树 PR 创建流程中由宿主生成的提交消息和 PR 正文。可在工作流、用户的 `<config_dir>/default.yaml` 或项目的 `.cruise/default.yaml` 中设置它们。优先级为：工作流 > 项目默认值 > 用户默认值 > 未设置。它们不是 CLI 选项，也不是环境变量覆盖项。

```yaml
commit_message_template: "{title} [{session_id}]"
pr_body_template: |
  ## Summary
  {body}
```

| 模板 | 允许的变量 |
|----------|-------------------|
| `commit_message_template` | `{title}`, `{issue}`, `{session_id}`, `{branch}` |
| `pr_body_template` | `{title}`, `{body}`, `{issue}`, `{session_id}`, `{branch}` |

`{title}` 是 LLM 生成的 PR 标题，`{body}` 是 LLM 生成的 PR 正文，`{issue}` 目前始终展开为空字符串，`{session_id}` 是会话 ID，`{branch}` 是工作树分支名。这些模板不会自动添加用于关闭议题的文本。

未知变量和格式错误的语法会在工作流加载/编译时被拒绝。使用 `{{` 和 `}}` 转义字面量花括号。

两个模板都未设置时，现有行为保持不变：提交使用生成的 PR 标题或现有的输入回退值，PR 标题为空时使用 `--fill`。这些设置不影响当前分支上的运行、`create_pr: false`、智能体创建的提交或压缩合并消息。

<a id="session-title-generation"></a>
### 会话标题生成

计划获批后，cruise 会生成一个简短的会话标题（最多 80 个字符），在 `cruise list` 和 WebUI 侧边栏中显示，以代替原始任务输入。

<a id="environment-variables"></a>
### 环境变量

`CRUISE_JCODE_CLI_PATH`、`CRUISE_CLAUDE_CLI_PATH` 和 `CRUISE_OPENCODE_CLI_PATH` 通过 Cruise 进程环境指定提供商可执行文件。它们不是工作流配置键，不能通过工作流 `env:` 提供。

环境变量可以在工作流级别和步骤级别设置。步骤级别的值会覆盖工作流级别的值，但仅对该步骤生效。并行子项还增加了第三层：工作流 < 并行块 < 子项。值支持模板变量替换；每个子项都基于进入该块前的变量解析自身的值。

进程级配置覆盖会在工作流和默认配置解析完成后应用。支持的名称和值类型如下：

| 变量 | 配置键 | 值 |
|---|---|---|
| `CRUISE_MODEL` | `model` | 模型 |
| `CRUISE_PLAN_MODEL` | `plan_model` | 模型 |
| `CRUISE_SUBAGENT_MODEL` | `subagent_model` | 模型条目 |
| `CRUISE_SDK` | `sdk` | 文本 |
| `CRUISE_COMMAND` | `command` | JSON 数组 |
| `CRUISE_MAX_RETRIES` | `max_retries` | 整数 |
| `CRUISE_INTERACTIVE_PLANNING` | `interactive_planning` | 布尔值 |
| `CRUISE_CLEANUP_AFTER_PR` | `cleanup_after_pr` | 布尔值 |
| `CRUISE_FORCE_EXEC` | `force_exec` | 布尔值 |
| `CRUISE_COMPUTER_USE` | `computer_use` | 布尔值 |
| `CRUISE_PERMISSION` | `permission` | 文本：`read-only`、`edit` 或 `full` |
| `CRUISE_DISALLOWED_TOOLS` | `disallowed_tools` | JSON 数组 |
| `CRUISE_COMMIT_MESSAGE_TEMPLATE` | `commit_message_template` | 文本 |
| `CRUISE_PR_BODY_TEMPLATE` | `pr_body_template` | 文本 |
| `CRUISE_RETRY` | `retry` | JSON 对象或 `null` |
| `CRUISE_RETRY_BASE_DELAY_MS` | `retry.base_delay_ms` | 整数 |
| `CRUISE_RETRY_MAX_DELAY_MS` | `retry.max_delay_ms` | 整数 |
| `CRUISE_RETRY_MODEL_FALLBACK` | `retry.model_fallback` | 布尔值 |
| `CRUISE_RETRY_FALLBACK_CHAINS` | `retry.fallback_chains` | JSON 对象 |
| `CRUISE_LANGUAGES` | `languages` | JSON 对象 |
| `CRUISE_LANGUAGE_PR` | `languages.pr` | 文本 |
| `CRUISE_LANGUAGE_PLAN` | `languages.plan` | 文本 |
| `CRUISE_RUN_ALL_PARALLELISM` | 应用的 `run_all_parallelism` | 整数，至少为 1 |

名称通常由 `CRUISE_` 加上大写的键路径组成，并将点和连字符替换为下划线。上述语言子字段的环境变量名称仅支持表中所示的拼写。值会去除首尾空白，仅包含空白的值会被忽略。布尔值仅接受 `true`、`false`、`1` 或 `0`；整数必须由不带正负号的十进制数字组成。文本按字面值处理。模型值按单个模型的字面值处理，除非以 `[` 或 `{` 开头，此时会按 JSON 解析，并且必须符合该字段通常的模型结构。具体来说，`CRUISE_MODEL` 和 `CRUISE_PLAN_MODEL` 现在接受 JSON 数组：此前，以 `[` 或 `{` 开头的值被视为字面模型名称。包含回退条目的数组会像 YAML 中一样启用模型回退。其他复合值必须使用 JSON，而不是 YAML 流式语法。在受支持的情况下，可选值应使用 JSON `null`，空列表应使用 `[]`，而不是空白值。无效值会触发指明变量名称的错误，并且仍会经过正常的配置验证。

先应用父对象，再应用子字段覆盖，因此 `CRUISE_RETRY_MAX_DELAY_MS` 会覆盖 `CRUISE_RETRY` 中的对应字段，`CRUISE_LANGUAGE_PR` 会覆盖 `CRUISE_LANGUAGES`。设置 `CRUISE_SDK` 会清除已配置的命令，而设置 `CRUISE_COMMAND` 会清除已配置的 SDK。同时将这两个变量设为非空白值会报错。例如：

```sh
export CRUISE_MODEL='["provider/primary", "provider/fallback"]'
export CRUISE_RETRY='{"base_delay_ms":1000}'
export CRUISE_RETRY_MAX_DELAY_MS=5000
```

`CRUISE_RUN_ALL_PARALLELISM` 会覆盖 TUI/WebUI“全部运行”操作和生效配置显示中的应用设置，但不会保存到配置文件中。CLI `run --all` 使用 `--parallelism`，不会受其影响。

`CRUISE_UI_LANGUAGE` 则控制进程的 UI 语言，而不是工作流字段或生成的提示词语言。有关支持的标签和优先级，请参阅 [`cruise config`](cli-reference.md#cruise-config)。

`CRUISE_COMMIT_COAUTHOR_NAME` 和 `CRUISE_COMMIT_COAUTHOR_EMAIL` 会为 cruise 为 PR 创建的提交添加 `Co-authored-by:` 尾注。两者都必须已设置且非空白；如果姓名包含 `<`、`>` 或换行符，或者地址无效，则会禁用该尾注，而不是让提交失败。

`CRUISE_SKIP_AGENT_AUTH_CHECK=1` 会绕过“SDK 模式”一节所述的 SDK 启动时 OAuth 过期检查。它是仅用于进程的运行时开关，不是工作流配置覆盖项。仅当值恰好为 `1` 时才会跳过该检查。

`CRUISE_USAGE_RECORDING` 是仅用于进程的运行时开关。将其设置为 `0` 或 `false` 可禁用本地用量记录；设置为 `1`、`true` 或未设置时会启用记录。值会去除首尾空白，且不区分大小写。其他值会启用记录，同时发出警告。

```yaml
env:                        # top-level: applied to all steps
  ANTHROPIC_API_KEY: sk-...
  TARGET_ENV: production

steps:
  deploy:
    command: ./deploy.sh
    env:                    # step-level: merged over top-level env
      TARGET_ENV: staging   # overrides top-level value for this step only
      LOG_LEVEL: debug
```

<a id="application-configuration"></a>
## 应用配置

Cruise 将应用设置与工作流 YAML 分开保存在 `$XDG_CONFIG_HOME/cruise/config.json` 或 `config.yaml` 中。如果两者都存在，则优先使用 JSON。有关工作流解析和其他设置，请参阅上方的[工作流配置解析](#config-file-resolution)。

<a id="ui-language"></a>
### 界面语言

在应用配置中设置 `uiLanguage`，或使用 `cruise config --set-language ja`。输入时也接受别名 `ui_language`。可显式指定的语言标签有 `en`、`zh-CN`、`es`、`fr`、`pt-BR`、`ja` 和 `ko`，支持大小写以及 `_`/`-` 的变体。`auto` 表示未指定；`cruise config --set-language auto` 会清除已保存的设置。设置或验证应用配置时，无效值会被拒绝。

对于 CLI 和 TUI，语言选择的优先级依次为 `CRUISE_UI_LANGUAGE`、`uiLanguage`、系统区域设置，最后回退到英语。环境变量值为空或为 `auto` 时，会使用下一优先级的设置。无效的环境变量值会触发英文警告并被忽略。系统区域设置检测会按顺序使用 `LC_ALL`、`LC_MESSAGES`、`LANG` 或 `LANGUAGE` 中第一个非空的值；如果该值不受支持，也不会尝试后续变量。如果无法通过环境变量确定区域设置，Windows 还会查询用户区域设置。macOS 和 Linux 仅使用环境变量，因此通过 GUI 启动时，如果没有这些环境变量，则默认使用英语。

如果浏览器中的 `cruise_ui_language` Cookie 有效，WebUI 就会使用它，否则使用服务器进程的默认语言。在 WebUI 中选择语言会保存 Cookie 并重新加载页面，不会更改应用配置。界面语言独立于工作流的 `languages.pr` 和 `languages.plan`，这两项设置用于控制生成的提示词。

<a id="config-hot-reload"></a>
## 配置热重载

在 `cruise run` 运行期间，相邻步骤之间都会检查配置文件是否发生变化。如果文件已被修改（通过 mtime 检测），就会自动重新加载更新后的配置，无需重启。这使你可以在会话运行期间调整提示词、添加步骤或微调设置。

> **注意：** 热重载仅适用于从外部配置文件启动的会话，不适用于嵌入的内置工作流。当前步骤必须在新配置中仍然存在，重新加载才会生效。新声明的标志如果不在会话已保存的值中，则处于关闭状态。

<a id="rate-limit-retry"></a>
## 速率限制重试

在没有 SDK 回退策略的情况下，如果命令的 stderr 包含表示速率限制的措辞，或包含独立的 `429` 标记（而非偶然出现的 `429` 子字符串），cruise 会采用指数退避，在同一模型上重试。SDK 后端还会将 `usage limit`、`session limit` 和 `overloaded` 视为限制：

- 初始延迟：2 秒
- 最大延迟：60 秒
- 默认重试次数：5（可通过 `--rate-limit-retries` 覆盖）

SDK 后端还支持通过可选的 `retry:` 块，或包含回退条目的工作流级模型数组来配置回退策略。显式的 `retry:` 块可用于配置该策略：

```yaml
retry:
  base_delay_ms: 500        # default: 500
  max_delay_ms: 300000      # default: 300000
  model_fallback: true      # default: true
  fallback_chains:
    default:
      - provider-a/model-a
      - provider-b/model-b
    "provider-a/*":
      - provider-c/*
```

启用 SDK 重试策略后，遇到 429、5xx、除 429 以外的 4xx 或网络故障时，会立即切换到下一个回退模型；只有在没有可用回退项时才会在同一模型上重试。只要声明了 `retry:`，即使 `model_fallback: false` 或回退链为空，相关错误分类和退避仍然生效。要保留以往的行为，请省略 `retry:` 块并使用标量或未设置的模型字段。包含回退项的工作流级模型数组会启用模型回退。`command:` 后端始终使用以往的行为，并忽略 `retry:`。
