[English](../en/workflow-control.md) | [日本語](../ja/workflow-control.md) | [简体中文](../zh/workflow-control.md) | [返回目录](README.md)

# 工作流控制与组合

<a id="flow-control"></a>
## 流程控制

<a id="explicit-next-step"></a>
### 显式指定下一步

```yaml
steps:
  step_a:
    command: echo "hello"
    next: step_c                  # jump over step_b
  step_b:
    command: echo "skipped"
  step_c:
    command: echo "world"
```


当省略 `next` 或将其设为 null（`~`）时，工作流会按声明顺序继续执行下一步，直到最后一个步骤执行完毕后才结束。显式指定 `next` 值则会跳转到指定名称的步骤。

<a id="skipping-a-step"></a>
### 跳过步骤

```yaml
steps:
  optional_step:
    command: cargo fmt
    skip: true                    # always skip

  fix_errors:
    command: cargo fix
    skip: prev.success            # skip if the variable "prev.success" resolves to "true"
```


<a id="workflow-flags"></a>
### 工作流标志

在顶层 `flags:` 中声明标志，并通过 `flag: <name>` 将某个标志关联到步骤。带有标志的步骤仅在其标志为 `on` 时运行，为 `off` 时则会跳过。

- **适用位置：** 普通步骤、组调用处、`workflow_call` 调用处（应用于展开后的每个步骤）、`parallel` 父步骤，以及 `after-pr` 下的步骤。组定义内部的步骤、被调用工作流自身的步骤以及并行子步骤不能使用 `flag:`。
- **默认为 off：** 每个已声明的标志在未指定时均为 off。
- **设置值：** `--flag NAME` 启用一个标志；`--flag NAME1,NAME2` 启用以逗号分隔的一组标志，也允许重复使用 `--flag` 选项。显式使用 `NAME=on|off|true|false` 赋值可设置或清除各个标志的值。`cruise run` 使用已保存的值并应用 CLI 覆盖值，不会提示用户输入。在声明了标志且未提供 `-f/--flag` 时，`cruise exec` 和 `force_exec` 入口会在 TTY 上提示用户输入，但试运行除外（参见[工作流标志](cli-reference.md#cruise-run)）；未知名称会被拒绝。
- **恢复：** 已保存的会话值会予以保留；提供的 `--flag` 值只会覆盖指定名称的标志。
- **与跳过项取并集：** 被标志禁用的步骤会与用户选择的 `skipped_steps` 合并；只要满足其中任一条件，步骤就会被跳过。CLI、TUI 和 WebUI 的跳过选择器，以及 WebUI 和 TUI 的图选项卡，仅在标志启用时显示带有该标志的步骤，但旧版无版本号的图检查点会不经筛选直接显示。新会话会移除被标志禁用的步骤所对应的跳过条目，以免稍后启用标志时仍保留隐藏的跳过设置。

`skip` 字段接受静态布尔值（`true`/`false`）或变量引用字符串。提供变量引用时，如果该变量的当前值为 `"true"`，则会跳过该步骤。

<a id="conditional-execution-by-file-existence-whenexists"></a>
### 根据文件是否存在有条件地执行（`when.exists`）

`when.exists` 是一种执行前条件，**除非至少有一个文件与给定的 glob 模式匹配，否则会跳过该步骤**。使用它可让步骤仅在存在相关文件时运行——例如，专门针对 Rust 的审查步骤，在没有 `.rs` 文件的仓库中应不执行任何操作。

```yaml
steps:
  rust-review:
    when:
      exists: "**/*.rs"       # run only if a matching file exists; otherwise skip the step
    prompt: "Review the Rust code and fix any issues."
```


- glob 模式相对于工作流的工作目录进行求值。绝对路径模式按原样使用。
- 模式中的模板变量会在进行 glob 匹配前解析，因此 `exists: "{input}/**/*.rs"` 可以正常工作。
- **无匹配项 -> 跳过步骤**（显示为 `skipping: <step> (no files match when.exists)`）。有一个或多个匹配项 -> 步骤正常运行。
- 空的 glob 模式会在配置验证时被拒绝，语法无效的模式也一样——但模式包含 `{` 时除外，此时会跳过静态验证，因为花括号可能属于模板变量。此类模式只在替换后检查；如果 glob 模式无效，步骤会在运行时失败。
- 如果扫描时无法读取某些条目（例如遇到权限错误），cruise 会倾向于运行该步骤，而不是静默跳过。
- `when.exists` 独立于 `skip`：如果 `skip` 已使该步骤被跳过，就完全不会对 glob 模式求值。

<a id="conditional-execution-file-changed-detection"></a>
### 条件执行（文件变更检测）

当步骤具有 `if: file-changed: <target>` 时，会在该步骤运行**之前**创建工作目录的快照。步骤执行后，如果其执行期间有任何文件发生变更，工作流就会跳转到 `<target>`。如果没有文件发生变更，工作流会正常继续执行下一步。

此功能用于回跳模式——例如，每当审查步骤修改代码时，就重新运行测试：

```yaml
steps:
  test:
    command: cargo test

  review:
    prompt: "Review the code and fix any issues."
    if:
      file-changed: test    # after review, if it modified files, jump back to test
```


> **注意：** 快照是在带有 `if:` 条件的步骤运行**之前**创建的。如果步骤执行期间没有文件发生变更，工作流会继续执行下一步（如果设置了 `next:` 字段，则按该字段跳转）。

> 当能够正常退出时，允许存在条件循环。执行前预检会考虑配置的跳过项和用户选择的跳过项，只有从实际起始位置无法到达任何正常出口时，才会拒绝可达的循环。对于可能进入无出口循环的分支，只会发出警告，而不会一概拒绝。运行时的边遍历次数预算仍然适用。

<a id="no-file-changes-detection-ifno-file-changes"></a>
### 无文件变更检测（`if.no-file-changes`）

当步骤将 `if.no-file-changes` 设为 `retry` 或 `failed` 时，会在该步骤运行**之前**创建工作目录的快照。如果步骤完成时没有修改任何工作区文件，就会执行配置的操作。有两种可用模式：

- **`failed`** —— 报错并中止工作流，将会话转为 `Failed` 状态。这有助于检测 LLM 声称已实现某项功能、但实际上没有修改任何文件的情况。
- **`retry`** —— 重新执行当前步骤。这适合用于不断重试某个步骤，直到它产生有意义的文件变更。

```yaml
steps:
  implement:
    prompt: "Implement the feature described in {plan}"
    if:
      no-file-changes: failed

  fix:
    prompt: "Fix the issue"
    if:
      no-file-changes: retry
```


**限制：**
- 值必须为 `retry` 或 `failed`；任何其他值（包括已移除的对象形式 `{ fail: true }` / `{ retry: true }`）都会导致解析错误。
- 不能用于 `after-pr` 步骤（会在验证时被拒绝）。
- 不能用于组级别（组定义中的 `if`）。
- 可以与同一步骤上的 `if: file-changed` 一起解析，但 `no-file-changes` 会禁用后者：不会为该步骤编译 `file-changed` 边，也绝不会跳转到其目标。
- 旧字段 `fail-if-no-file-changes: true` 会被作为未知字段拒绝；请迁移为 `if: { no-file-changes: failed }`。

<a id="declaring-intentional-no-changes"></a>
#### 声明有意不作更改

并非每次未作更改都是需要绕开的失败——有时计划会明确要求某个步骤不作更改（例如“不要在这里添加测试”），而智能体在每次重试时都再次得出这个结论，给出的正是正确答案，并非停滞不前。可以通过两种方式告知 cruise，未作更改是有意为之；任一方式都会在本次尝试中禁用**两种**操作（`failed` 和 `retry`）：

- **输出标记** —— 在步骤的原始输出中包含以 `NO_CHANGES_INTENTIONAL: <reason>` 开头的一行（会忽略该行的前导空白）。适用于所有后端（`command:` 和全部三种 `sdk:` 模式），因为它只是纯文本匹配，不需要工具支持。标记必须位于行首——在行中间提及它（例如顺带引用、在代码块内提及等）不算。
- **结构化 SDK 响应** —— SDK 模式下的智能体，可以在带有 `if.no-file-changes` 条件的提示步骤中，通过经过 schema 验证的结构化响应声明有意不作更改。在传统 `command:` 模式下，请使用输出标记。

无论采用哪种方式，都会记录所声明的原因，让这一决定在运行输出中保持可见。并行子步骤不能替其父步骤作出此声明：子步骤的输出标记仍位于聚合 JSON 内，且子步骤不会收到该结构化响应。父步骤的 `if.no-file-changes` 条件仍会评估整个块中的文件变更。

<a id="failure-handling-iffail"></a>
### 失败处理（`if.fail`）

`if.fail` 决定发生可恢复的步骤失败时如何处理：命令步骤返回非零退出码、发生可恢复的提示错误（包括 LLM 传输错误）、发生 `timeout`，或触发 `no-file-changes: failed`。jcode 提示中的模型缺失错误无法通过 `if.fail` 恢复；它会绕过处理器并中止工作流。

以下两种形式具有实际作用（其他映射，例如 `{}` 或 `{ retry: false }`，可以解析，但只会直接继续执行下一步）：

- **`fail: <step-name>`** —— 跳转到指定名称的步骤。
- **`fail: { retry: true }`** —— 重新执行当前步骤。

```yaml
steps:
  flaky_test:
    command: cargo test --flaky
    timeout: 2m
    if:
      fail:
        retry: true        # retry on non-zero exit, timeout, or other failure

  deploy:
    command: ./deploy.sh
    if:
      fail: rollback       # jump to the `rollback` step on failure

  rollback:
    command: ./rollback.sh
```


`if.fail` 与其他流程控制跳转受相同的循环保护预算约束（使用 `--max-retries`，否则使用顶层 `max_retries`，再否则使用 3）。预算涵盖所有图边，但用户跳过时的回退边和组重试耗尽后的退出边除外，因此普通的顺序推进以及显式的 `next`/选项跳转也会计数：任何单条受预算约束的边，其遍历次数超过上限时，运行都会因循环保护而停止，而不会让配置错误的循环无限运行。

当能够正常退出时，允许存在条件循环。执行前预检会考虑配置的跳过项和用户选择的跳过项，只有从实际起始位置无法到达任何正常出口时，才会拒绝可达的循环。对于可能进入无出口循环的分支，只会发出警告，而不会一概拒绝。运行时的边遍历次数预算仍然适用。

**限制：**
- 组级别以及所有 `after-pr` 步骤均不接受 `if.fail`，但原生 `squash-merge: true` 除外，它只接受跳转到指定名称步骤的形式。
- 在普通步骤中，`if.fail` 可以与其他 `if:` 键（`file-changed`、`no-file-changes`）组合使用；原生 squash-merge 则只接受指向指定名称步骤的 `if.fail` 路由。

<a id="quality-gates-quality_gates"></a>
### 质量门禁（`quality_gates`）

使用非空的命令列表，在应用提示步骤的转移之前检查该步骤的工作结果：

```yaml
max_retries: 2
steps:
  implement:
    prompt: Implement the requested change.
    quality_gates:
      - command: cargo test
      - command: cargo clippy
        timeout: 10m
    if:
      fail: recover
    next: done
  recover:
    prompt: Investigate the failed implementation.
  done:
    command: echo done
```


每个条目都必须包含非空白的 `command`，并且可以使用常规的秒数、`m` 或 `h` 语法指定一个正的 `timeout`。命令会在提示尝试成功后按顺序运行，包括最后一个步骤，并在首次失败时停止。退出码为零，且未超时、未收到信号时，视为通过。已跳过的步骤、失败的提示尝试，以及被 `if.no-file-changes: retry` 丢弃的尝试都不会运行门禁。只有门禁通过后，才会应用待执行的 `if.file-changed` 转移。

门禁失败时，会先重试同一个提示步骤，再应用其正常转移。下一次提示会收到经过清理且长度受限的命令输出片段，并将其作为不可信数据处理。如有可用日志，经过清理的日志会保存在会话的 `artifacts/quality-gates/` 目录中；该目录位于仓库之外，日志受制品大小限制约束。反馈在保存检查点及恢复后仍会保留，并在该次重试不再处于待执行状态后清除。门禁输出不会替换 `{prev.*}`。

重试使用现有的逐边预算（依次采用 `--max-retries`、`max_retries`、3），并与该步骤上的其他自重试共享预算。在此前没有发生自重试的情况下，门禁最多尝试 `1 + max_retries` 次；零表示首次失败就会耗尽预算。预算耗尽时，`if.fail: <step>` 会跳转到该步骤；否则，cruise 会报出 `QualityGateExhausted` 并停止运行。映射 `if.fail: {retry: true}` 不会额外增加一份门禁重试预算。

允许在提示步骤和 `prompt_file` 步骤上使用门禁，包括组内的各个步骤。并行父步骤和子步骤、`after-pr` 步骤、组或 `workflow_call` 调用处，以及非提示步骤均不接受门禁。门禁命令与普通 `command` 步骤一样，会执行 shell 命令；请仅使用可信的工作流配置。

<a id="step-groups"></a>
## 步骤组

步骤可以分组，以协调跨多个步骤的重试循环。当 `if: file-changed` 条件触发时，组会一起重试其所有成员步骤。

组可以内联定义自己的步骤，并在主 `steps` 部分通过 `group: <name>` 调用：

```yaml
max_retries: 4

groups:
  review:
    if:
      file-changed: test    # if any step in the group changes files, jump to `test`; sequencing then re-enters the group
    max_retries: 3          # maximum number of group-level retry loops (optional)
    steps:                  # steps defined inside the group
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  test:
    command: cargo test

  review-pass:
    group: review           # invokes the "review" group's steps at this point
```


同一个组可以在工作流中的多个位置调用：

```yaml
steps:
  test-lib:
    command: cargo test --lib
  review-lib:
    group: review

  test-doc:
    command: cargo test --doc
  review-doc:
    group: review           # same group, different call site
```


**限制：**
- 组定义内的步骤不能包含嵌套的 `group:` 引用，也不能有单独的 `if:` 条件——组级别的 `if:` 适用于整个组。
- 当组的 `if: file-changed` 条件触发时，执行会跳转到配置的目标步骤（上例中的 `test`）；随后，正常的执行顺序会再次进入该组，并重新运行其中的步骤。
- 调用点步骤（例如 `review-pass: group: review`）不能有自己的 `if:` 条件。
- 组调用点不能设置 `allow_commit: true`；应改为在组内的 prompt 步骤上设置。
- 组调用点也不能设置 `output_file`；会有一条专门的错误提示，指引你到组内的 prompt 步骤。
- 组调用点不能设置 `computer_use`；应改为在组内的 prompt 步骤上设置。
- 组定义内的步骤不能使用 `workflow_call:`；应改为在顶层展开被调用的工作流。
- 组定义内的步骤不能有 `flag:`；应改为将该标志放在组调用点上。

<a id="workflow-composition-workflow_call"></a>
## 工作流组合（`workflow_call`）

步骤可以通过设置 `workflow_call`（而不是 `prompt`、`prompt_file`、`command`、`option` 或 `parallel`），将执行委托给另一个工作流配置文件。被调用工作流的步骤会在调用点内联展开到父工作流中，每个步骤 ID 都以调用点的名称为前缀（例如 `shared-review/simplify`）。

```yaml
steps:
  build:
    command: cargo build

  shared-review:
    workflow_call: ./workflows/review.yaml    # local relative path

  deploy:
    command: cargo publish
```


引用的文件是一个普通的 cruise 配置文件。其顶层执行设置（`command`、`sdk`、`model`、`env`、`computer_use` 等）会被忽略——只导入其 `steps`。父工作流的设置适用于展开后的步骤，而被调用工作流中针对单个步骤的覆盖设置会被保留。

<a id="required-string-inputs"></a>
### 必需的字符串输入

完整的被调用工作流 `workflows/review.yaml`：

```yaml
inputs: [review_prompt, label]
steps:
  review:
    prompt: "{inputs.review_prompt}"
  announce:
    env:
      REVIEW_LABEL: "{inputs.label}"
    command: 'printf "%s\n" "$REVIEW_LABEL"'
```


调用它的完整工作流 `cruise.yaml`：

```yaml
create_pr: false
steps:
  shared-review:
    workflow_call: ./workflows/review.yaml
    with:
      review_prompt: "Review the changes for {input}, using {plan}."
      label: "review"
```


`inputs` 是一个可选列表，其中的名称必须唯一，且匹配 `[A-Za-z_][A-Za-z0-9_]*`。每个已声明的输入都是必需的。`with` 是一个值为字符串的映射：允许使用空字符串，但数字、布尔值、数组和对象不会被转换为字符串。缺少参数、存在多余参数或引用了未声明的输入，都会在执行前导致失败。省略 `with` 或将其设置为 `null` 都表示不传递任何参数。`with` 即使是空映射，也只能出现在 `workflow_call` 步骤上。直接加载的根配置不能声明非空的 `inputs`，并且用户/项目的 `default.yaml` 中禁止使用 `inputs`。不支持输入默认值、可选输入或 CLI 输入参数。

`{inputs.name}` 会在配置加载时，在提示文本（包括已加载的 `prompt_file` 内容）、instruction、command 字符串/数组元素、env 值以及嵌套调用的 `with` 值中进行绑定，也包括并行的 prompt/command 子项。它不会对路径、步骤名称、转移、标志或模型进行参数化。在绑定之前，`prompt_file` 路径会相对于声明它的源文件进行解析。每个被调用方都有自己的词法作用域：父级和同级不会隐式共享输入。应显式转发，例如，在被调用方声明了 `label` 的嵌套调用上使用 `with: {label: "{inputs.label}"}`。

在同一轮绑定过程中，不会重新扫描已插入的值。`{input}`、`{plan}`、`{prev.*}`、`{pr.*}` 和 `{file:...}` 等运行时标记会保留下来，供执行时按常规方式求值。转义在加载时保持不变：`{{inputs.label}}` 不会触发输入查找，而是在运行时变为字面量 `{inputs.label}`。现有的 `{{...}}` 和 `}}` 花括号规则仍然适用。输入是模板片段，而不是机密存储库或自动 shell 引号处理机制。

<a id="return-labels"></a>
### 返回标签

被调用的工作流可以声明具名出口，由调用方选择这些出口的目标。例如，`workflows/review-gate.yaml`：

```yaml
returns: [approved, needs_fix]
steps:
  review:
    command: "cargo test"
  decide:
    option:
      - selector: Approve
        return: approved
      - selector: Request changes
        return: needs_fix
```


调用方将每个声明的标签映射到其自身步骤映射中的一个步骤：

```yaml
steps:
  gate:
    workflow_call: ./workflows/review-gate.yaml
    on_return: {approved: publish, needs_fix: fix}
  fix:
    prompt: "Fix the review findings in {plan}"
  publish:
    command: "git push"
```


`returns` 是一个不含重复项的标签列表，其中的标签须匹配 `[A-Za-z_][A-Za-z0-9_-]*`。每个声明的标签都必须由被调用方的某个返回操作使用，并由调用方映射，且不得有多余的映射。`on_return` 只能用于 `workflow_call`，null 表示省略。目标必须存在于同一个父级步骤映射中，且不能是工作流调用点。被调用工作流的顶层步骤可以使用 `return` 代替 `next`；对于选项步骤，则应将 `return` 放在各个选项上。返回不能与同一步骤或选项上的 `next` 同时使用，也不能与包含该返回的步骤上的 `if.file-changed` 同时使用。不允许在 group 或 parallel 的子步骤、调用点或 PR 后步骤上使用返回。直接加载的根工作流不能声明非空的 `returns`，也不能使用返回路由，且禁止在用户/项目级 `default.yaml` 中使用 `returns`。

路由会在加载时转换为显式的 `next` 边。如果未发生返回而正常完成，则保留调用点的 `next`；若调用点没有 `next`，则继续执行父级的下一个步骤。包含返回的步骤被跳过时不会返回，而是按声明顺序继续执行，即使调用点设置了 `next` 也是如此。如果被调用工作流的最后一个步骤具有步骤级返回，则调用点的 `next` 不合法；但返回位于选项上时则允许使用。嵌套调用必须在每个作用域中映射标签；标签不会向外传播。路由可以循环回到父级中较早的步骤，但仍须满足现有图验证的要求，即必须存在正常出口。

<a id="supported-sources"></a>
### 支持的来源

| 来源 | 示例 |
|--------|---------|
| 本地相对路径 | `workflow_call: ./workflows/review.yaml` |
| GitHub blob URL | `workflow_call: https://github.com/org/repo/blob/main/workflows/review.yaml` |
| GitHub raw URL | `workflow_call: https://raw.githubusercontent.com/org/repo/main/workflows/review.yaml` |

GitHub 工作流会在加载配置时通过 `gh api` 获取。GitHub 托管的工作流中的相对路径以远程目录为基准解析，因此嵌套引用可以跨仓库使用。

已安装的内置工作流从嵌入式资源中解析其共享依赖，绝不会使用同名的本地文件。将 `builtin/*.yaml` 作为普通本地文件加载时，则相对于这些文件解析依赖。内联 YAML 使用为其提供的基准目录来解析本地依赖，而不使用嵌入式注册表。共享的 PR 后资源属于内部资源，既不是目录条目，也不是新的公开 `workflow_call` 来源语法。

六个通用内置工作流均按顺序调用同一段包含六个步骤的 PR 后前置流程：
`pr-ready` → `sync-base` → `resolve-conflict` → `push-after-sync` → `wait-ci` →
`fix-ci-error`。每个根工作流都通过 `with` 提供其工作流专用的冲突处理和 CI 提示词。`follow-up` 调用的 `flag: [after_pr, merge]` 控制所有六个步骤是否执行；被调用工作流本身没有标志。新会话使用 `follow-up/wait-ci` 这样的 ID，内部跳转也会加上同一个调用的前缀。原生的 `squash-merge` 仍是根工作流中直接定义的最后一个 PR 后步骤，仅由 `merge` 控制执行。这两个标志默认仍为关闭状态，`research` 也保留 `create_pr: false`。

`cruise workflow eject <name> [--to user|project]` 会写出一个自包含、可编辑的工作流，其中的调用和基于文件的提示词均已展开为实际内容。它会保留根级字段的省略状态，不会将宿主模型、环境覆盖设置或用户/项目默认值固化到工作流中。绝不会覆盖现有文件。已安装的软件包以及新建的内置/仓库/内联会话快照同样会保留解析后的内容，而不包含 `inputs`、`with`、`workflow_call` 或 `prompt_file` 元数据。不会迁移现有快照和已保存的步骤 ID。基于文件的会话仍保留其现有的实时重载行为。

<a id="call-site-fields"></a>
### 调用点字段

`workflow_call` 步骤是一个纯粹的委派点。除 `workflow_call` 外，只能设置 `with`、`on_return`、`skip`、`when`、`next` 和 `flag`：

- `skip` 和 `when` 应用于展开后的**第一个**步骤。
- `next` 应用于展开后的**最后一个**步骤（前提是该步骤自身没有显式的 `next`）。
- `flag` 应用于展开后的每个步骤。被调用工作流自身的步骤不能使用 `flag:`。

所有其他步骤字段（`prompt`、`prompt_file`、`command`、`model`、`instruction`、`plan`、`option`、`if`、`timeout`、`env`、`output_file`、`group`、`parallel`、`computer_use`、`permission`、`disallowed_tools`）以及 `allow_commit: true` 都会在验证时被拒绝，且错误消息会列出所有违规字段的名称。显式设置 `allow_commit: false` 等同于省略该字段。

<a id="nesting-and-cycle-detection"></a>
### 嵌套与循环检测

工作流调用可以嵌套：被调用的工作流本身也可以包含 `workflow_call` 步骤。步骤 ID 会逐层累加前缀（`outer/inner/step`）。循环引用（A 调用 B，B 调用 A）会被检测并拒绝。分组与 `workflow_call` 在两个方向上都不能混用：不支持在被调用工作流中使用分组，且分组定义中的步骤本身也不能是 `workflow_call`。

```yaml
# parent.yaml -> nested/outer.yaml -> inner/leaf.yaml
# Results in step IDs: outer-call/leaf-call/leaf
steps:
  outer-call:
    workflow_call: ./nested/outer.yaml
```

