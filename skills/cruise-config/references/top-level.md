# Top-level structure

```yaml
command:                  # LLM invocation command (array). Mutually exclusive with `sdk`.
  - claude
  - --model
  - "{model}"
  - -p

# sdk: seher              # Alternative backend: run prompts via seher's resolved
                          # provider instead of an external command (see sdk.md).
# sdk: pi                 # Alternative backend: run prompts via pi_agent_rust
                          # directly, no seher config needed (see sdk.md).

description: My workflow  # Optional: shown alongside the file name in config selectors

model: sonnet             # Optional: default model for prompt steps
                          # (in SDK mode, reinterpreted as a seher mode_key)
plan_model: opus          # Optional: model for the built-in plan step
pr_language: English      # Optional: language for auto-generated PR title/body (default: English)

llm:                      # Optional: OpenAI-compatible API for session-title generation
  api_key: sk-...
  endpoint: https://api.openai.com/v1
  model: gpt-4o-mini

env:                      # Optional: environment variables applied to every step
  API_KEY: sk-...

groups:                   # Optional: step group definitions (see references/groups.md)
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify

steps:                    # Required: workflow steps (declaration order = execution order)
  step_name:
    # ...

after-pr:                 # Optional: steps that run after PR creation (see references/after-pr.md)
  step_name:
    # ...

cleanup_after_pr: false   # Optional: delete local worktree and branch after PR creation (default: false)
```

`steps` and exactly one of `command` / `sdk` are required. Setting both `command` and `sdk`, or neither, is a validation error (an empty `command` array counts as "not set"). When `sdk` is set it must be `seher` or `pi` — any other value is a validation error. `steps` is held as an `IndexMap`, so declaration order is the execution order.

## `command` vs `sdk`

There are three prompt-execution backends:

- `command:` — spawn an external CLI (e.g. `claude -p`) and write the prompt to its stdin.
- `sdk: seher` — run prompts in-process, resolving a provider/model through seher's own `~/.config/seher/config.yaml`. `model` / `plan_model` / per-step `model` are reinterpreted as seher **mode keys**. See [sdk.md](sdk.md) for details.
- `sdk: pi` — run prompts in-process via `pi_agent_rust` **directly**, bypassing seher's provider resolution and config file entirely. `model` / `plan_model` / per-step `model` are plain **model references** (`"provider/model[:thinking]"` or a bare `"model"`), not mode keys. See [sdk.md](sdk.md) for details.

## `command` and the `{model}` placeholder

`{model}` inside the `command` array is a special placeholder resolved at runtime. It is **not** a template variable and cannot be used inside `prompt` / `instruction` / `command` step fields.

- When an effective model is set: `{model}` is replaced with the model name.
- When no model is set: both `{model}` and its immediately preceding `--model` flag are removed automatically.
- When the `command` array contains **no** `{model}` placeholder and an effective model is set: `--model <model>` is appended to the command arguments automatically.

The prompt body is passed to the spawned process via **stdin** (avoids ARG_MAX limits), not as an argument.

A step-level `model:` overrides the top-level `model:` for that step only.

```yaml
command:
  - claude
  - --model
  - "{model}"      # resolved at runtime; `--model {model}` is stripped if no model is set
  - -p

model: sonnet      # default

steps:
  planning:
    model: opus    # this step uses opus
    prompt: "Plan: {input}"
```

## `plan_model`

Model used by the built-in plan step (driven by `cruise plan`). Falls back to `model` if unset. Under `sdk: seher` it is reinterpreted as the planning mode key; under `sdk: pi` it is a plain model reference (see [sdk.md](sdk.md)).

## `description`

Free-form text shown alongside the file name in the CLI/GUI config selectors. Purely informational; no effect on execution.

```yaml
description: Full TDD flow with review loop
```

## `pr_language`

Language used for the auto-generated PR title and body. Defaults to `English`.

```yaml
pr_language: Japanese     # PR title/body generated in Japanese
```

## Hot-reload

During `cruise run`, the config file's mtime is checked between steps and the file is reloaded automatically when changed.

- Does not apply to sessions started from the built-in default.
- The current step must still exist in the new config.

## `cleanup_after_pr`

When set to `true`, cruise deletes the local git worktree and its branch after the PR has been created successfully.

```yaml
cleanup_after_pr: true   # remove worktree + branch once the PR is open
```

- Has no effect in **current-branch mode** (no worktree exists to remove).
- Has no effect for **`--repo` sessions** (the clone is always removed after PR creation regardless of this flag).
- Errors during cleanup are downgraded to warnings; the session is still marked `Completed`.
- Override per-run with `--cleanup-after-pr` / `--no-cleanup-after-pr` CLI flags (takes precedence over config and session-level setting).

See [after-pr.md](after-pr.md) for steps that run after PR creation.

## Rate-limit retry

When an HTTP 429 is detected, cruise retries with exponential backoff:

- Initial delay: 2 seconds
- Max delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)
