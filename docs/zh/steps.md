[English](../en/steps.md) | [日本語](../ja/steps.md) | [简体中文](../zh/steps.md) | [返回目录](README.md)

# 步骤参考

<a id="step-types"></a>
## 步骤类型

<a id="prompt-step-llm-call"></a>
### 提示步骤（LLM 调用）

提示步骤可以声明 `quality_gates`，在成功尝试结束并继续之前执行验证；参见[质量门](workflow-control.md#quality-gates-quality_gates)。

```yaml
steps:
  planning:
    model: claude-opus-4-5        # model to use (optional; overrides top-level model)
    instruction: |                # message displayed before the step runs (optional)
      Before planning, add any constraints or context:
    prompt: |                     # prompt body (use either prompt or prompt_file)
      Create an implementation plan for:
      {input}
    timeout: 10m                  # per-step timeout (optional; see Step Timeout)
    output_file: initial-state.md # save the final response as a session artifact (optional)
    allow_commit: false           # default: guard this prompt from moving Git HEAD in this repository
    computer_use: true            # optional; jcode only; overrides top-level computer_use
    env:                          # environment variables for this step (optional)
      ANTHROPIC_MODEL: claude-opus-4-5
```

设置 `output_file` 后，Cruise 会将后端完成的响应保存在工作区之外、会话的 `artifacts/` 目录中。后续提示正文可以通过 `{file:initial-state.md}` 读取该文件。

`instruction` 是步骤运行前显示给用户的消息，不是系统提示。如果 `{input}` 为空，这条消息会作为多行输入提示，用户提交的文字将成为 `{input}`；否则只会显示消息，不会将其发送给 LLM。

默认情况下，提示步骤受提交保护。SDK 提示还会应用[工具限制](sdk.md#tool-restrictions)中描述的隐式 Git shell 拒绝规则；`allow_commit: true` 会移除这些拒绝规则和提交保护，但不会移除显式的 `disallowed_tools`，也不会授予更广泛的权限。只有提示确实需要创建提交或以其他方式移动 Git `HEAD` 时才设置 `allow_commit: true`；省略该值或设为 `false` 都会保留保护。此保护适用于传统的 `command:` 模式及所有 SDK 后端（`jcode`、`claude` 和 `opencode`），而 command 和 option 步骤不受影响。

应将 `allow_commit: true` 设置在实际提示步骤上，而不是 `group:` 或 `workflow_call:` 调用处。

较长提示可以通过 `prompt_file` 从文件加载提示正文：

```yaml
steps:
  implement:
    prompt_file: prompts/implement.md
```

| `prompt_file` 值 | 解析方式 |
| --- | --- |
| 绝对路径 | 按原路径使用 |
| `~/...`、`~` 或 `~user/...` | 展开为当前用户或指定用户的主目录 |
| `./...`、`../...` 或裸文件名 | 相对于配置文件所在目录 |
| GitHub blob/raw URL | 从 GitHub 获取 |
| 被调用工作流中的相对路径 | 相对于被调用工作流的目录（或远程目录） |

绝对路径和 `~` 路径指向本地文件系统。在 GitHub 托管的工作流中，相对的非 URL 值会作为远程目录中的路径解析；本地 `~` 路径会被拒绝。也可以显式使用直接的 GitHub blob/raw URL。

<a id="command-step-shell-execution"></a>
### 命令步骤（shell 执行）

```yaml
steps:
  run_tests:
    command: cargo test           # single command (required)
    timeout: 5m                   # per-step timeout (optional; see Step Timeout)
    env:                          # environment variables for this step (optional)
      RUST_LOG: debug

  lint_and_test:
    command:                      # list of commands: run sequentially, stop on first failure
      - cargo fmt --all
      - cargo clippy -- -D warnings
      - cargo test
```

<a id="parallel-steps"></a>
### 并行步骤

使用 `parallel:` 可在一个会话中并发运行命名的提示或命令步骤。所有子步骤完成后才会开始下一步。

```yaml
steps:
  checks:
    parallel:
      lint:
        command: cargo clippy -- -D warnings
      tests:
        command: cargo test
      review:
        prompt: Review the changes and report findings without editing files.
        timeout: 10m
    timeout: 15m
  summarize:
    prompt: "Summarize the check results: {prev.output}"
```

- 所有子步骤都在会话现有工作目录中同时启动。应使用彼此独立的命令或文件编辑；Cruise 不会隔离或合并子步骤的变更。并发编辑相同文件或 Git 状态可能产生冲突。
- 每个子步骤接收相同的输入，以及并行块开始前的 `{prev.*}` 值。环境变量优先级依次为工作流 < 并行块 < 子步骤。子步骤可使用 `model`、`env`、`skip`、`when` 和 `timeout`；提示子步骤还可设置 `instruction`、`computer_use`、`permission`、`disallowed_tools`（父步骤不能设置这些字段）和 `output_file`。并行块中的 `instruction` 不会与用户交互，因此需要非空运行时输入。请提供输入或在此前安排顺序执行的 instruction 步骤；否则执行会在打开输入编辑器前报错。每个子步骤中的命令数组仍按顺序执行。
- 汇合后，`{prev.output}` 是按 YAML 声明顺序以子步骤名称为键的 JSON 对象。每项包含 `output`（提示文本；命令为 `null`）、`stderr`（含执行错误）、`success` 和 `skipped`。所有子步骤都成功或被跳过时，`{prev.success}` 为 true；`{prev.stderr}` 会合并子步骤错误并附上子步骤名称。
- 子步骤失败不会取消兄弟步骤。整个块计为一个步骤，最多计为一次失败，并遵循普通步骤失败语义。在父步骤上设置 `if.fail` 或 `next`；重试会重新运行整个块。
- 子步骤超时只影响该子步骤。父步骤超时会取消尚未完成的子步骤，并等待它们停止后再走失败路径。
- 并行块也可用于 `groups`、`after-pr` 和被调用的工作流中。子步骤必须是提示或命令步骤；不允许子步骤使用 `next`、`if`、`option`、`plan`、`group`、`workflow_call`、嵌套 `parallel`、`flag` 或 `allow_commit: true`。应在父步骤配置流程控制（包括 `flag`），并在后续顺序步骤中执行提交。

<a id="step-timeout"></a>
### 步骤超时

提示、命令和并行步骤可设置 `timeout:`，超时后中止步骤（option 步骤忽略该字段；命令数组中的每条命令分别应用超时；并行块的超时限制整个块）。支持的格式如下：

| 后缀 | 含义 | 示例 |
|--------|---------|---------|
| （无） | 秒 | `timeout: "30"` |
| `m` | 分钟 | `timeout: 5m` |
| `h` | 小时 | `timeout: 1h` |

超时触发时：

- **命令步骤**：子进程会被终止，步骤被视为失败（非零退出）。`{prev.success}` 为 `false`，工作流按常规失败路径执行（见下文 `if.fail`）。
- **提示步骤**：LLM 调用会中止，步骤被视为失败。

无效的超时字符串会在配置校验时被拒绝。group 和 `after-pr` 中定义的步骤也遵守超时设置。

<a id="option-step-interactive-selection"></a>
### 选项步骤（交互式选择）

`option` 中的每个项目可以是 `selector`（菜单选项）或 `text-input`（自由文本提示）。可选的 `plan` 字段解析为文件路径；显示菜单前，会在带边框的面板中展示该文件内容：

```yaml
steps:
  review_plan:
    plan: "{plan}"               # optional: display contents of this file before the menu
    option:
      - selector: Approve and continue   # shown in selection menu
        next: implement
      - selector: Revise the plan
        next: planning
      - text-input: Other (free text)    # shows a text prompt when selected;
        next: planning                   # entered text is available as {prev.input}
      - selector: Cancel
        next: ~                          # null/omitted next falls through to the
                                         # next declared step; the last step ends the workflow
```

<a id="post-pr-automation-after-pr"></a>
## PR 创建后的自动化（`after-pr`）

有关主机创建的提交和 PR 正文格式，参见[提交和 PR 模板](configuration.md#commit-and-pr-templates)。

`after-pr` 用于定义 `cruise run` 成功创建 pull request 后自动运行的步骤。`after-pr` 使用与 `steps` 相同的步骤格式，因此也可在其中定义内联或基于文件的提示步骤（`prompt` / `prompt_file`）、命令步骤和分组步骤。

```yaml
steps:
  implement:
    prompt: "{input}"

  test:
    command: cargo test

after-pr:
  notify:
    command: "echo 'PR #{pr.number} created: {pr.url}'"

  label:
    command: "gh pr edit {pr.number} --add-label enhancement"
```

只有 PR 创建成功后才会运行 `after-pr` 步骤。它们可以使用所有普通模板变量，以及下文列出的 PR 专用变量。

`after-pr` 中的提示步骤也受相同的默认保护。若某个 after-PR 提示需要推进 `HEAD`，请单独设置 `allow_commit: true`。

<a id="review-loop-github-review"></a>
### 评审循环（`github-review`）

显式的 `after-pr` 步骤可以处理 CodeRabbit 等评审机器人。通用内置工作流不会加入该步骤；`builtin:pr-review-fix` 则会对已有 PR 使用它。

```yaml
after-pr:
  coderabbit:
    github-review:
      bots: ["coderabbitai[bot]"]   # any one of these reviewing the current PR head ends the wait
      max-iterations: 3             # default 3
    prompt: |
      Fix the review comments and commit the changes.
      Reply with the requested JSON describing each thread.
    allow_commit: true
    timeout: 30m                    # required; bounds the whole loop
    output_file: review-report.json # optional iteration report in the session artifacts
```

在机器人模式下，每轮都会等待列出的某个机器人对当前 head commit 提交评审，然后获取该机器人发起的所有未解决线程（包含所有分页）。在全评审者模式下，会立即获取符合条件的未解决线程，不等待新评审。

两种模式都会将线程数据附加给提示后运行，提示必须只返回 JSON：`{"actions":[{"thread_id":"...","reply":"optional","resolve":true}]}`。Cruise 会通过 `gh api graphql` 发布回复并解决线程（要求 `gh` 已认证）。提示创建的提交会被推送；若仍有未提交的更改，步骤会失败。`github-review` 失败会将运行标记为 `Failed`。该步骤只能直接出现在 `after-pr` 中。

默认的 `reviewers: bots` 保留上述行为，并要求 `bots` 列表非空且无重复。若要处理已有的人类和机器人反馈，可使用 `github-review: { reviewers: all, max-iterations: 3 }`，并省略 `bots` 或将其设为空列表。此模式不会等待新评审。将线程文本传给智能体之前，Cruise 会检查每位作者在仓库中的协作者权限：只有 `write`、`maintain` 或 `admin` 作者符合条件。当没有符合条件的未解决线程时循环成功；若达到迭代上限时仍有符合条件的线程，则循环失败。

<a id="variable-reference"></a>
## 变量参考

提交和 PR 模板使用单独的[允许变量集合](configuration.md#commit-and-pr-templates)，其中不包括 `{plan}`。下表的 `{plan}` 仍表示会话计划文件路径。

| 变量 | 说明 |
|----------|-------------|
| `{input}` | 来自 CLI 参数或 stdin 的初始输入；对于 `--skip-planning` / input-as-plan 会话（`input_as_plan: true`），解析为当前 `plan.md` 的内容 |
| `{prev.output}` | 上一提示的输出，或按子步骤名称为键的并行块 JSON 结果 |
| `{prev.input}` | 上一选项步骤中的用户文本输入 |
| `{prev.stderr}` | 上一命令/提示的 stderr，或按子步骤名称加前缀合并的并行块 stderr 和执行错误 |
| `{prev.success}` | 上一命令是否成功，或并行块是否成功（`true` / `false`；被跳过的子步骤计为成功） |
| `{plan}` | 会话计划文件路径（由 `cruise run` 自动设置） |
| `{file:name}` | 名称为 `name` 的相对会话工件的 UTF-8 内容；仅提示字段支持，并在执行前立即加载 |
| `{plan.language}` | 内置计划提示实际使用的语言（来自 `CRUISE_LANGUAGE_PLAN`、`languages.plan`、旧字段、区域设置推断或默认值） |
| `{pr.number}` | 创建 PR 后可用的 pull request 编号 |
| `{pr.url}` | 创建 PR 后可用的 pull request URL |
| `{pr.language}` | PR 标题/正文生成实际使用的语言（来自 `CRUISE_LANGUAGE_PR`、`languages.pr`、旧字段、区域设置推断或默认值） |

> **注意：** `{model}` **不是**模板变量，它是仅在顶层 `command` 数组中解析的特殊占位符。它不能用于 `prompt`、`prompt_file`、`instruction` 或 `command` 步骤字段。

字面花括号使用 Rust `format!` 风格转义：`{{` → `{`，`}}` → `}`（例如，`"{{input}}"` 解析为字面字符串 `"{input}"`，不会查找 `input`）。未闭合的 `{`、单独出现的 `}`、空的 `{}` 以及引用未定义变量都会导致模板语法错误。
