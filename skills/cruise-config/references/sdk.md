# SDK backends: `sdk: jcode` / `sdk: claude`

Setting the top-level `sdk` field selects an SDK execution path instead of the classic external `command` backend. Two values are accepted: `"jcode"` and `"claude"`. Any other value is a validation error.

```yaml
sdk: jcode

# Optional model references use `provider/model[:effort]` or a bare model.
# Replace this placeholder with a route available in your jcode setup.
# model: provider/model[:effort]

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
| `provider/model[:effort]` | `provider/model` | Explicit provider + model syntax for `sdk: jcode`. `sdk: claude` treats everything before `:effort` as the model name. |
| `:effort` | `:high` | Provider/model left to the backend default; the requested effort is applied. |
| `model` (no `/`) | `model-name` | Provider left to the backend's own resolution. |
| unset | *(both `model` and `plan_model` omitted)* | The backend's configured default provider/model is used. |

The `:effort` suffix is recognized only when it names an effort tier (`low`/`medium`/`high`/`xhigh`/`max`, the aliases `minimal`/`min`/`med`, or the numeric spellings `1`..`4`) or one of `off`/`none`/`0`/`5` — those four are stripped from the model id but leave the effort unset. Any other `:` suffix (e.g. an OpenRouter `:free` variant) stays part of the model id, so watch out for a model id whose own suffix happens to be `:0`..`:5`.

A `/` with an empty side (`"/model"`, `"provider/"`) is rejected by `sdk: jcode` when the prompt runs — a step error, not a config-validation error, so `cruise plan --dry-run` does not catch it. `sdk: claude` forwards it to the CLI, which fails with its own message.

## `sdk: jcode` — the jcode SDK (default)

`sdk: jcode` uses jcode's SDK integration and requires the `jcode` CLI v0.88.0 or newer; older binaries are rejected. `model`, `plan_model`, and per-step `model` accept `provider/model[:effort]`, a bare `model`, or no value. The effort suffix is applied through the SDK.

Cruise uses the process `JCODE_HOME` as its source home, falling back to `~/.jcode`. It creates a private SDK home for each Cruise session, inherits credentials and config into it, and reuses that home for resumed turns. Plan homes persist until the Cruise session is deleted or cleaned; ordinary non-resumable prompt homes are removed after the turn. Homes left before a session ID is saved are pruned after 24 hours, stopping the owned daemon first. Each prompt attempt launches a private jcode runtime against the session home. The GitHub Action provisions credentials in its temporary source home; see [GitHub Actions](../../../docs/github-actions.md).

**Upgrade note:** After upgrading from an MCP-bridge release, remove the stale `mcpServers.cruise` entry from the source `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json` when `JCODE_HOME` is unset). Cruise strips it from each private session copy and leaves the source configuration unchanged.

The backend exposes Cruise session tools through jcode's session-tools support.

## `sdk: claude` — the claude CLI in-process

`sdk: claude` drives the `claude` CLI in-process through claude-agent-sdk. Model references are plain `claude --model` names with the optional `:effort` suffix (forwarded as `--effort`; a `claude` CLI without that flag fails the step with `unknown option '--effort'`, which is classified permanent and never retried — cruise is verified against 2.1.250). Authentication is the claude CLI's own — its stored credentials or `ANTHROPIC_API_KEY` — unrelated to `jcode login`. The CLI runs with permissions bypassed -- cruise workflows are unattended, so there is no console to answer a permission prompt on. Cruise's workflow tools are exposed through the SDK.

## Differences from command mode

- **`env` applies to prompt steps**: top-level and per-step `env:` values are passed to SDK prompt execution; command steps receive them in their child process.
- **`{model}` placeholder is irrelevant**: it only exists for the `command` array.
- **Interactive planning**: when enabled, SDK planning can ask questions and submit or revise `plan.md`; non-interactive runs omit user interaction. Set `interactive_planning: false` to have the agent write `plan.md` directly, as in `command` mode.
- **Run steps execute autonomously**: ordinary prompt steps use the provider's built-in tools for file editing. Cruise only adds its workflow-specific session capabilities where needed.
- **Session continuity**: planning's plan/fix/ask turns share one backend session only when an SDK backend is active and `interactive_planning` is `true`; with `interactive_planning: false`, each turn starts a fresh backend session.

### Structured intentional no-changes (`if.no-file-changes` steps only)

A prompt step with an `if.no-file-changes` condition (`failed` or `retry`) additionally gets a structured SDK action for declaring that leaving the workspace unchanged is deliberate (for example, the plan explicitly says not to add tests). This disables that step's `if.no-file-changes` action for the current attempt. A plain-text alternative — a `NO_CHANGES_INTENTIONAL: <reason>` line anchored at the start of a line in the step's output — has the same effect and works in `command:` mode too; see `flow-control.md` for both.

The structured action is available only on prompt steps that carry an `if.no-file-changes` condition. In classic `command:` mode, the output marker is the only option.

Command and option steps behave identically in both modes.


## Rate limits and fallback

Both SDK backends use policy mode when the top-level `retry:` block is present (see `top-level.md`) or when a workflow-level model array with fallback entries creates an implicit policy. In policy mode, HTTP 5xx, HTTP 4xx other than 429, 401, 403, 407, and 408, and network errors are retryable and the backoff uses `base_delay_ms` doubling to an 8s ceiling. Such a 4xx never retries the same model: it switches to the next chain entry under the same conditions as a 5xx, and what the failure says about the model decides the cooldown. A refused request leaves the left-behind model uncooled, unlike a 429/5xx/network skip, because the status describes the request; text naming the model as absent (`model_not_supported`, `model_not_found`, `model not found`, `unknown model`, `no such model`, `model does not exist`, `not a valid model`, read from the provider's own error rather than the appended CLI stderr) is classified as a missing model instead and does cool the left-behind model for the same 30 minutes, so later turns and steps stop selecting a model the provider does not have. A 400 qualifies as a switch even when the message reads permanent (`invalid request`, `context length`, `max_tokens`), since another model may accept the request — and `invalid_request_error: model not found` is read as a missing model, not as a permanent failure; a 401, 403, or 407 fails the step immediately, because no other model can answer an auth or permission failure; a 408 is classified as a network failure, so it takes the 5xx/network path — switch to a usable fallback first, same-model resend only when no fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0` — and its skipped model is cooled down like any other network skip; a failure carrying no status code (`unknown option '--effort'`) stays permanent and fails the step at once. A number in the failure text counts as an HTTP status only when it is a standalone three-digit number with `http`, `status`, `code`, `error`, `returned`, or `upstream` within the preceding 24 bytes, and source locations (`src/lib.rs:404:17`) and URL ports (`https://host:443/path`) never qualify; rate-limit wording, 5xx, and the network markers are decided before the missing-model wordings, which in turn are decided before any bare 4xx-looking number, so appended stderr noise cannot downgrade a 429 or a 503. Without either policy, SDK backends retry any provider failure classified as a limit—HTTP 429, `rate limit` / `too many requests` text, `usage limit`, `session limit`, or `overloaded`—against the same model with 2-second-doubling backoff (capped at 60s). The attempt budget is `--rate-limit-retries` either way; `--rate-limit-retries 0` means no retry and therefore no model switch, except for a model reference the backend refuses outright (nothing was sent, so the next chain entry is tried immediately). Every retry starts a **fresh session** — resuming a partially-answered session would duplicate context — and a turn that already streamed visible text is never retried on another model. A model array with fallback entries always enables switching, even if `retry.model_fallback: false` is also present.
