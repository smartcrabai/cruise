# SDK backend: `sdk: jcode`

Setting the top-level `sdk` field selects the jcode SDK instead of the classic external `command` backend. `command` and `sdk` are mutually exclusive; when neither remains set after applicable `default.yaml` layers, prompts use the fallback `jcode` backend. The only supported SDK value is `"jcode"`. Any other value fails with `unsupported SDK "{value}"; only "jcode" is supported`, substituting the rejected value.

```yaml
sdk: jcode

# Every actual jcode prompt needs an effective model. Set one here, in
# applicable default.yaml, or through CRUISE_MODEL / CRUISE_PLAN_MODEL.
# Use `provider/model[:effort]` or a bare model supported by your jcode setup.
# model: provider/model[:effort]

steps:
  implement:
    prompt: "{input}"
```

## Mutual exclusivity with `command`

- Both `command` and `sdk` set → validation error.
- Neither remains set in the resolved workflow after applicable `default.yaml` layers are applied → valid: prompts run on the **fallback `jcode` backend**. An empty `command` array counts as "not set", so an `sdk`-only config is valid.
- `sdk` set to anything other than `jcode` → validation error: `unsupported SDK "{value}"; only "jcode" is supported`, substituting the rejected value.

## Model fields are plain model references

Workflow and app-config `model` and `plan_model` arrays may mix strings with `{model, disabled_during?}` objects:

```yaml
model:
  - openai/gpt-6.1-sol:medium
  - model: anthropic/claude-sonnet-4-6
    disabled_during: ["mon-fri 01:00-04:00"]
  - openrouter/z-ai/glm-5.3-flash:high
```

Strings use the workflow's effective SDK; without `sdk` or `command`, that is `jcode`. An object may declare an unavailable-time schedule, but an `sdk` property is unsupported and fails with `SDK-qualified model objects are unsupported; use a plain model string`. Model objects accept only `model` and `disabled_during`. This also applies to application `config.json` / `config.yaml` and layered `default.yaml`; `planModel` is an application-config alias.

Model objects cannot be used with `command`, including fallback and `plan_model` entries. String-only command arrays still use only the primary entry. `CRUISE_MODEL` and `CRUISE_PLAN_MODEL` replace whole lists; `CRUISE_SDK` can only select `jcode`. Explicit string-only `retry.fallback_chains` entries take precedence over array tails. Cooldown and candidate identity use the jcode model reference. Only jcode receives the caller's saved resume ID; failed attempts do not forward newly emitted IDs. A step model override does not inherit an unrelated global fallback list.

### User-declared unavailable windows

`disabled_during` may be used on a model-array object to declare unavailable
windows. SDK-qualified objects are rejected with
`SDK-qualified model objects are unsupported; use a plain model string`.

```yaml
model:
  - model: openai/gpt-5.5
    disabled_during: ["mon-fri 01:00-04:00"]
  - openrouter/z-ai/glm-5.3-flash
```

This is a user-chosen schedule, not a provider pricing preset. Equivalent fixed-offset windows are `mon-fri 10:00-13:00 +09:00` and `mon-fri 15:00-19:00 +09:00`.

- Grammar: `[days ]HH:MM-HH:MM[ offset]`. Weekdays `mon`..`sun` are case-insensitive; use comma lists and inclusive wrapping ranges, such as `sat,sun`, `mon,wed-fri`, and `fri-mon`. No days means daily.
- UTC is the default. Explicit `UTC`, `Z`, and fixed `+HH:MM` / `-HH:MM` offsets with absolute value at most `14:00` are accepted. IANA zones, DST, and local system time are not supported.
- Starts are `00:00`..`23:59`; ends are `00:01`..`24:00`. Endpoints are half-open, equal endpoints are invalid, and `00:00-24:00` covers all day. An earlier end crosses midnight. Days identify the start day, so `fri 22:00-02:00` includes Saturday 01:00 only as Friday's tail.
- Any matching window disables the candidate. Checks run at each prompt start, including planning/title/workflow generation and step reruns, and at retry candidate switches. A streaming turn is never interrupted. A disabled single candidate or an entirely disabled/cooling-down list fails without launching a backend.
- Window exclusions apply independently of retry policy, retry budget, and cooldowns. Skipping them consumes no retry budget and registers no cooldown. Capability validation still includes all candidates regardless of time.
- Candidate identity uses the jcode model reference and base model. String-only retry chains remain ordering rules and do not carry unavailable windows.
- `subagent_model` is a single string and cannot use `disabled_during`. Standalone model objects, per-step window fields, and window objects in `retry.fallback_chains` are unsupported. Environment model overrides replace the list and its windows.

