# The three step types

Each step primarily holds one of `prompt` (LLM call), `command` (shell execution), or `option` (interactive selection). A step that only holds `group:` (a group call) is the exception — see [groups.md](groups.md).

## Prompt step (LLM call)

```yaml
steps:
  planning:
    model: claude-opus-4-5     # Optional: per-step model override
    instruction: |             # Optional: system prompt
      You are a senior engineer.
    prompt: |                  # Required: prompt body
      Create an implementation plan for: {input}
    env:                       # Optional: per-step environment variables
      ANTHROPIC_MODEL: claude-opus-4-5
```

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
      - selector: Cancel
        next: ~                          # null next = end of workflow
```

- `selector`: label shown in the menu; selecting it transitions to `next`.
- `text-input`: label shown as a text prompt; the user's input becomes `{prev.input}` in the next step.
- `next: ~` (null): selecting that option ends the workflow.

## Common field reference

| Field | Type | Description |
|-------|------|-------------|
| `model` | string | Model for prompt steps (overrides top-level) |
| `prompt` | string | Prompt body (prompt steps) |
| `instruction` | string | System prompt (prompt steps) |
| `plan` | string | Path of a file displayed before an option step menu |
| `option` | array | Choices for option steps |
| `command` | string \| array | Shell command(s) |
| `next` | string | Explicit next step name |
| `skip` | bool \| string | Skip condition (see [flow-control.md](flow-control.md)) |
| `if` | object | Conditional execution (see [flow-control.md](flow-control.md)) |
| `env` | object | Per-step environment variables |
| `group` | string | Group invocation (see [groups.md](groups.md)) |
| `fail-if-no-file-changes` | bool | Legacy: fail when no files changed (see [flow-control.md](flow-control.md)) |
