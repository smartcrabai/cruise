# Workflow step reference

**Languages:** [English](../en/steps.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/steps.md) · [简体中文](../zh/steps.md) · [Index](README.md)

<a id="step-types"></a>
## Step Types

<a id="prompt-step-llm-call"></a>
### Prompt Step (LLM call)

Prompt steps can declare `quality_gates` to verify a successful attempt before advancing; see [Quality gates](workflow-control.md#quality-gates-quality_gates).

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

When `output_file` is set, cruise saves the completed backend response in the session's `artifacts/` directory outside the workspace. Later prompt bodies can read it with `{file:initial-state.md}`.

An `instruction` is a message shown to the user immediately before a step runs, not a system prompt. If `{input}` is empty, the message is used as a multiline input prompt and the submitted text becomes `{input}`; otherwise it is displayed without being sent to the LLM.

Prompt steps are commit-guarded by default. SDK prompts also receive the implicit Git shell denials described under [Tool restrictions](sdk.md#tool-restrictions). `allow_commit: true` removes those denials and the guard, but not explicit `disallowed_tools`, and never grants broader permissions. Set it only when a prompt intentionally needs to create a commit or otherwise move Git `HEAD`; omitted and `false` values keep the guard enabled. The guard covers classic `command:` mode and all SDK backends (`jcode`, `claude`, and `opencode`), while command and option steps remain unaffected.
Set `allow_commit: true` on the actual prompt, not on a `group:` or `workflow_call:` invocation.

For longer prompts, load the prompt body from a file with `prompt_file`:

```yaml
steps:
  implement:
    prompt_file: prompts/implement.md
```

| `prompt_file` value | Resolution |
| --- | --- |
| Absolute path | Used as-is |
| `~/...`, `~`, or `~user/...` | Expanded from the current user's or named user's home directory |
| `./...`, `../...`, or a bare file name | Relative to the directory containing the config file |
| GitHub blob/raw URL | Fetched from GitHub |
| Relative path inside a called workflow | Relative to the called workflow's directory (or remote directory) |

Absolute and `~` paths refer to the local filesystem. In a GitHub-hosted
workflow, relative non-URL values are resolved as paths in the remote directory; local
`~` paths are rejected. A direct GitHub blob/raw URL can be used explicitly.

<a id="command-step-shell-execution"></a>
### Command Step (shell execution)

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
### Parallel Steps

Use `parallel:` to run named prompt or command steps concurrently within one
session. The next step starts after every child finishes.

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

- All children start together in the session's existing working directory.
  Use independent commands or file edits; cruise does not isolate or merge child
  changes. Concurrent edits to the same files or Git state can conflict.
- Each child receives the same input and `{prev.*}` values from before the block. Environment precedence is workflow < parallel block < child. Children can use `model`, `env`, `skip`, `when`, and `timeout`; prompt children may also set `instruction`, `computer_use`, `permission`, `disallowed_tools` (the parent cannot), and `output_file`. Parallel `instruction` is noninteractive and requires nonempty runtime input. Supply input or use a preceding sequential instruction step; otherwise execution returns an error without opening an input editor. Command arrays remain sequential per child.
- After joining, `{prev.output}` is a JSON object keyed by child name in YAML declaration order. Each entry has `output` (prompt text, or `null` for commands), `stderr` (including execution errors), `success`, and `skipped`. `{prev.success}` is true when every child succeeded or was skipped; `{prev.stderr}` combines child errors with their names.
- A child failure does not cancel siblings. The block counts as one step and one failure at most, following normal step failure semantics. Put `if.fail` or `next` on the parent; a retry reruns the entire block.
- A child timeout affects only that child. A parent timeout cancels unfinished children and waits for them to stop before taking the failure path.
- Parallel blocks also work inside `groups`, `after-pr`, and called workflows.
  Children must be prompt/command steps; child `next`, `if`, `option`,
  `plan`, `group`, `workflow_call`, nested `parallel`, `flag`, and
  `allow_commit: true` are rejected. Configure flow control on the parent (including `flag`) and
  perform commits in a subsequent sequential step.

<a id="step-timeout"></a>
### Step Timeout

Prompt, command, and parallel steps may set `timeout:` to abort the step if it runs too long (option steps ignore it; in a command array each command gets the timeout; on a parallel block it limits the whole block). Accepted formats:

| Suffix | Meaning | Example |
|--------|---------|---------|
| (none) | Seconds | `timeout: "30"` |
| `m` | Minutes | `timeout: 5m` |
| `h` | Hours | `timeout: 1h` |

When a timeout fires:

- **Command steps**: the child process is killed and the step is treated as a failure (non-zero exit). `{prev.success}` is `false` and the workflow follows the normal failure path (see `if.fail` below).
- **Prompt steps**: the LLM call is aborted and the step is treated as a failure.

Invalid timeout strings are rejected at config validation time. Timeouts are also honoured for steps defined inside groups and `after-pr`.

<a id="option-step-interactive-selection"></a>
### Option Step (interactive selection)

Each item in `option` is either a `selector` (menu choice) or a `text-input` (free-text prompt). The optional `plan` field resolves to a file path whose contents are displayed in a bordered panel before the menu is shown:

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
## Post-PR Automation (`after-pr`)

For host-created commit and PR body formatting, see [Commit and PR Templates](configuration.md#commit-and-pr-templates).

Use `after-pr` for steps that should run automatically after `cruise run` successfully creates a pull request. `after-pr` uses the same step format as `steps`, so you can define inline or file-backed prompt steps (`prompt` / `prompt_file`), command steps, and grouped steps there as well.

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

`after-pr` steps run only after PR creation succeeds. They can use all normal template variables plus the PR-specific variables listed below.

Prompt steps in `after-pr` are guarded by the same default. Set `allow_commit: true` individually for an after-PR prompt that needs to advance `HEAD`.

<a id="review-loop-github-review"></a>
### Review loop (`github-review`)

An explicit `after-pr` step can handle review bots such as CodeRabbit. The general-purpose built-ins do not add it; `builtin:pr-review-fix` uses it for an existing PR.

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

In bot mode, each iteration waits for one of the listed bots to review the current head commit, then fetches all unresolved threads started by that bot (across all pages). In all-reviewer mode, it fetches eligible unresolved threads immediately without waiting for a new review.

Both modes run the prompt with thread data appended, and the prompt must return JSON only: `{"actions":[{"thread_id":"...","reply":"optional","resolve":true}]}`. Cruise posts replies and resolves threads through `gh api graphql` (requires an authenticated `gh`). Commits made by the prompt are pushed; if uncommitted changes remain, the step fails. A `github-review` failure marks the run `Failed`. This step is valid only directly under `after-pr`.

The default `reviewers: bots` preserves this behavior and requires a nonempty, unique `bots` list. To process existing human and bot feedback, use `github-review: { reviewers: all, max-iterations: 3 }` and omit `bots` or specify an empty list. This mode does not wait for a new review. Cruise checks each thread author's repository collaborator permission before passing thread text to the agent; only authors with `write`, `maintain`, or `admin` are eligible. The loop succeeds when no eligible unresolved threads remain and fails if eligible threads remain at the iteration limit.

<a id="variable-reference"></a>
## Variable Reference

Commit and PR templates use a separate [allowed vocabulary](configuration.md#commit-and-pr-templates), which does not include `{plan}`. The `{plan}` below remains a session plan file path.

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from CLI argument or stdin; for `--skip-planning` / input-as-plan sessions (`input_as_plan: true`) it resolves to the current `plan.md` content |
| `{prev.output}` | Previous prompt output, or a parallel block's JSON results keyed by child name |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Previous command/prompt stderr, or a parallel block's combined stderr and execution errors prefixed by child name |
| `{prev.success}` | Previous command success, or parallel block success (`true`/`false`; skipped children count as successful) |
| `{plan}` | Session plan file path (set automatically by `cruise run`) |
| `{file:name}` | UTF-8 contents of a relative session artifact named `name`; prompt fields only, loaded immediately before execution |
| `{plan.language}` | Effective language used for built-in planning prompts (from `CRUISE_LANGUAGE_PLAN`, `languages.plan`, the legacy field, locale inference, or the default) |
| `{pr.number}` | Pull request number, available after a PR has been created |
| `{pr.url}` | Pull request URL, available after a PR has been created |
| `{pr.language}` | Effective language used for PR title/body generation (from `CRUISE_LANGUAGE_PR`, `languages.pr`, the legacy field, locale inference, or the default) |

> **Note:** `{model}` is **not** a template variable -- it is a special placeholder resolved only within the top-level `command` array. It is not available inside `prompt`, `prompt_file`, `instruction`, or `command` step fields.

Literal braces are escaped Rust-`format!`-style: `{{` -> `{` and `}}` -> `}` (e.g. `"{{input}}"` resolves to the literal string `"{input}"`, not a lookup of `input`). An unclosed `{`, a lone `}`, or an empty `{}` is a template syntax error, as is referencing an undefined variable.