In jcode mode, workflow and step `model` / `plan_model` have the same precedence as command mode (step `model` > top-level `model` / `plan_model`). A workflow-level model array uses its first entry as primary and remaining entries as an implicit fallback chain. Arrays with fallback entries enable fallback automatically; explicit string-only `retry.fallback_chains` entries take precedence. See [Rate limits and fallback](#rate-limits-and-fallback) for retry behavior.

| Form | Example | Behavior |
|------|---------|----------|
| `provider/model[:effort]` | `openai/gpt-5.5:high` | Explicit jcode provider and model. |
| `model` (no `/`) | `gpt-5.5` | Bare model; provider resolution is left to jcode. |
| unset | *(both `model` and `plan_model` omitted)* | A jcode prompt stops with a missing-model setup error. |

The `:effort` parser recognizes `low`/`medium`/`high`/`xhigh`/`max`, aliases `minimal`/`min`/`med`, and numeric spellings `1`..`4`. `off`/`none`/`0`/`5` leave effort unset; unknown suffixes such as `:free` remain part of the model ID.

A `/` with an empty side (`"/model"`, `"provider/"`) is rejected by jcode when the prompt runs — a step error, not a config-validation error, so `cruise plan --dry-run` does not catch it.

`subagent_model` is independent of the main `model` / `plan_model`. Set it at
workflow level or override it on a prompt step. It accepts exactly one
nonblank model string; object, array, and fallback-chain forms are unsupported.
It is available to jcode prompt steps and planning's plan/fix/ask turns, but
not to `command:` backends.

jcode sets the session's swarm default using `Agents` with `swarm` before the
prompt. A worker spawn with an explicit `model` wins over that default. This is
not a forced model and does not use jcode's user-invoked `/subagent` model
setting. Recognized `:effort` suffixes are rejected; the model must be valid
and nonempty.

The setting covers workflow prompt steps (including group, `after-pr`, parallel
prompt child, and `github-review` prompts) and planning's plan/fix/ask turns.
Title generation, PR metadata, `workflow generate`, and `workflow make` do not
use it. Non-prompt steps and group/workflow-call/parallel wrappers reject it.
A called workflow's top-level value is ignored in favor of the caller's
default, but per-step overrides inside the callee remain. Layered
`default.yaml` and `CRUISE_SUBAGENT_MODEL` overrides support it; application
`config.json` / `config.yaml` model defaults do not.

```yaml
sdk: jcode
model: openai/gpt-5.5
subagent_model: openai/gpt-5.5
steps:
  review:
    prompt: Delegate implementation checks and summarize findings.
    subagent_model: anthropic/claude-sonnet-4-6
```

## `sdk: jcode` — the jcode SDK (default)

`sdk: jcode` uses the official jcode Rust SDK and requires jcode v0.88.0 or newer. Cruise checks the SDK handshake for `sessions` and, when custom tools are registered, `session_tools`; it does not run a CLI version probe. A missing capability produces an actionable upgrade error. `model`, `plan_model`, and per-step `model` accept `provider/model[:effort]` or a bare model. Every actual jcode prompt must resolve to a model: ordinary prompts use step `model` then workflow/default `model`; planning calls use `plan_model` then `model`. An omitted model, blank value, or effort-only reference stops the call before jcode starts. Cruise creates or preserves the user-level `default.yaml` template to explain the setting; it never uses `JCODE_MODEL`, the jcode home default, or a resume session to fill in Cruise's missing model. Claude and external `command` backends retain their existing no-model behavior.

The GitHub Action's shell bootstrap uses `jcode version --json`, `jcode login`, `jcode provider add`, and observational `jcode auth status` because that shell setup has no SDK client and the SDK exposes neither CLI version metadata nor account/profile setup operations. The Cruise backend itself uses the SDK capability handshake after launch.

The startup OAuth expiration preflight checks explicit jcode providers `claude`, `claude-oauth`, and `claude-subprocess` via `<source home>/auth.json` (`anthropic_accounts[].expires` and `.refresh`), and `openai` and `openai-oauth` via `<source home>/openai-auth.json` (`openai_accounts[].expires_at` and `.refresh_token`). These account formats have no refresh-expiration field. With multiple accounts, Cruise blocks only when all accounts are conclusively expired; one usable or indeterminate account allows startup. Bare models, API-key providers (`anthropic-api`, `openai-api`), and other providers such as `openrouter` and `copilot` are skipped. See [the shared rules and limitations](#startup-oauth-expiration-checks).

Cruise resolves the process `JCODE_HOME` as its source home, falling back to `~/.jcode`. Each Cruise session gets an isolated SDK home that inherits credentials and config; resumed plan turns reuse it. The private runtime receives `JCODE_CHECK_UPDATES=0` without rewriting the copied config. Plan homes persist until their Cruise session is deleted or cleaned, while non-resumable prompt homes are removed after each turn. Unclaimed homes are pruned after 24 hours only when a cross-process lock proves no run is active. Before removing a private home, Cruise stops only a live `jcode serve` process whose command and `JCODE_HOME`/`JCODE_RUNTIME_DIR`/`JCODE_SOCKET` identify that runtime; it never trusts a registry PID alone. The absolute source-home path is saved with resumable plan sessions. The GitHub Action provisions credentials in its temporary source home; see [GitHub Actions](../../../docs/en/github-actions.md).

`ask_user` keeps one pending question across the SDK's 120-second callback deadline. After about 110 seconds without an answer, the callback returns a successful instruction for the model to call `ask_user` again with the identical question. Cruise does not re-prompt, and an answer arriving between calls is returned by the next call; the wait remains bounded by the workflow step timeout. If a saved plan conversation is missing from its private jcode home, Cruise starts a fresh SDK session because the current plan is already in the prompt context.

`submit_plan` and `update_plan` only write the plan and return; approval remains Cruise's normal post-turn flow rather than a blocking SDK callback.

**Upgrade note:** After upgrading from an MCP-bridge release, remove the stale `mcpServers.cruise` entry from the source `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json` when `JCODE_HOME` is unset). Cruise strips it from each private session copy and leaves the source configuration unchanged. Until the entry is removed, jcode sessions outside Cruise will continue to launch it. Cruise exits with migration instructions on stderr instead of treating it as legacy plan input, so jcode reports the `cruise` MCP server as failed.

The backend exposes Cruise session tools through jcode's session-tools support. Restricted `permission` modes force `macos_computer_use` off and disable the jcode tool `bash`. `read-only` additionally disables `edit`, `write`, `apply_patch`, and `replace`.

## Tool restriction enforcement

Workflow and step `disallowed_tools` lists use SDK-neutral `web`, `subagent`,
`mcp`, `mcp:<name>`, and `shell:<prefix>` values, not native backend syntax.
They are unioned with implicit Git denials when `allow_commit` is false.
`allow_commit: true` removes only those implicit rules and the commit guard,
not explicit restrictions or the permission preset. Restricted presets discard
shell rules because shell is already disabled. See [steps.md](steps.md) for
the complete implicit list, prefix grammar, and allowed positions.

| Backend | Enforcement |
|---------|-------------|
| jcode | Merge `webfetch`/`websearch`/`browser` and `swarm` into `JCODE_DISABLED_TOOLS`; append Cruise's shell pre-tool gate without discarding user hooks |
| `command:` | Explicit restrictions are errors; implicit shell denials are not enforced, while the commit guard remains |

jcode selects existing pre-tool hooks from workflow env, process env, then
`[hooks].pre_tool` in config, appending the gate. A gate without existing hooks
uses a single command string; combining hooks needs a jcode release supporting
hook arrays. `JCODE_HOOKS_DISABLED` in workflow or process env disables the gate
and warns. Failure to prepare the gate warns as well. The gate returns `2` for
a denial or malformed policy/input, but jcode hook execution failures and other
exit codes are fail-open. Tool-list merging also applies with
`computer_use: true` when explicit web/subagent denials exist.

jcode shell matching splits compound commands, strips assignments and common
wrappers, examines shell `-c` scripts, compares executable basenames, and
normalizes Git global options. Heredoc body lines may produce false positives.
These are best-effort controls, not a shell interpreter, sandbox, or security
boundary. The prompt warning, reference-transaction hook, and CAS rollback
remain in place.

MCP denials filter only workflow `mcp_servers` before backend launch. User-level
backend MCP configuration and Cruise's own session tools remain unaffected.
Built-in planning/fix/ask/title/PR-description and workflow-generation turns
remain `full` without explicit or implicit tool restrictions.


## Startup OAuth expiration checks

Before agents start for `cruise plan`, `cruise run`, `cruise exec`, or WebUI session planning/execution, Cruise checks credential files only for effective explicit jcode OAuth providers. Planning uses `plan_model` with `model` as its fallback. Execution includes workflow models and step overrides in ordinary steps, `after-pr`, groups, and parallel children, including model arrays and `disabled_during` entries. There is no global check merely for opening the WebUI. Command backends, dry-runs, `cruise workflow generate`, and `cruise workflow make` are not checked.

The jcode `claude`, `claude-oauth`, and `claude-subprocess` providers use `<source home>/auth.json` (`anthropic_accounts[].expires` and `.refresh`). The `openai` and `openai-oauth` providers use `<source home>/openai-auth.json` (`openai_accounts[].expires_at` and `.refresh_token`). These jcode provider names are separate from any external CLI. With multiple accounts, Cruise blocks only when all accounts are conclusively expired; one usable or indeterminate account allows startup. Bare models, API-key providers (`anthropic-api`, `openai-api`), and other providers such as `openrouter` and `copilot` are skipped. Sign in with `jcode login claude` or `jcode login openai`.

This is a read-only expiration preflight, not an authentication guarantee. It never runs auth-status CLIs, contacts providers, refreshes tokens, writes credentials, or takes credential locks. It cannot detect server-side refresh-token revocation and does not consult jcode's `auth-refresh-state.json`. Step-level `env` API keys are not considered, and jcode bare-model provider resolution is left to jcode at runtime. The GitHub Action's `anthropic-api` / `openai-api` providers are outside the check.

Set `CRUISE_SKIP_AGENT_AUTH_CHECK=1` in the process environment to bypass the check, for example after a false positive. Only the exact value `1` opts out; `true` and whitespace-padded values do not. This is not a workflow-config override.

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

Prompt `disallowed_tools: [mcp]` or `["mcp:<name>"]` can exclude workflow
servers for that step. This does not disable user-level MCP configuration or
Cruise's session tools, and does not restrict built-in turns.

The optional top-level `mcp_servers` map configures servers for all jcode prompt runs: workflow prompt steps (including parallel children), `after-pr` steps, planning turns, session-title generation, and PR-description generation. It is not a per-step setting, and a `workflow_call` callee's top-level MCP settings are ignored. MCP servers cannot be used with the `command` backend.

Only local `stdio` transport is supported: omit `type` or set `type: stdio`. HTTP and SSE entries fail config validation.

```yaml
sdk: jcode
mcp_servers:
  local_search:
    command: npx
    args: ["-y", "@example/search"]
```

Entries accept only `type`, `command`, `args`, and `env`. Names must be non-empty and contain only ASCII letters, digits, `_`, or `-`; `cruise` is reserved. Stdio requires a nonblank `command`. Unknown fields are rejected. Values are passed without Cruise `{variable}` resolution.

For jcode, Cruise merges workflow entries into the private session copy of `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json`) without changing the source. Workflow entries replace same-named source entries. jcode subsequently loads `~/.claude.json`, `~/.claude/mcp.json`, and project-local `.jcode/mcp.json`, `.mcp.json`, and `.claude/mcp.json`, so those later layers can replace a workflow server with the same name. jcode expands `${VAR}` and `${VAR:-default}` from the daemon's runtime environment, including workflow-level `env:` values.


