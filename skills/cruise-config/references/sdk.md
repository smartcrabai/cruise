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

`sdk: jcode` uses the official jcode Rust SDK and requires jcode v0.88.0 or newer. Cruise checks the SDK handshake for `sessions` and, when custom tools are registered, `session_tools`; it does not run a CLI version probe. A missing capability produces an actionable upgrade error. `model`, `plan_model`, and per-step `model` accept `provider/model[:effort]`, a bare `model`, or no value. The effort suffix is applied through the SDK.

The GitHub Action's shell bootstrap uses `jcode version --json`, `jcode login`, `jcode provider add`, and observational `jcode auth status` because that shell setup has no SDK client and the SDK exposes neither CLI version metadata nor account/profile setup operations. The Cruise backend itself uses the SDK capability handshake after launch.

Cruise resolves the process `JCODE_HOME` as its source home, falling back to `~/.jcode`. Each Cruise session gets an isolated SDK home that inherits credentials and config; resumed plan turns reuse it. The private runtime receives `JCODE_CHECK_UPDATES=0` without rewriting the copied config. Plan homes persist until their Cruise session is deleted or cleaned, while non-resumable prompt homes are removed after each turn. Unclaimed homes are pruned after 24 hours only when a cross-process lock proves no run is active. Before removing a private home, Cruise stops only a live `jcode serve` process whose command and `JCODE_HOME`/`JCODE_RUNTIME_DIR`/`JCODE_SOCKET` identify that runtime; it never trusts a registry PID alone. The absolute source-home path is saved with resumable plan sessions. The GitHub Action provisions credentials in its temporary source home; see [GitHub Actions](../../../docs/github-actions.md).

`ask_user` keeps one pending question across the SDK's 120-second callback deadline. After about 110 seconds without an answer, the callback returns a successful instruction for the model to call `ask_user` again with the identical question. Cruise does not re-prompt, and an answer arriving between calls is returned by the next call; the wait remains bounded by the workflow step timeout. If a saved plan conversation is missing from its private jcode home, Cruise starts a fresh SDK session because the current plan is already in the prompt context.

`submit_plan` and `update_plan` only write the plan and return; approval remains Cruise's normal post-turn flow rather than a blocking SDK callback.

**Upgrade note:** After upgrading from an MCP-bridge release, remove the stale `mcpServers.cruise` entry from the source `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json` when `JCODE_HOME` is unset). Cruise strips it from each private session copy and leaves the source configuration unchanged.

The backend exposes Cruise session tools through jcode's session-tools support. Restricted `permission` modes force `macos_computer_use` off and disable the jcode tool `bash`. `read-only` additionally disables `edit`, `write` and `apply_patch`.

## `sdk: claude` — the claude CLI in-process

`sdk: claude` drives the `claude` CLI in-process through claude-agent-sdk. Model references are plain `claude --model` names with the optional `:effort` suffix (forwarded as `--effort`; a `claude` CLI without that flag fails the step with `unknown option '--effort'`, which is classified permanent and never retried — cruise is verified against 2.1.250). Authentication is the claude CLI's own — its stored credentials or `ANTHROPIC_API_KEY` — unrelated to `jcode login`. With `permission: full` (the default) the CLI runs with permissions bypassed -- cruise workflows are unattended, so there is no console to answer a permission prompt on. `read-only` and `edit` run as `dontAsk` with tool deny lists. Cruise's workflow tools are exposed through the SDK.

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

## MCP servers

The optional top-level `mcp_servers` map configures servers for all SDK prompt runs: workflow prompt steps (including parallel children), `after-pr` steps, planning turns, session-title generation, and PR-description generation. It is not a per-step setting, and a `workflow_call` callee's top-level MCP settings are ignored. Stdio entries work with `sdk: jcode`, the default jcode backend, and `sdk: claude`. HTTP and SSE entries require `sdk: claude` because jcode currently supports stdio only.

```yaml
sdk: claude
mcp_servers:
  local_search:
    command: npx
    args: ["-y", "@example/search"]
  remote_docs:
    type: http
    url: https://mcp.example.test/server
    headers:
      Authorization: "Bearer replace-with-your-token"
```

