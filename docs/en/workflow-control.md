# Workflow control and composition

**Languages:** [English](../en/workflow-control.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/workflow-control.md) · [简体中文](../zh/workflow-control.md) · [Index](README.md)

<a id="flow-control"></a>
## Flow Control

<a id="explicit-next-step"></a>
### Explicit next step

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

When `next` is omitted or set to null (`~`), the workflow falls through to the next step in declaration order and ends only after the final step. An explicit `next` value jumps to the named step.

<a id="skipping-a-step"></a>
### Skipping a step

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
### Workflow flags

Declare flags in top-level `flags:` and attach one to a step with `flag: <name>`. A flagged step runs only when its flag is `on` and is skipped when it is `off`.

- **Targets:** a regular step, a group call site, a `workflow_call` call site (applied to every expanded step), a `parallel` parent, and steps under `after-pr`. Steps inside a group definition, a called workflow's own steps, and parallel children cannot use `flag:`.
- **Default off:** every declared flag is off when absent.
- **Set values:** `--flag NAME` enables one flag; `--flag NAME1,NAME2` enables a comma-separated selection, and repeated `--flag` options are allowed. Explicit `NAME=on|off|true|false` assignments set or clear individual values. `cruise run` uses saved values plus CLI overrides without prompting. `cruise exec` and `force_exec` entry points prompt on a TTY when flags are declared and no `-f/--flag` is supplied, except on dry runs (see [Workflow flags](cli-reference.md#cruise-run)); unknown names are rejected.
- **Resume:** saved session values are preserved; supplied `--flag` values override only the named flags.
- **Union with skips:** Flag-disabled steps are combined with the user-selected `skipped_steps`; a step is skipped if either applies.

The `skip` field accepts a static boolean (`true`/`false`) or a variable reference string. When a variable reference is given, the step is skipped if that variable's current value is `"true"`.

<a id="conditional-execution-by-file-existence-whenexists"></a>
### Conditional execution by file existence (`when.exists`)

`when.exists` is a pre-execution condition that **skips the step unless at least one file matches the given glob**. Use it to run a step only when relevant files are present -- for example, a Rust-specific review step that should be a no-op in a repo with no `.rs` files.

```yaml
steps:
  rust-review:
    when:
      exists: "**/*.rs"       # run only if a matching file exists; otherwise skip the step
    prompt: "Review the Rust code and fix any issues."
```

- The glob is evaluated relative to the workflow's working directory. Absolute patterns are used as-is.
- Template variables in the pattern are resolved before globbing, so `exists: "{input}/**/*.rs"` works.
- An empty glob is rejected at config validation time.
- `when.exists` is independent of `skip`: if `skip` already skips the step, the glob is not evaluated at all.

<a id="conditional-execution-file-changed-detection"></a>
### Conditional execution (file-changed detection)

When a step has `if: file-changed: <target>`, the workflow jumps to `<target>` if any files changed during its execution. If no files changed, it continues to the next step normally.

This is designed for loop-back patterns -- for example, re-running tests whenever a review step modifies code:

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
### No file changes detection (`if.no-file-changes`)

When a step has `if.no-file-changes` set to `retry` or `failed`, the configured action is taken if the step completes without modifying any workspace files.

- **`failed`** -- Abort the workflow with an error and transition the session to the `Failed` state. This is useful for detecting cases where an LLM claims to have implemented something but did not actually modify any files.
- **`retry`** -- Re-execute the current step. This is useful for retrying a step until it produces meaningful file changes.

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

**Constraints:**
- The value must be either `retry` or `failed`.
- Cannot be used in `after-pr` steps (rejected at validation time).
- Cannot be used at the group level (`if` in group definitions).
- Parses alongside `if: file-changed` on the same step, but `no-file-changes` disables it: no `file-changed` edge is compiled for that step and no jump to its target ever happens.

<a id="declaring-intentional-no-changes"></a>
#### Declaring intentional no-changes

Not every no-change is a failure to route around -- sometimes the plan explicitly says a step should make no changes (e.g. "don't add tests here"), and an agent that reaches that conclusion again on every retry is giving the correct answer, not stalling. Two ways to tell cruise the no-change is deliberate; either one disables **both** actions (`failed` and `retry`) for that attempt:

- **Output marker** -- a line in the step's raw output starting with `NO_CHANGES_INTENTIONAL: <reason>` (leading whitespace on the line is ignored). Works with every backend (`command:` and all three `sdk:` modes) since it's plain text matching, no tool support required. The marker must anchor the start of a line -- a mid-line mention (quoted in passing, inside a code block, etc.) does not count.
- **Structured SDK response** -- SDK-mode agents can declare intentional no-changes through a schema-validated structured response on prompt steps with an `if.no-file-changes` condition. In classic `command:` mode, use the output marker.

<a id="failure-handling-iffail"></a>
### Failure handling (`if.fail`)

`if.fail` decides what happens when a recoverable step failure occurs: a non-zero exit code from a command step, a recoverable prompt error (including LLM transport errors), a `timeout`, or a `no-file-changes: failed` trigger.

- **`fail: <step-name>`** -- Jump to the named step.
- **`fail: { retry: true }`** -- Re-execute the current step.

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

`if.fail` is subject to the same loop-protection budget as other flow-control jumps (`--max-retries`, else top-level `max_retries`, else 3). The budget covers every graph edge except the user-skip fallback and the group-retry-exhausted exit, so plain sequential advances and explicit `next`/option jumps are counted too: traversing any single budgeted edge more times than the ceiling stops the run with loop protection rather than letting a misconfigured loop run forever.

Conditional cycles are allowed when a normal exit is possible. Execution preflight rejects a reachable cycle only when no normal exit is reachable from the actual start position, considering configured and user-selected skips. A branch that may enter an exitless cycle produces a warning, not a blanket rejection. Runtime edge budgets still apply.

**Constraints:**
- `if.fail` is rejected at the group level and on every `after-pr` step except native `squash-merge: true`, which accepts only a named jump.
- On ordinary steps, `if.fail` can be combined with other `if:` keys (`file-changed`, `no-file-changes`); native squash-merge accepts only its named `if.fail` route.

<a id="quality-gates-quality_gates"></a>
### Quality gates (`quality_gates`)

Use a non-empty list of commands to check a prompt step's work before its transition is applied:

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

Each entry requires a nonblank `command` and may specify a positive `timeout` using the usual seconds, `m`, or `h` syntax. Commands run in order after a successful prompt attempt, including the final step, and stop at the first failure. Exit zero without a timeout or signal passes. Skipped steps, failed prompt attempts, and attempts discarded by `if.no-file-changes: retry` do not run gates. A pending `if.file-changed` transition is applied only if the gates pass.

A failed gate retries the same prompt step before applying its normal transition. The next prompt receives a sanitized, bounded excerpt of the command output as untrusted data. Gate output does not replace `{prev.*}`.

Retries use the existing per-edge budget (`--max-retries`, then top-level `max_retries`, then 3), shared with other self-retries on that step. On exhaustion, `if.fail: <step>` jumps to that step; otherwise cruise stops with `QualityGateExhausted`.

Gates are allowed on prompt and `prompt_file` steps, including individual steps inside a group. They are rejected on parallel parents and children, `after-pr` steps, group or `workflow_call` call sites, and non-prompt steps. Gate commands execute shell commands, just like ordinary `command` steps; only use trusted workflow configurations.

<a id="step-groups"></a>
## Step Groups

Steps can be grouped to coordinate retry loops across multiple steps. A group retries all its member steps together when the `if: file-changed` condition triggers.

Groups can define their steps inline and are invoked from the main `steps` section with `group: <name>`:

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

The same group can be invoked from multiple places in the workflow:

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

**Constraints:**
- Steps inside a group definition cannot have nested `group:` references or individual `if:` conditions -- the group-level `if:` applies to the entire group.
- When the group's `if: file-changed` condition triggers, execution jumps to the configured target step (`test` above); normal sequencing then re-enters the group and its steps re-run.
- A call-site step (e.g. `review-pass: group: review`) cannot have its own `if:` condition.
- A group call site cannot set `allow_commit: true`; set it on the inner prompt step instead.
- A group call site cannot set `output_file` either; a dedicated error points you at the inner prompt step.
- A group call site cannot set `computer_use`; set it on the inner prompt step instead.
- A step inside a group definition cannot use `workflow_call:`; expand the called workflow at the top level instead.
- A step inside a group definition cannot have a `flag:`; put the flag on the group call site instead.

<a id="workflow-composition-workflow_call"></a>
## Workflow Composition (`workflow_call`)

A step can delegate to another workflow config file by setting `workflow_call` instead of `prompt`, `prompt_file`, `command`, `option`, or `parallel`. The called workflow's steps are inlined into the parent at the call site, with each step ID prefixed by the call-site name (e.g. `shared-review/simplify`).

```yaml
steps:
  build:
    command: cargo build

  shared-review:
    workflow_call: ./workflows/review.yaml    # local relative path

  deploy:
    command: cargo publish
```

The referenced file is a regular cruise config. Its top-level execution settings (`command`, `sdk`, `model`, `env`, `computer_use`, etc.) are ignored -- only its `steps` are imported. The parent's settings apply to the expanded steps, while per-step overrides inside the called workflow are preserved.

<a id="required-string-inputs"></a>
### Required string inputs

A complete callee, `workflows/review.yaml`:

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

Its complete caller, `cruise.yaml`:

```yaml
create_pr: false
steps:
  shared-review:
    workflow_call: ./workflows/review.yaml
    with:
      review_prompt: "Review the changes for {input}, using {plan}."
      label: "review"
```

`inputs` is an optional list of unique names matching `[A-Za-z_][A-Za-z0-9_]*`. Every declared input is required. `with` is a string-valued mapping: empty strings are valid, but numbers, booleans, arrays, and objects are not converted to strings. Missing or extra arguments and undeclared input references fail before execution. Omitting `with` or setting it to `null` passes no arguments. Even an empty mapping is allowed only on a `workflow_call` step. A directly loaded root cannot declare nonempty `inputs`, and `inputs` is forbidden in user/project `default.yaml`.

`{inputs.name}` binds at config-load time in prompt text (including loaded `prompt_file` contents), instruction, command strings/array elements, env values, and nested call `with` values. It does not parameterize paths, step names, transitions, flags, or models.

Runtime tokens such as `{input}`, `{plan}`, `{prev.*}`, `{pr.*}`, and `{file:...}` remain for normal execution-time evaluation. Inputs are template fragments, not a secret store or an automatic shell-quoting mechanism.

<a id="return-labels"></a>
### Return labels

A called workflow can declare named exits, leaving the caller to choose their
destinations. For example, `workflows/review-gate.yaml`:

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

The caller maps every declared label to a step in its own step map:

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

`returns` is a unique list of labels matching `[A-Za-z_][A-Za-z0-9_-]*`. Every declared label must be used by a callee return and mapped by the caller, with no extra mappings. `on_return` belongs only on a `workflow_call` and null means omission. Destinations must exist in the same parent step map and cannot be workflow call sites. A callee's top-level step may use `return` instead of `next`; on an option step put `return` on individual options instead. A return cannot be combined with `next` on the same step or option, or with `if.file-changed` on the return-bearing step. Returns are not allowed on group or parallel children, call sites, or after-PR steps.

Without a return, normal completion follows the call-site `next` if present, or continues to the next parent step. A skipped return-bearing step does not return; it continues in declaration order, even if the call site has `next`. A call-site `next` is invalid when the callee's final step has a step-level return, but is allowed when returns are on options.

<a id="supported-sources"></a>
### Supported sources

| Source | Example |
|--------|---------|
| Local relative path | `workflow_call: ./workflows/review.yaml` |
| GitHub blob URL | `workflow_call: https://github.com/org/repo/blob/main/workflows/review.yaml` |
| GitHub raw URL | `workflow_call: https://raw.githubusercontent.com/org/repo/main/workflows/review.yaml` |

`cruise workflow eject <name> [--to user|project]` expands calls and file-backed prompts into their contents, then writes a self-contained, editable workflow.

<a id="call-site-fields"></a>
### Call-site fields

A `workflow_call` step is a pure delegation point. Only `with`, `on_return`, `skip`, `when`, `next`, and `flag` may be set alongside `workflow_call`:

- `skip` and `when` are applied to the **first** expanded step.
- `next` is applied to the **last** expanded step (when it has no explicit `next` of its own).
- `flag` is applied to every expanded step. A called workflow's own steps cannot use `flag:`.

All other step fields (`prompt`, `prompt_file`, `command`, `model`, `instruction`, `plan`, `option`, `if`, `timeout`, `env`, `output_file`, `group`, `parallel`, `computer_use`, `permission`, `disallowed_tools`) and `allow_commit: true` are rejected at validation time, and the error names every offending field. An explicit `allow_commit: false` is equivalent to omission.

<a id="nesting-and-cycle-detection"></a>
### Nesting and cycle detection

Workflow calls can be nested: a called workflow may itself contain `workflow_call` steps. Step IDs accumulate prefixes (`outer/inner/step`). Circular references (A calls B, B calls A) are detected and rejected. Groups and `workflow_call` do not mix in either direction: groups inside called workflows are not supported, and a step inside a group definition cannot itself be a `workflow_call`.

```yaml
# parent.yaml -> nested/outer.yaml -> inner/leaf.yaml
# Results in step IDs: outer-call/leaf-call/leaf
steps:
  outer-call:
    workflow_call: ./nested/outer.yaml
```