## Rate limits and fallback


Jcode uses policy mode when the top-level `retry:` block is present (see `top-level.md`) or a workflow model array has fallback entries. In policy mode, a 429 switches immediately to a usable fallback (no `Retry-After` wait) and retries the same model only when none is left; HTTP 5xx, HTTP 4xx other than 429, 401, 403, 407, and 408, and network errors are retryable, with backoff using `base_delay_ms` doubling to an 8s ceiling. Such a 4xx never retries the same model: it switches to the next chain entry under the same conditions as a 5xx. A refused request leaves the model uncooled, unlike a 429/5xx/network skip. Errors naming the model as absent (`model_not_supported`, `model_not_found`, `model not found`, `unknown model`, `no such model`, `model does not exist`, `not a valid model`) are missing-model failures and cool the left-behind model for 30 minutes. A 400 can switch even when its message sounds permanent (`invalid request`, `context length`, `max_tokens`); `invalid_request_error: model not found` is a missing-model failure. A 401, 403, or 407 fails immediately. A 408 is a network failure: switch to a usable fallback first and resend to the same model only when none is left, visible text was streamed, or `--rate-limit-retries` is `0`. A failure without a status code stays permanent. Status numbers count only when a standalone three-digit number has `http`, `status`, `code`, `error`, `returned`, or `upstream` within the preceding 24 bytes; source locations and URL ports do not count. Rate-limit wording, 5xx, and network markers take priority over missing-model and bare 4xx-like text. Without policy mode, jcode retries classified provider limits (HTTP 429, `rate limit` / `too many requests`, `usage limit`, `session limit`, or `overloaded`) against the same model with 2-second-doubling backoff capped at 60s. `--rate-limit-retries` is the attempt budget; `--rate-limit-retries 0` disables retries and model switching, except when jcode rejects a model reference before startup, when the next chain entry is tried immediately. Every retry starts a **fresh session** and a turn that already streamed visible text is never retried on another model. A model array with fallback entries always enables switching, even if `retry.model_fallback: false` is also present.
