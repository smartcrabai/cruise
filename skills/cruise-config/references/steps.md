# Step types and file-backed prompts

Each step primarily holds one of `prompt` (an inline LLM prompt, or a prompt loaded
from `prompt_file`), `command` (shell execution), or `option` (interactive
selection). A step that only holds `group:` (a group call) is the exception — see
[groups.md](groups.md).

## Prompt step (LLM call)

```yaml
steps:
  planning:
    model: claude-opus-4-5     # Optional: per-step model override (a "provider/model[:effort]" reference in SDK mode)
    instruction: |             # Optional: message shown to the user before the step runs
      Describe the feature you want to build.
    prompt: |                  # Use either prompt or prompt_file
      Create an implementation plan for: {input}
    timeout: 10m               # Optional: per-step timeout ("30" = seconds, "5m", "1h")
    env:                       # Optional: per-step environment variables
      ANTHROPIC_MODEL: claude-opus-4-5
```

For a longer prompt, load the body from a file:

```yaml
steps:
  implement:
    prompt_file: prompts/implement.md
```

`prompt_file` accepts an absolute path, a `~/` path, a path relative to the
configuration file, or a supported GitHub blob/raw URL. A bare file name means
the same directory as the configuration file. GitHub URLs are fetched via
`gh api` at config-load time. In a called workflow, relative paths use the
called workflow's directory. File contents are preserved exactly
and receive the same variable expansion as `prompt`. `prompt` and `prompt_file`
are mutually exclusive.

Resolution rules:

| Path form | Resolution |
| --- | --- |
| Absolute path | Used as-is |
| `~/...`, `~`, or `~user/...` | Expanded from the current user's or named user's home directory |
| `./...`, `../...`, or a bare file name | Relative to the config file's directory |
| GitHub blob/raw URL | Fetched from GitHub |
| Relative path in a called workflow | Relative to the called workflow's directory |

Absolute and `~` paths refer to the local filesystem. In a GitHub-hosted
workflow, relative non-URL values are resolved as paths in the remote directory; local
`~` paths are rejected. A direct GitHub blob/raw URL can be used explicitly.

### `instruction` is not a system prompt

`instruction` is a message **displayed to the user** (after variable resolution) just before the step runs. It is never sent to the LLM. It additionally doubles as an interactive input prompt: when `{input}` is currently empty, the resolved instruction text is shown as a multiline input prompt and the user's entry becomes `{input}`.

## Command step (shell execution)

`command:` may be a single string or an array. Arrays are run sequentially and stop on the first failure.

```yaml
steps:
  run_tests:
    command: cargo test        # single string

  lint_and_test:
    command:                   # array: run sequentially, stop on first failure
      - cargo fmt --all
      - cargo clippy -- -D warnings
      - cargo test
```

The next step can read this step's stderr and exit status via `{prev.stderr}` and `{prev.success}`.

## Option step (interactive selection)

Each option item is either a `selector` (menu entry) or a `text-input` (free-text prompt).

When `plan:` is set, the file's contents are displayed in a bordered panel before the menu.

```yaml
steps:
  review_plan:
    plan: "{plan}"                       # Optional: path of a file shown before the menu
    option:
      - selector: Approve and continue
        next: implement
      - selector: Revise the plan
        next: planning
      - text-input: Other (free text)    # shows a text input prompt on selection
        next: planning                   # the entered text becomes {prev.input}
      - selector: Continue
        next: ~                          # null next = fall through to declaration order
```

- `selector`: label shown in the menu; selecting it transitions to `next`.
- `text-input`: label shown as a text prompt; the user's input becomes `{prev.input}` in the next step.
- `next: ~` (null): falls through to the next step in YAML declaration order. The workflow ends only when the option step is the last one.

## Common field reference

| Field | Type | Description |
|-------|------|-------------|
| `model` | string | Model for prompt steps (overrides top-level; a model reference in SDK mode) |
| `prompt` | string | Inline prompt body (use with prompt steps) |
| `prompt_file` | string \| null | File or supported GitHub blob/raw URL whose contents become the prompt; absolute, `~/`, or config-file-relative path (`~`/null means the local home directory) |
| `instruction` | string | Message shown to the user before the step; input prompt when `{input}` is empty (prompt steps) |
| `plan` | string | Path of a file displayed before an option step menu |
| `option` | array | Choices for option steps |
| `command` | string \| array | Shell command(s) |
| `next` | string | Explicit next step name |
| `skip` | bool \| string | Skip condition (see [flow-control.md](flow-control.md)) |
| `when` | object | Pre-execution condition: `exists: <glob>` (see [flow-control.md](flow-control.md)) |
| `if` | object | Conditional execution: `file-changed` / `no-file-changes` / `fail` (see [flow-control.md](flow-control.md)) |
| `timeout` | string | Per-step timeout: `"30"` = seconds, `"5m"` = minutes, `"1h"` = hours (see [flow-control.md](flow-control.md)) |
| `env` | object | Per-step environment variables |
| `group` | string | Group invocation (see [groups.md](groups.md)) |
| `workflow_call` | string | Workflow file or supported GitHub URL to inline |
| `fail-if-no-file-changes` | bool | Legacy: fail when no files changed (see [flow-control.md](flow-control.md)) |
