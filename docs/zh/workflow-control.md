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
- **与跳过项取并集：** 被标志禁用的步骤会与用户选择的 `skipped_steps` 合并；只要满足其中任一条件，步骤就会被跳过。

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
- 空的 glob 模式会在配置验证时被拒绝。
- `when.exists` 独立于 `skip`：如果 `skip` 已使该步骤被跳过，就完全不会对 glob 模式求值。

<a id="conditional-execution-file-changed-detection"></a>
### 条件执行（文件变更检测）

当步骤具有 `if: file-changed: <target>` 时，如果步骤执行期间有任何文件发生变更，工作流就会跳转到 `<target>`。如果没有文件发生变更，工作流会正常继续执行下一步。

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

<a id="no-file-changes-detection-ifno-file-changes"></a>
### 无文件变更检测（`if.no-file-changes`）

当步骤将 `if.no-file-changes` 设为 `retry` 或 `failed` 时，如果步骤完成时没有修改任何工作区文件，就会执行配置的操作。

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
- 值必须为 `retry` 或 `failed`。
- 不能用于 `after-pr` 步骤（会在验证时被拒绝）。
- 不能用于组级别（组定义中的 `if`）。
- 可以与同一步骤上的 `if: file-changed` 一起解析，但 `no-file-changes` 会禁用后者：不会为该步骤编译 `file-changed` 边，也绝不会跳转到其目标。

<a id="declaring-intentional-no-changes"></a>
#### 声明有意不作更改

并非每次未作更改都是需要绕开的失败——有时计划会明确要求某个步骤不作更改（例如“不要在这里添加测试”），而智能体在每次重试时都再次得出这个结论，给出的正是正确答案，并非停滞不前。可以通过两种方式告知 cruise，未作更改是有意为之；任一方式都会在本次尝试中禁用**两种**操作（`failed` 和 `retry`）：

- **输出标记** —— 在步骤的原始输出中包含以 `NO_CHANGES_INTENTIONAL: <reason>` 开头的一行（会忽略该行的前导空白）。适用于所有后端（`command:` 和全部三种 `sdk:` 模式），因为它只是纯文本匹配，不需要工具支持。标记必须位于行首——在行中间提及它（例如顺带引用、在代码块内提及等）不算。
- **结构化 SDK 响应** —— SDK 模式下的智能体，可以在带有 `if.no-file-changes` 条件的提示步骤中，通过经过 schema 验证的结构化响应声明有意不作更改。在传统 `command:` 模式下，请使用输出标记。

<a id="failure-handling-iffail"></a>
### 失败处理（`if.fail`）

`if.fail` 决定发生可恢复的步骤失败时如何处理：命令步骤返回非零退出码、发生可恢复的提示错误（包括 LLM 传输错误）、发生 `timeout`，或触发 `no-file-changes: failed`。

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

门禁失败时，会先重试同一个提示步骤，再应用其正常转移。下一次提示会收到经过清理且长度受限的命令输出片段，并将其作为不可信数据处理。门禁输出不会替换 `{prev.*}`。

重试遵循现有的逐边预算（`--max-retries`、否则为顶层 `max_retries`、再否则为 3），并与该步骤上的其他自重试共享预算。预算耗尽时，`if.fail: <step>` 会跳转到该步骤；否则，cruise 会报出 `QualityGateExhausted` 并停止运行。

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

`inputs` 是一个可选列表，其中的名称必须唯一，且匹配 `[A-Za-z_][A-Za-z0-9_]*`。每个已声明的输入都是必需的。`with` 是一个值为字符串的映射：允许使用空字符串，但数字、布尔值、数组和对象不会被转换为字符串。缺少参数、存在多余参数或引用了未声明的输入，都会在执行前导致失败。省略 `with` 或将其设置为 `null` 都表示不传递任何参数。`with` 即使是空映射，也只能出现在 `workflow_call` 步骤上。直接加载的根配置不能声明非空的 `inputs`，并且用户/项目的 `default.yaml` 中禁止使用 `inputs`。

`{inputs.name}` 会在配置加载时，在提示文本（包括已加载的 `prompt_file` 内容）、instruction、command 字符串/数组元素、env 值以及嵌套调用的 `with` 值中进行绑定。它不会对路径、步骤名称、转移、标志或模型进行参数化。

`{input}`、`{plan}`、`{prev.*}`、`{pr.*}` 和 `{file:...}` 等运行时标记会保留下来，供执行时按常规方式求值。输入是模板片段，而不是秘密存储库或自动 shell 引号处理机制。

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

`returns` 是一个不含重复项的标签列表，其中的标签须匹配 `[A-Za-z_][A-Za-z0-9_-]*`。每个声明的标签都必须由被调用方的某个返回操作使用，并由调用方映射，且不得有多余的映射。`on_return` 只能用于 `workflow_call`，null 表示省略。目标必须存在于同一个父级步骤映射中，且不能是工作流调用点。被调用工作流的顶层步骤可以使用 `return` 代替 `next`；对于选项步骤，则应将 `return` 放在各个选项上。返回不能与同一步骤或选项上的 `next` 同时使用，也不能与包含该返回的步骤上的 `if.file-changed` 同时使用。不允许在 group 或 parallel 的子步骤、调用点或 PR 后步骤上使用返回。

如果未发生返回而正常完成，则保留调用点的 `next`；若调用点没有 `next`，则继续执行父级的下一个步骤。包含返回的步骤被跳过时不会返回，而是按声明顺序继续执行，即使调用点设置了 `next` 也是如此。如果被调用工作流的最后一个步骤具有步骤级返回，则调用点的 `next` 不合法；但返回位于选项上时则允许使用。

<a id="supported-sources"></a>
### 支持的来源

| 来源 | 示例 |
|--------|---------|
| 本地相对路径 | `workflow_call: ./workflows/review.yaml` |
| GitHub blob URL | `workflow_call: https://github.com/org/repo/blob/main/workflows/review.yaml` |
| GitHub raw URL | `workflow_call: https://raw.githubusercontent.com/org/repo/main/workflows/review.yaml` |

`cruise workflow eject <name> [--to user|project]` 会写出一个自包含、可编辑的工作流，并将调用和基于文件的提示词展开为实际内容。

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
