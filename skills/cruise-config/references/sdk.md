# SDK backends: `sdk: jcode` / `sdk: claude`

Setting the top-level `sdk` field selects an SDK execution path instead of the classic external `command` backend. Two values are accepted: `"jcode"` and `"claude"`. Any other value is a validation error.

```yaml
sdk: jcode

model: anthropic-api/claude-sonnet-4-6   # "provider/model[:effort]" for prompt steps
plan_model: openai-api/gpt-5.5:high      # model for the built-in plan step (falls back to `model`)

steps:
  implement:
    prompt: "{input}"
```

## Mutual exclusivity with `command`

- Both `command` and `sdk` set → validation error.
- Neither set → valid: prompts run on the **default `jcode` backend**. An empty `command` array counts as "not set", so an `sdk`-only config is valid.
- `sdk` set to anything other than `jcode` / `claude` → validation error.

## Model fields are plain model references

In both SDK backends, `model` / `plan_model` / per-step `model` carry the same precedence as command mode (step `model` > top-level `model` / `plan_model`), and the value is a model reference:

| Form | Example | Behavior |
|------|---------|----------|
| `provider/model[:effort]` | `openai-api/gpt-5.5:xhigh` | Explicit provider + model **under `sdk: jcode` only**. `sdk: claude` does not split on `/`: it strips the `:effort` suffix and passes the rest to `claude --model` verbatim, so `provider/model` reaches the CLI as a single (usually unknown) model id. |
| `model` (no `/`) | `claude-sonnet-4-6` | Provider left to the backend's own resolution. |
| unset | *(both `model` and `plan_model` omitted)* | The backend's configured default provider/model is used. |

The `:effort` suffix is recognized only when it names an effort tier (`low`/`medium`/`high`/`xhigh`/`max`, the aliases `minimal`/`min`/`med`, or the numeric spellings `1`..`4`) or one of `off`/`none`/`0`/`5` — those four are stripped from the model id but leave the effort unset. Any other `:` suffix (e.g. an OpenRouter `:free` variant) stays part of the model id, so watch out for a model id whose own suffix happens to be `:0`..`:5`.

A `/` with an empty side (`"/model"`, `"provider/"`) is rejected by `sdk: jcode` when the prompt runs — a step error, not a config-validation error, so `cruise plan --dry-run` does not catch it. `sdk: claude` forwards it to the CLI, which fails with its own message.

## `sdk: jcode` — the jcode CLI (default)

