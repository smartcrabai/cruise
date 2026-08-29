# Top-level structure

```yaml
command:                  # LLM invocation command (array). Mutually exclusive with `sdk`.
  - claude
  - --model
  - "{model}"
  - -p

# sdk: jcode              # Alternative backend: drive the jcode CLI (this is the
                          # default when neither `command` nor `sdk` is set; see sdk.md).
# sdk: claude             # Alternative backend: drive the claude CLI in-process
                          # via claude-agent-sdk (see sdk.md).

description: My workflow  # Optional: shown alongside the file name in config selectors

model: sonnet             # Optional: default model for prompt steps
                          # (in SDK mode, a "provider/model[:effort]" reference)
plan_model: opus          # Optional: model for the built-in plan step
                          # (in SDK mode, a "provider/model[:effort]" reference)
max_retries: 4           # Optional: global DAG edge traversal ceiling (default: 3)
interactive_planning: true # Optional: enable SDK plan tools (default: true)
languages:                # Optional: prompt languages; defaults to English
  pr: English             # Language for auto-generated PR title/body
  plan: English           # Language for built-in planning prompts

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
force_exec: false         # Optional: execute direct plan entry points in place (default: false)

```
`steps` is required. Setting both `command` and `sdk` is a validation error (an empty `command` array counts as "not set"); setting neither runs prompts on the default `jcode` backend. When `sdk` is set it must be `jcode` or `claude` — any other value is a validation error. `steps` is held as an `IndexMap`, so declaration order is the execution order. When a group's `if.file-changed` target is outside the group, its `max_retries` requires one additional global loop-protection budget unit; the group example above uses `max_retries: 3` with a top-level `max_retries: 4`.

## `command` vs `sdk`

There are three prompt-execution backends:

- `command:` — spawn an external CLI (e.g. `claude -p`) and write the prompt to its stdin.
- `sdk: jcode` — drive the `jcode` CLI as a subprocess, against cruise's own jcode home (sign in with `cruise login`). **Default** when neither `command` nor `sdk` is set. `model` / `plan_model` / per-step `model` are plain **model references** (`"provider/model[:effort]"` or a bare `"model"`). See [sdk.md](sdk.md) for details.
- `sdk: claude` — drive the `claude` CLI in-process via claude-agent-sdk. Model references are plain `claude --model` names with an optional `:effort` suffix; authentication is the claude CLI's own. See [sdk.md](sdk.md) for details.

## `command` and the `{model}` placeholder

`{model}` inside the `command` array is a special placeholder resolved at runtime. It is **not** a template variable and cannot be used inside `prompt` / `prompt_file` / `instruction` / `command` step fields.

- When an effective model is set: `{model}` is replaced with the model name.
- When no model is set: both `{model}` and its immediately preceding `--model` flag are removed automatically.
- When the `command` array contains **no** `{model}` placeholder and an effective model is set: `--model <model>` is appended to the command arguments automatically.
- Rust-`format!`-style brace escaping applies here too: `{{model}}` is the literal string `{model}`, not the placeholder. Any other unescaped `{name}`, an empty `{}`, an unclosed `{`, or a lone `}` is a template syntax error.

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

Model used by the built-in plan step (driven by `cruise plan`). Falls back to `model` if unset. In SDK mode it is a plain model reference (see [sdk.md](sdk.md)).

## `description`

Free-form text shown alongside the file name in the CLI/GUI config selectors. Purely informational; no effect on execution.

```yaml
description: Full TDD flow with review loop
```

## `languages`

`languages.pr` controls the language used for the auto-generated PR title and body, and `languages.plan` controls the language used for built-in planning prompts. The deprecated top-level `pr_language` and `plan_language` fields remain supported.

`CRUISE_LANGUAGE_PR` and `CRUISE_LANGUAGE_PLAN`, when set, override the corresponding YAML values. Blank values are ignored. Without an environment override, the nested field takes precedence over its deprecated top-level counterpart, then the first supported locale from `LC_ALL`, `LC_MESSAGES`, `LANG`, or `LANGUAGE`, then the default is `English`. Unsupported or language-neutral locales use `English`.

```yaml
languages:
  pr: Japanese           # PR title/body generated in Japanese
  plan: Japanese         # plans and plan answers generated in Japanese
```

For compatibility with older configs:

```yaml
# Deprecated; use languages.pr instead.
pr_language: Japanese
# Deprecated; use languages.plan instead.
plan_language: Japanese
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

## `force_exec`

When `true`, direct plan entry points use the same current-directory execution
path as `cruise exec`: no planning, worktree, or PR. It applies to
`cruise "<input>"`, `cruise plan "<input>"`, and `cruise --plan "<input>"`.

```yaml
force_exec: true
```

Use `--no-force-exec`, `--repo`, `--grill`, or image attachments to keep normal
planning. `--skip-planning` and `--no-interactive-planning` do not disable direct
execution. Background `--plan` runs foreground because no plan worker is needed.

## Rate-limit retry

When an HTTP 429 is detected, cruise retries the same model with exponential backoff:

- Initial delay: 2 seconds
- Max delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)

The SDK backends additionally accept an optional `retry:` block. Declaring it widens rate-limit handling into a fallback policy: 5xx and network failures become retryable too, the backoff switches to `base_delay_ms` doubling to an 8s ceiling, and a model that has spent its retry budget is swapped for the next entry of its fallback chain. Setting `model_fallback: false` (or leaving the chains empty) only turns the *switching* off — the wider classification and the new backoff schedule still apply, so omit the block entirely to keep the historical behavior.

```yaml
retry:                    # Optional; SDK backends only. Omitted = same-model 429 retries only
  base_delay_ms: 500      # Backoff base (default 500); delay is min(base * 2^(attempt-1), 8s) with jitter
  max_delay_ms: 300000    # Waiting cap (default 300000). The computed backoff is already capped at 8s,
                          # so this only binds a server Retry-After hint (itself clamped to 60s): a
                          # hinted delay above it moves to the next fallback model, or fails the step
                          # when there is no chain entry left
  model_fallback: true    # Allow switching models via fallback_chains (default true)
  fallback_chains:        # Keys: "provider/model", "provider/*", a bare "model", or "default"
                          # (most specific wins)
    default:
      - anthropic-api/claude-opus-4-6
      - openai-api/gpt-5.5
```

Chain entries are `"provider/model"`, `"provider/*"` (keeps the failing model id, swaps only the provider), or a bare model name. A switched-to model gets a fresh retry budget and no delay; every retry starts a fresh session; a turn that already streamed visible text is never retried on another model; a model that just failed is skipped for 5 minutes (in-memory, and cleared when the config is hot-reloaded). The attempt budget stays `--rate-limit-retries` — `retry:` adds no second count. Top-level `max_retries` is unrelated: it is the DAG loop-protection ceiling, not a retry budget for prompts.