Entries use the Claude Code `mcpServers` shape and accept only `type`, `command`, `args`, `env`, `url`, and `headers`. Omitted `type` means `stdio`. Names must be non-empty and contain only ASCII letters, digits, `_`, or `-`; `cruise` is reserved. Stdio requires a non-blank `command` and rejects `url` and non-empty `headers`. HTTP/SSE require a URL starting with `http://` or `https://` and reject `command` and non-empty `args` or `env`. `mcp_servers` with the `command` backend is a validation error. Values are passed without Cruise `{variable}` resolution.

For jcode, Cruise merges workflow entries into the private session copy of `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json`) without changing the source. Workflow entries replace same-named source entries. jcode subsequently loads `~/.claude.json`, `~/.claude/mcp.json`, and project-local `.jcode/mcp.json`, `.mcp.json`, and `.claude/mcp.json`, so those later layers can replace a workflow server with the same name. jcode expands `${VAR}` and `${VAR:-default}` from the daemon's runtime environment, including workflow-level `env:` values.

For Claude, Cruise writes workflow entries to a per-run private JSON file with mode `0600` on Unix and passes the file path via `--mcp-config`; the SDK passes its in-process `cruise` tools separately as its own inline `--mcp-config` value. The user's own Claude MCP config remains enabled. `${VAR}` expansion for this backend is not guaranteed.


## Rate limits and fallback

Both SDK backends use policy mode when the top-level `retry:` block is present (see `top-level.md`) or when a workflow-level model array with fallback entries creates an implicit policy. In policy mode, HTTP 5xx, HTTP 4xx other than 429, 401, 403, 407, and 408, and network errors are retryable and the backoff uses `base_delay_ms` doubling to an 8s ceiling. Such a 4xx never retries the same model: it switches to the next chain entry under the same conditions as a 5xx, and what the failure says about the model decides the cooldown. A refused request leaves the left-behind model uncooled, unlike a 429/5xx/network skip, because the status describes the request; text naming the model as absent (`model_not_supported`, `model_not_found`, `model not found`, `unknown model`, `no such model`, `model does not exist`, `not a valid model`, read from the provider's own error rather than the appended CLI stderr) is classified as a missing model instead and does cool the left-behind model for the same 30 minutes, so later turns and steps stop selecting a model the provider does not have. A 400 qualifies as a switch even when the message reads permanent (`invalid request`, `context length`, `max_tokens`), since another model may accept the request — and `invalid_request_error: model not found` is read as a missing model, not as a permanent failure; a 401, 403, or 407 fails the step immediately, because no other model can answer an auth or permission failure; a 408 is classified as a network failure, so it takes the 5xx/network path — switch to a usable fallback first, same-model resend only when no fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0` — and its skipped model is cooled down like any other network skip; a failure carrying no status code (`unknown option '--effort'`) stays permanent and fails the step at once. A number in the failure text counts as an HTTP status only when it is a standalone three-digit number with `http`, `status`, `code`, `error`, `returned`, or `upstream` within the preceding 24 bytes, and source locations (`src/lib.rs:404:17`) and URL ports (`https://host:443/path`) never qualify; rate-limit wording, 5xx, and the network markers are decided before the missing-model wordings, which in turn are decided before any bare 4xx-looking number, so appended stderr noise cannot downgrade a 429 or a 503. Without either policy, SDK backends retry any provider failure classified as a limit—HTTP 429, `rate limit` / `too many requests` text, `usage limit`, `session limit`, or `overloaded`—against the same model with 2-second-doubling backoff (capped at 60s). The attempt budget is `--rate-limit-retries` either way; `--rate-limit-retries 0` means no retry and therefore no model switch, except for a model reference the backend refuses outright (nothing was sent, so the next chain entry is tried immediately). Every retry starts a **fresh session** — resuming a partially-answered session would duplicate context — and a turn that already streamed visible text is never retried on another model. A model array with fallback entries always enables switching, even if `retry.model_fallback: false` is also present.