`sdk: jcode` drives the [jcode](https://github.com/1jehuang/jcode) CLI as a subprocess: one prompt is one `jcode run --ndjson` child. jcode **v0.81.1 or newer** is required — an older binary is rejected with a clear error, because the NDJSON event shape is the whole contract. The provider part of a model reference is a jcode provider id — one of the values `jcode login --help` lists (`jcode provider list` prints only a curated subset and omits API-key providers such as `anthropic-api`); `cruise login --status` shows which ones cruise can already authenticate as. The effort suffix is forwarded through jcode's reasoning-effort environment overrides and ignored by providers/models without reasoning effort.

### Authentication and isolation

Credentials, sessions, `config.toml`, and MCP registration live in **cruise's own jcode home** (`$XDG_DATA_HOME/cruise/jcode-home`, default `~/.local/share/cruise/jcode-home`), completely separate from your `~/.jcode` — cruise never reads or writes it, and runs jcode with telemetry and the auto-update check disabled. Sign in with:

- `cruise login [provider]` — hands the terminal to `jcode login` (interactive picker / OAuth flow) against cruise's home.
- `cruise login <provider> --api-key` — non-interactive API-key entry (key from `CRUISE_LOGIN_API_KEY`, an echo-less prompt, or piped stdin — never a CLI argument).
- `cruise login --status` — lists the providers configured in cruise's home and their models.

Running `sdk: jcode` with no authenticated provider fails with an error pointing at `cruise login`. Custom OpenAI-compatible endpoints are added as jcode's own `[providers.<name>]` profiles (`jcode provider add`) in that home's `config.toml` — cruise adds no provider notation of its own.

### Custom tools via MCP

jcode cannot register custom tools in-process, so cruise's tools reach the model through a stdio MCP server (`cruise mcp-bridge`, registered in the home's `mcp.json`) and appear as `mcp__cruise__<tool>`. Caveat: jcode also merges MCP configuration from the run directory (`.jcode/mcp.json`, `.mcp.json`, `.claude/mcp.json`), last-wins over the home. A project-local server named `cruise` is a **hard error** (it would shadow cruise's tools); servers under other names load but are reported with a warning.

## `sdk: claude` — the claude CLI in-process

`sdk: claude` drives the `claude` CLI in-process through claude-agent-sdk, with cruise's tools exposed as `mcp__cruise__<tool>`. Model references are plain `claude --model` names with the optional `:effort` suffix (forwarded as `--effort`; a `claude` CLI without that flag fails the step with `unknown option '--effort'`, which is classified permanent and never retried — cruise is verified against 2.1.250). Authentication is the claude CLI's own — its stored credentials or `ANTHROPIC_API_KEY` — unaffected by `cruise login`. The CLI runs with permissions bypassed: cruise workflows are unattended, so there is no console to answer a permission prompt on.

## Differences from command mode

- **`env` applies to prompt steps**: top-level and per-step `env:` values are placed in the environment of the backend's child process (the `jcode` / `claude` CLI).
- **`{model}` placeholder is irrelevant**: it only exists for the `command` array.
- **Interactive planning**: during `cruise plan`, the SDK agent gets custom planning tools — `ask_user` (ask the user a clarifying question), `submit_plan` (write the plan markdown), and `update_plan` (find/replace a section of the existing plan). Both SDK backends support them. In non-interactive runs (no TTY), `ask_user` is not registered — the prompt instead tells the agent to decide on explicitly stated assumptions — but `submit_plan` and `update_plan` remain available, and a turn that ends without a successful `submit_plan`/`update_plan` call fails instead of falling back to the agent's final message as the plan. The interview-style `cruise plan --grill` mode builds on `ask_user`. Set `interactive_planning: false` to disable all of this and have the agent write `plan.md` directly, exactly like the `command` backend.
- **Run steps execute autonomously**: ordinary prompt steps get no custom tools; the agent's built-in tools do the file editing. The one exception is `skip_step` (see below), which is registered only on steps that need it.
- **Session continuity**: planning's plan/fix/ask turns resume the same backend session, so the agent keeps its context between turns.

## Commit guard

SDK prompt steps are protected from advancing Git `HEAD` by default. Set `allow_commit: true` on a prompt that is intentionally expected to create commits or otherwise move `HEAD`; omitted and `false` values keep the guard enabled and false values are omitted when configs are serialized. The `true` value bypasses all commit-guard behavior for that prompt. A guarded movement is reported as a commit-guard violation and fails the step, even if cruise restores the original branch reference without touching the index or worktree.

This guard covers both `sdk: jcode` and `sdk: claude` prompt execution. It does not apply to command or option steps, planning/title/PR-metadata calls, or cruise-owned PR worktree commits. `allow_commit: true` belongs on the actual prompt step, not a `group:` or `workflow_call:` invocation (those call sites reject the override).

### `skip_step` — declaring intentional no-changes (`if.no-file-changes` steps only)

A prompt step with an `if.no-file-changes` condition (`failed` or `retry`) additionally gets a `skip_step(reason)` tool: the agent calls it to declare that leaving the workspace unchanged this turn is the deliberate, correct outcome (for example, the plan explicitly says not to add tests), which disables that step's `if.no-file-changes` action for the current attempt. A plain-text alternative that needs no tool support at all — a `NO_CHANGES_INTENTIONAL: <reason>` line anchored at the start of a line in the step's output — has the same effect and works in `command:` mode too; see `flow-control.md` for both.

This tool is registered **only** on steps that carry `if.no-file-changes` — not on every run step — to keep the exposed tool set minimal on steps that can never call it.

`skip_step` only exists in SDK mode. In classic `command:` mode, `run_command` never sees `tools` at all, so there is no way for a `command:` step to call it — the `NO_CHANGES_INTENTIONAL:` output marker is the only option there.

Command and option steps behave identically in both modes.

## Rate limits and fallback

Both SDK backends retry according to the optional top-level `retry:` block (see `top-level.md`): declaring it — even as `model_fallback: false` or with no chains — makes HTTP 5xx and network errors retryable and switches the backoff to `base_delay_ms` doubling to an 8s ceiling. Without the block, only rate limits are retried, against the same model, with the command backend's 2s-doubling backoff (capped at 60s). The attempt budget is `--rate-limit-retries` either way; `--rate-limit-retries 0` means no retry and therefore no model switch, except for a model reference the backend refuses outright (nothing was sent, so the next chain entry is tried immediately). Every retry starts a **fresh session** — resuming a partially-answered session would duplicate context — and a turn that already streamed visible text is never retried on another model.
