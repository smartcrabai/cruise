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
                          # In SDK mode, [primary, fallback, ...] is an implicit chain
plan_model: opus          # Optional: model for the built-in plan step
                          # (in SDK mode, a "provider/model[:effort]" reference)
                          # In SDK mode, [primary, fallback, ...] is an implicit chain
max_retries: 4           # Optional: global graph edge traversal ceiling (default: 3)
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

Unknown top-level keys are silently ignored, so obsolete fields from older
configs (for example `worktree:` or `state:`) still load without error. Only
per-step configuration and the `retry:` block reject unknown fields, so a typo
at the top level is accepted and has no effect, while the same typo inside a
step fails the load (see [steps.md](steps.md)).

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

A step-level `model:` overrides the top-level `model:` for that step only. In
SDK mode, workflow-level model arrays use the first entry as the primary and
the remaining entries as fallbacks. Arrays with fallback entries automatically
enable model fallback; an explicit `retry.fallback_chains` entry for a primary
model wins over its array tail. Rate limits retry the current model while its
retry budget and delay permit, then switch to the next usable fallback when
one exists. 5xx, HTTP 4xx other than 429, 401, 403, 407, and 408, and
network failures switch immediately to a usable fallback when
`--rate-limit-retries` is above zero and no visible text was streamed; such a
4xx is never resent to the same model, so it switches or fails as-is. 401,
403, and 407 are auth or permission failures no other model can answer and
fail the step at once; a 408 counts as a network failure, switching first like
a 5xx and resent to the same model only when no usable fallback is left,
visible text was already streamed, or `--rate-limit-retries` is `0`. Models
skipped after a 429, a 5xx, or a network failure including a 408 are cooled
down for 30 minutes in the current process, and so is one left behind because
the provider's text named the model itself as absent (`model_not_supported`,
`model_not_found`, `unknown model`): there the model is the defect, so later
turns and later steps stop selecting it. Only a refused request -- a plain
client error -- keeps its model immediately selectable, since such a status says
nothing about the model's health. In command mode, only the first array entry
is used and the historical same-model retry behavior remains unchanged.

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

Model used by the built-in plan step (driven by `cruise plan`). Falls back to `model` if unset. In SDK mode it is a plain model reference (see [sdk.md](sdk.md)), or an array whose first entry is primary and remaining entries are fallbacks.

## `description`

Free-form text shown alongside the file name in the CLI/WebUI config selectors. Purely informational; no effect on execution.

```yaml
description: Full TDD flow with review loop
```

## `languages`

`languages.pr` controls the language used for the auto-generated PR title and body, and `languages.plan` controls the language used for built-in planning prompts. The deprecated top-level `pr_language` and `plan_language` fields remain supported.

`CRUISE_LANGUAGE_PR` and `CRUISE_LANGUAGE_PLAN`, when set, override the corresponding YAML values. Blank values are ignored. Without an environment override, the nested field takes precedence over its deprecated top-level counterpart, then the first non-empty variable among `LC_ALL`, `LC_MESSAGES`, `LANG`, and `LANGUAGE` is mapped once; if that value is not a supported locale, no other variable is consulted and the default is `English`. Unsupported or language-neutral locales use `English`.

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
- For `--repo` sessions, post-PR clone/worktree cleanup errors are silently ignored and the removed-clone notice is printed unconditionally; only non-repo worktree cleanup errors are downgraded to warnings. The session is still marked `Completed`.
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
planning. On the foreground `cruise plan` entry point, `--formal-spec` also keeps
normal planning and adds formal specifications. `--skip-planning` and
`--no-interactive-planning` do not disable direct execution. Background `--plan`
runs foreground because no plan worker is needed.

## Rate-limit retry

Without a retry policy, cruise retries the same model with exponential backoff while its retry budget and delay permit. Command mode retries on HTTP 429 / rate-limit text in stderr; SDK backends retry any provider failure classified as a limit — HTTP 429, `rate limit` / `too many requests` text, `usage limit`, `session limit`, or `overloaded`. Both use the same schedule:

- Initial delay: 2 seconds
- Max delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)

The `retry:` block is accepted by configuration validation for any backend but affects retry behavior only for SDK backends. In SDK mode, declaring it,
or using a workflow-level model array with fallback entries, widens rate-limit
handling into a fallback policy: 5xx, HTTP 4xx other than 429, 401, 403, 407,
and 408, and network failures become retryable too,
the backoff switches to `base_delay_ms` doubling to an 8s ceiling, and those
failures switch immediately to the next usable fallback when
`--rate-limit-retries` is above zero and no visible text was streamed. A 429
retries the current model while its retry budget and delay permit, then
switches to the next usable fallback when one exists. A retryable 4xx only
ever switches -- never a same-model retry -- and surfaces the original error
when no usable fallback is left. What it implies about the model decides the
cooldown: a refused request skips its model without cooling it, since a client
error describes the request rather than the model's health, while text naming
the model as absent (`model_not_supported`, `model_not_found`, `model not
found`, `model not supported`, `model is not supported`, `unknown model`, `no
such model`, `model does not exist`, `not a valid model`) is classified as a
missing model and cools the skipped model for the same 30 minutes, so later
turns and later steps stop choosing a model the provider does not have. Only
the provider's own error text is read for this, not the child-process stderr
tail appended after it. A 400 still switches when its message reads permanent
(`invalid request`, `context length`, `max_tokens`); `invalid_request_error:
model not found` is a missing model instead of a permanent failure, because
the reference rather than the request is at fault. A failure naming no status
code (`unknown option '--effort'`) stays permanent and fails at once. A
number is read as a status only when it is a standalone three-digit number
with one of `http`, `status`, `code`, `error`, `returned`, or `upstream`
within the preceding 24 bytes; source locations (`src/lib.rs:404:17`) and URL
ports (`https://host:443/path`) never are. Rate-limit wording is classified
first and 5xx second, so a 429 or a 503 keeps its class even when appended
stderr carries a 4xx-looking number; the missing-model wordings are read
after the rate-limit, 408, 5xx, and network markers and before the bare-4xx
client-error fallthrough. 401, 403, and 407 fail the step at once --
authentication and permission failures no other model can answer. A 408 is
classified as a network failure: immediate switch like a 5xx, resent to the
same model only when no usable fallback is left, visible text was already
streamed, or `--rate-limit-retries` is `0`, and it cools down its skipped
model. Setting
`model_fallback: false` (or leaving the chains empty) only turns the *switching*
off for scalar model configurations; model arrays with fallback entries always
enable switching. The
wider classification and the new backoff schedule still apply, so omit the
block entirely and use scalar or unset model fields to keep the historical
behavior.

```yaml
retry:                    # Optional; accepted by validation for any backend, honored only by SDK backends
                          # Command mode ignores this block and keeps the historical rate-limit retry loop
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

Chain entries are `"provider/model"`, `"provider/*"` (keeps the failing model id, swaps only the provider), or a bare model name. A switched-to model gets a fresh retry budget and no delay; every retry starts a fresh session; a turn that already streamed visible text is never retried on another model; a model skipped because of a 429, a 5xx, a network failure, or an error naming it as missing remains skipped for 30 minutes in this process (in-memory, not persisted across processes), while a client-error switch keeps its model immediately selectable. The attempt budget stays `--rate-limit-retries` — `retry:` adds no second count. Top-level `max_retries` is unrelated: it is the graph loop-protection ceiling, not a retry budget for prompts.
