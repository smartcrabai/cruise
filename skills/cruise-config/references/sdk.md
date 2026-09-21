# SDK backends: `sdk: jcode` / `sdk: claude`

Setting the top-level `sdk` field selects an SDK execution path instead of the classic external `command` backend. Two values are accepted: `"jcode"` and `"claude"`. Any other value is a validation error.

```yaml
sdk: jcode

model: anthropic-api/claude-sonnet-4-6   # "provider/model[:effort]" for prompt steps
plan_model: openai-api/gpt-5.5:high      # model for the built-in plan step (falls back to `model`)
# model: [anthropic-api/claude-sonnet-4-6, openai-api/gpt-5.5]

steps:
  implement:
    prompt: "{input}"
```

## Mutual exclusivity with `command`

- Both `command` and `sdk` set → validation error.
- Neither set → valid: prompts run on the **default `jcode` backend**. An empty `command` array counts as "not set", so an `sdk`-only config is valid.
- `sdk` set to anything other than `jcode` / `claude` → validation error.

## Model fields are plain model references

In both SDK backends, `model` / `plan_model` / per-step `model` carry the same precedence as command mode (step `model` > top-level `model` / `plan_model`). Workflow-level `model` and `plan_model` may be arrays: the first entry is primary and the remaining entries are an implicit fallback chain. Arrays with fallback entries enable model fallback automatically; explicit `retry.fallback_chains` entries for a primary model take precedence. A 429 retries the current model while its retry budget and delay permit, while a 5xx, an HTTP 4xx other than 429, 401, 403, 407, and 408, or a network failure switches immediately to a usable fallback when `--rate-limit-retries` is above zero and no visible text was streamed. Such a 4xx is never resent to the same model — an identical request would fail identically — so it either switches or surfaces as-is; 401, 403, and 407 are auth or permission failures another model cannot fix and fail the step at once, and a 408 is treated as a network failure, so it switches to a usable fallback first exactly like a 5xx and is resent to the same model only when no usable fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0`. Models skipped after a 429, a 5xx, or a network failure (a 408 included) are cooled down for 30 minutes in the current process, and so are models the provider named as missing (`400 model_not_supported`, `404 model_not_found`); only a model left behind by a plain client-error switch is not. Each scalar value is a model reference:

| Form | Example | Behavior |
|------|---------|----------|
| `provider/model[:effort]` | `openai-api/gpt-5.5:xhigh` | Explicit provider + model **under `sdk: jcode` only**. `sdk: claude` does not split on `/`: it strips the `:effort` suffix and passes the rest to `claude --model` verbatim, so `provider/model` reaches the CLI as a single (usually unknown) model id. |
| `:effort` | `:high` | Provider/model left to the backend default; the requested effort is applied. |
| `model` (no `/`) | `claude-sonnet-4-6` | Provider left to the backend's own resolution. |
| unset | *(both `model` and `plan_model` omitted)* | The backend's configured default provider/model is used. |

The `:effort` suffix is recognized only when it names an effort tier (`low`/`medium`/`high`/`xhigh`/`max`, the aliases `minimal`/`min`/`med`, or the numeric spellings `1`..`4`) or one of `off`/`none`/`0`/`5` — those four are stripped from the model id but leave the effort unset. Any other `:` suffix (e.g. an OpenRouter `:free` variant) stays part of the model id, so watch out for a model id whose own suffix happens to be `:0`..`:5`.

A `/` with an empty side (`"/model"`, `"provider/"`) is rejected by `sdk: jcode` when the prompt runs — a step error, not a config-validation error, so `cruise plan --dry-run` does not catch it. `sdk: claude` forwards it to the CLI, which fails with its own message.

## `sdk: jcode` — the jcode CLI (default)

`sdk: jcode` drives the [jcode](https://github.com/1jehuang/jcode) CLI as a subprocess: one prompt is one `jcode run --ndjson` child. jcode **v0.82.0 or newer** is required — an older binary is rejected with a clear error, because cruise relies on both the NDJSON event shape and jcode's upstream per-server MCP `timeout_secs` setting. The provider part of a model reference is a jcode provider id — one of the values `jcode login --help` lists (`jcode provider list` prints only a curated subset and omits API-key providers such as `anthropic-api`); `jcode auth status` shows which ones you are signed in to. The effort suffix is forwarded through jcode's reasoning-effort environment overrides and ignored by providers/models without reasoning effort.

OpenAI priority processing is off by default (cruise sets `JCODE_OPENAI_SERVICE_TIER=off`); opt in with `env.JCODE_OPENAI_SERVICE_TIER: "priority"` — see [the configuration example](env-and-llm.md#openai-priority-processing-with-jcode).

### Authentication and the shared jcode home

Credentials, sessions, `config.toml`, and MCP registration live in **jcode's own home** — `$JCODE_HOME` when that variable is set and non-empty, otherwise `~/.jcode` — the same home an interactive `jcode` session uses. Cruise has no home of its own and never injects `JCODE_HOME` into the jcode child; exporting it before starting cruise relocates everything at once, which is how the GitHub Action keeps credentials off a runner's real home.

- `jcode login <provider>` — sign in (interactive picker / OAuth flow, or the provider's API key).
- `jcode auth status` — list the authenticated providers; `--json` for machine-readable output.
- `jcode model list` — list the models those providers expose.

Running `sdk: jcode` with no authenticated provider fails with an error pointing at `jcode login`. Custom OpenAI-compatible endpoints are added as jcode's own `[providers.<name>]` profiles (`jcode provider add`) in that home's `config.toml` — cruise adds no provider notation of its own.

### Custom tools via MCP

jcode cannot register custom tools in-process, so cruise's tools reach the model through a stdio MCP server (`cruise mcp-bridge`, registered in the home's `mcp.json`) and appear as `mcp__cruise__<tool>`. Cruise sets jcode's upstream `timeout_secs` to 86,400 seconds (24 hours) for that server, so interactive `ask_user` questions can wait up to 24 hours at the MCP layer, subject to any shorter workflow step timeout. Caveat: jcode also merges MCP configuration from the run directory (`.jcode/mcp.json`, `.mcp.json`, `.claude/mcp.json`), last-wins over the home. A project-local server named `cruise` is a **hard error** (it would shadow cruise's tools); servers under other names load but are reported with a warning.

## `sdk: claude` — the claude CLI in-process

`sdk: claude` drives the `claude` CLI in-process through claude-agent-sdk, with cruise's tools exposed as `mcp__cruise__<tool>`. Model references are plain `claude --model` names with the optional `:effort` suffix (forwarded as `--effort`; a `claude` CLI without that flag fails the step with `unknown option '--effort'`, which is classified permanent and never retried — cruise is verified against 2.1.250). Authentication is the claude CLI's own — its stored credentials or `ANTHROPIC_API_KEY` — unrelated to `jcode login`. The CLI runs with permissions bypassed: cruise workflows are unattended, so there is no console to answer a permission prompt on.

## Differences from command mode

- **`env` applies to prompt steps**: top-level and per-step `env:` values are placed in the environment of the backend's child process (the `jcode` / `claude` CLI).
- **`{model}` placeholder is irrelevant**: it only exists for the `command` array.
- **Interactive planning**: during `cruise plan`, the SDK agent gets custom planning tools — `ask_user` (ask the user a clarifying question), `submit_plan` (write the plan markdown), and `update_plan` (find/replace a section of the existing plan). Both SDK backends support them. In non-interactive runs (no TTY), `ask_user` is not registered — the prompt instead tells the agent to decide on explicitly stated assumptions — but `submit_plan` and `update_plan` remain available, and a turn that ends without a successful `submit_plan`/`update_plan` call fails instead of falling back to the agent's final message as the plan. The interview-style `cruise plan --grill` mode builds on `ask_user`. Set `interactive_planning: false` to disable all of this and have the agent write `plan.md` directly, exactly like the `command` backend.
- **Run steps execute autonomously**: ordinary prompt steps get no custom tools; the agent's built-in tools do the file editing. The one exception is `skip_step` (see below), which is registered only on steps that need it.
- **Session continuity**: planning's plan/fix/ask turns share one backend session only when an SDK backend is active and `interactive_planning` is `true`; with `interactive_planning: false`, each turn starts a fresh backend session.

## Commit guard

SDK prompt steps are protected from advancing Git `HEAD` by default. Set `allow_commit: true` on a prompt that is intentionally expected to create commits or otherwise move `HEAD`; omitted and `false` values keep the guard enabled and false values are omitted when configs are serialized. The `true` value bypasses all commit-guard behavior for that prompt. A guarded movement is reported as a commit-guard violation and fails the step, even if cruise restores the original branch reference without touching the index or worktree.

This guard covers both `sdk: jcode` and `sdk: claude` prompt execution. It does not apply to command or option steps, planning/title/PR-metadata calls, or cruise-owned PR worktree commits. `allow_commit: true` belongs on the actual prompt step, not a `group:` or `workflow_call:` invocation (those call sites reject the override).

### `skip_step` — declaring intentional no-changes (`if.no-file-changes` steps only)

A prompt step with an `if.no-file-changes` condition (`failed` or `retry`) additionally gets a `skip_step(reason)` tool: the agent calls it to declare that leaving the workspace unchanged this turn is the deliberate, correct outcome (for example, the plan explicitly says not to add tests), which disables that step's `if.no-file-changes` action for the current attempt. A plain-text alternative that needs no tool support at all — a `NO_CHANGES_INTENTIONAL: <reason>` line anchored at the start of a line in the step's output — has the same effect and works in `command:` mode too; see `flow-control.md` for both.

This tool is registered **only** on steps that carry `if.no-file-changes` — not on every run step — to keep the exposed tool set minimal on steps that can never call it.

`skip_step` only exists in SDK mode. In classic `command:` mode, `run_command` never sees `tools` at all, so there is no way for a `command:` step to call it — the `NO_CHANGES_INTENTIONAL:` output marker is the only option there.

Command and option steps behave identically in both modes.

## Rate limits and fallback

Both SDK backends use policy mode when the top-level `retry:` block is present (see `top-level.md`) or when a workflow-level model array with fallback entries creates an implicit policy. In policy mode, HTTP 5xx, HTTP 4xx other than 429, 401, 403, 407, and 408, and network errors are retryable and the backoff uses `base_delay_ms` doubling to an 8s ceiling. Such a 4xx never retries the same model: it switches to the next chain entry under the same conditions as a 5xx, and what the failure says about the model decides the cooldown. A refused request leaves the left-behind model uncooled, unlike a 429/5xx/network skip, because the status describes the request; text naming the model as absent (`model_not_supported`, `model_not_found`, `model not found`, `unknown model`, `no such model`, `model does not exist`, `not a valid model`, read from the provider's own error rather than the appended CLI stderr) is classified as a missing model instead and does cool the left-behind model for the same 30 minutes, so later turns and steps stop selecting a model the provider does not have. A 400 qualifies as a switch even when the message reads permanent (`invalid request`, `context length`, `max_tokens`), since another model may accept the request — and `invalid_request_error: model not found` is read as a missing model, not as a permanent failure; a 401, 403, or 407 fails the step immediately, because no other model can answer an auth or permission failure; a 408 is classified as a network failure, so it takes the 5xx/network path — switch to a usable fallback first, same-model resend only when no fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0` — and its skipped model is cooled down like any other network skip; a failure carrying no status code (`unknown option '--effort'`) stays permanent and fails the step at once. A number in the failure text counts as an HTTP status only when it is a standalone three-digit number with `http`, `status`, `code`, `error`, `returned`, or `upstream` within the preceding 24 bytes, and source locations (`src/lib.rs:404:17`) and URL ports (`https://host:443/path`) never qualify; rate-limit wording, 5xx, and the network markers are decided before the missing-model wordings, which in turn are decided before any bare 4xx-looking number, so appended stderr noise cannot downgrade a 429 or a 503. Without either policy, SDK backends retry any provider failure classified as a limit—HTTP 429, `rate limit` / `too many requests` text, `usage limit`, `session limit`, or `overloaded`—against the same model with 2-second-doubling backoff (capped at 60s). The attempt budget is `--rate-limit-retries` either way; `--rate-limit-retries 0` means no retry and therefore no model switch, except for a model reference the backend refuses outright (nothing was sent, so the next chain entry is tried immediately). Every retry starts a **fresh session** — resuming a partially-answered session would duplicate context — and a turn that already streamed visible text is never retried on another model. A model array with fallback entries always enables switching, even if `retry.model_fallback: false` is also present.
