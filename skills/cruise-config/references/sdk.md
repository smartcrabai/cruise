# SDK backends: `sdk: jcode` / `sdk: claude` / `sdk: opencode`

Setting the top-level `sdk` field selects an SDK execution path instead of the classic external `command` backend. Three values are accepted: `"jcode"`, `"claude"`, and `"opencode"`. Any other value is a validation error.

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
- `sdk` set to anything other than `jcode` / `claude` / `opencode` → validation error.

## Model fields are plain model references

Workflow and app-config `model` and `plan_model` arrays may mix strings with strict `{model, sdk?, disabled_during?}` objects:

```yaml
model:
  - openai/gpt-6.1-sol:medium
  - sdk: claude
    model: claude-sonnet-5-5:medium
  - openrouter/z-ai/glm-5.3-flash:high
```

String entries always inherit the workflow's effective SDK, not the preceding object's SDK. Without `sdk` or `command`, that SDK is jcode. Objects with `sdk` select `jcode`, `claude`, or the existing `opencode` backend for that entry only; objects without `sdk` inherit the workflow SDK. `openai/` and `openrouter/` remain model-provider prefixes. Each object requires a nonblank `model` and at least one of nonblank `sdk` or nonempty `disabled_during`, rejects explicit null SDKs, and rejects unknown keys. Standalone model objects and per-step SDK fields are not supported. The JSON equivalent uses the same array of strings and objects, and saving preserves their original forms.

Any model object conflicts with `command`, including fallback and `plan_model` entries. String-only command arrays still use only the primary. `CRUISE_MODEL` and `CRUISE_PLAN_MODEL` replace whole lists, while `CRUISE_SDK` changes inheritance for strings and objects without `sdk`, without changing explicit SDK overrides. Explicit string-only `retry.fallback_chains` take precedence. Cooldown and candidate identity distinguish SDKs even for identical model names. Only the owning SDK receives the caller's saved resume ID. Another SDK starts without it, and returning to the original owner may reuse that original ID and its retained storage. Newly emitted IDs from failed attempts are not forwarded to subsequent attempts. All applicable candidates must support the request: computer use requires jcode, jcode accepts only stdio MCP, and OpenCode rejects SSE. A step model override does not inherit an unrelated global fallback list.

### User-declared unavailable windows

`disabled_during` on a `model` or `plan_model` array object is a nonempty list of windows. It is supported through workflow, application-config, and layered defaults without requiring an explicit SDK on the object:

```yaml
sdk: opencode
model:
  - model: opencode-go/deepseek-v4.1-flash
    disabled_during: ["mon-fri 01:00-04:00", "mon-fri 06:00-10:00"]
  - opencode-go/kimi-k2.6
```

This is a user-chosen schedule, not a provider pricing preset. Equivalent fixed-offset windows are `mon-fri 10:00-13:00 +09:00` and `mon-fri 15:00-19:00 +09:00`.

- Grammar: `[days ]HH:MM-HH:MM[ offset]`. Weekdays `mon`..`sun` are case-insensitive; use comma lists and inclusive wrapping ranges, such as `sat,sun`, `mon,wed-fri`, and `fri-mon`. No days means daily.
- UTC is the default. Explicit `UTC`, `Z`, and fixed `+HH:MM` / `-HH:MM` offsets with absolute value at most `14:00` are accepted. IANA zones, DST, and local system time are not supported.
- Starts are `00:00`..`23:59`; ends are `00:01`..`24:00`. Endpoints are half-open, equal endpoints are invalid, and `00:00-24:00` covers all day. An earlier end crosses midnight. Days identify the start day, so `fri 22:00-02:00` includes Saturday 01:00 only as Friday's tail.
- Any matching window disables the candidate. Checks run at each prompt start, including planning/title/workflow generation and step reruns, and at retry candidate switches. A streaming turn is never interrupted. A disabled single candidate or an entirely disabled/cooling-down list fails without launching a backend.
- Window exclusions apply independently of retry policy, retry budget, and cooldowns. Skipping them consumes no retry budget and registers no cooldown. Capability validation still includes all candidates regardless of time.
- Identity is resolved SDK plus base model, shared by effort/native-variant references and matching string entries in explicit retry chains. Such chains remain string-only and take precedence for ordering, not availability.
- A step model override inherits the workflow list and its windows only when its resolved SDK and base model both match the primary candidate. Changing only effort preserves inheritance when the SDK also matches. Other overrides do not inherit the list. `subagent_model` rejects windows and still requires `sdk` in object form. Standalone model objects, per-step window fields, and window objects in `retry.fallback_chains` are unsupported. Environment model overrides replace the list and its windows.

In all SDK backends, `model` / `plan_model` / per-step `model` carry the same precedence as command mode (step `model` > top-level `model` / `plan_model`). Workflow-level `model` and `plan_model` may be arrays: the first entry is primary and the remaining entries are an implicit fallback chain. Arrays with fallback entries enable model fallback automatically; explicit `retry.fallback_chains` entries for a primary model take precedence. A 429 retries the current model while its retry budget and delay permit, while a 5xx, an HTTP 4xx other than 429, 401, 403, 407, and 408, or a network failure switches immediately to a usable fallback when `--rate-limit-retries` is above zero and no visible text was streamed. Such a 4xx is never resent to the same model — an identical request would fail identically — so it either switches or surfaces as-is; 401, 403, and 407 are auth or permission failures another model cannot fix and fail the step at once, and a 408 is treated as a network failure, so it switches to a usable fallback first exactly like a 5xx and is resent to the same model only when no usable fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0`. Models skipped after a 429, a 5xx, or a network failure (a 408 included) are cooled down for 30 minutes in the current process, and so are models the provider named as missing (`400 model_not_supported`, `404 model_not_found`); only a model left behind by a plain client-error switch is not. Each scalar value is a model reference:

| Form | Example | Behavior |
|------|---------|----------|
| `provider/model[:effort]` | `provider/model` | Explicit provider + model for jcode and OpenCode (catalog-validated effort). Claude treats everything before `:effort` as the model name. |
| `:effort` | `:high` | Claude may use its backend model default with this effort. Jcode rejects this because it does not identify a model. |
| `model` (no `/`) | `model-name` | jcode/Claude only: provider left to the backend's own resolution. OpenCode requires an explicit provider for configured references. |
| `provider/model#variant` | `provider/model#variant` | OpenCode V2 native variant, mutually exclusive with common `:effort`. Embedded `/` and unknown colon suffixes in the model ID are preserved. |
| unset | *(both `model` and `plan_model` omitted)* | A jcode prompt stops with a missing-model setup error. Claude and OpenCode keep their backend-default behavior. |

The common `:effort` parser recognizes tiers (`low`/`medium`/`high`/`xhigh`/`max`, aliases `minimal`/`min`/`med`, numeric spellings `1`..`4`) case-insensitively. For jcode and Claude, `off`/`none`/`0`/`5` are also stripped from the model ID but leave effort unset. OpenCode rejects those tierless suffixes instead (see below). Unknown colon suffixes such as `:free` stay in the ID; recognized trailing suffixes are reserved.

A `/` with an empty side (`"/model"`, `"provider/"`) is rejected by `sdk: jcode` when the prompt runs — a step error, not a config-validation error, so `cruise plan --dry-run` does not catch it. `sdk: claude` forwards it to the CLI, which fails with its own message.

## Subagent model

`subagent_model` is independent of the main `model` / `plan_model`. Set it at
workflow level or override it on a prompt step. It accepts exactly one nonblank
string or `{sdk: claude, model: haiku}` object (`jcode` is also supported), with
no array or fallback chain. Strings inherit the workflow SDK, defaulting to
jcode; explicit objects resolve in their named SDK. Each execution/fallback
attempt receives the setting only when its SDK matches that resolved SDK.
Other attempts retain their backend defaults. Validation rejects values with
no applicable candidate, OpenCode subagent models, and all `command:` backends.
Mixed-SDK workflows can use an explicit object for a Claude/jcode candidate.

Recognized `:effort` suffixes, including `:off`, are rejected rather than
silently ignored. Unknown suffixes such as `:free` remain part of the model ID.
jcode model references must contain a valid nonempty model (not `provider/`).

Claude injects `CLAUDE_CODE_SUBAGENT_MODEL=<model>` and
`CLAUDE_CODE_SUBAGENT_MODEL_FORCE=1` to fix all subagents, including Explore and
Plan. These override same-named workflow/step `env:` entries. If the field is
unset, Cruise leaves those keys untouched. jcode instead sets the session's
swarm default using `Agents` with `swarm` before sending the prompt; a worker
spawn with an explicit `model` wins over that default. It is not a forced model
and does not use jcode's user-invoked `/subagent` model setting.

The setting covers workflow prompt steps (including group, `after-pr`, parallel
prompt child, and `github-review` prompts) and planning's plan/fix/ask turns.
Title generation, PR metadata, and `workflow generate` do not use it. Non-prompt
steps and group/workflow-call/parallel wrappers reject it. A called workflow's
top-level value is ignored in favor of the caller's default, but per-step
overrides inside the callee remain. Layered `default.yaml` supports it;
application `config.json` / `config.yaml` defaults and
`CRUISE_SUBAGENT_MODEL` overrides are not provided.

```yaml
sdk: claude
model: sonnet
subagent_model: haiku
steps:
  review:
    prompt: Delegate implementation checks and summarize findings.
    subagent_model: {sdk: claude, model: sonnet}
```

## `sdk: jcode` — the jcode SDK (default)

`sdk: jcode` uses the official jcode Rust SDK and requires jcode v0.88.0 or newer. Cruise checks the SDK handshake for `sessions` and, when custom tools are registered, `session_tools`; it does not run a CLI version probe. A missing capability produces an actionable upgrade error. `model`, `plan_model`, and per-step `model` accept `provider/model[:effort]` or a bare model. Every actual jcode prompt must resolve to a model: ordinary prompts use step `model` then workflow/default `model`; planning calls use `plan_model` then `model`. An omitted model, blank value, or effort-only reference stops the call before jcode starts. Cruise creates or preserves the user-level `default.yaml` template to explain the setting; it never uses `JCODE_MODEL`, the jcode home default, or a resume session to fill in Cruise's missing model. Claude and external `command` backends retain their existing no-model behavior.

The GitHub Action's shell bootstrap uses `jcode version --json`, `jcode login`, `jcode provider add`, and observational `jcode auth status` because that shell setup has no SDK client and the SDK exposes neither CLI version metadata nor account/profile setup operations. The Cruise backend itself uses the SDK capability handshake after launch.

The startup OAuth expiration preflight checks explicit jcode providers `claude`, `claude-oauth`, and `claude-subprocess` via `<source home>/auth.json` (`anthropic_accounts[].expires` and `.refresh`), and `openai` and `openai-oauth` via `<source home>/openai-auth.json` (`openai_accounts[].expires_at` and `.refresh_token`). These account formats have no refresh-expiration field. With multiple accounts, Cruise blocks only when all accounts are conclusively expired; one usable or indeterminate account allows startup. Bare models, API-key providers (`anthropic-api`, `openai-api`), and other providers such as `openrouter` and `copilot` are skipped. See [the shared rules and limitations](#startup-oauth-expiration-checks).

Cruise resolves the process `JCODE_HOME` as its source home, falling back to `~/.jcode`. Each Cruise session gets an isolated SDK home that inherits credentials and config; resumed plan turns reuse it. The private runtime receives `JCODE_CHECK_UPDATES=0` without rewriting the copied config. Plan homes persist until their Cruise session is deleted or cleaned, while non-resumable prompt homes are removed after each turn. Unclaimed homes are pruned after 24 hours only when a cross-process lock proves no run is active. Before removing a private home, Cruise stops only a live `jcode serve` process whose command and `JCODE_HOME`/`JCODE_RUNTIME_DIR`/`JCODE_SOCKET` identify that runtime; it never trusts a registry PID alone. The absolute source-home path is saved with resumable plan sessions. The GitHub Action provisions credentials in its temporary source home; see [GitHub Actions](../../../docs/github-actions.md).

`ask_user` keeps one pending question across the SDK's 120-second callback deadline. After about 110 seconds without an answer, the callback returns a successful instruction for the model to call `ask_user` again with the identical question. Cruise does not re-prompt, and an answer arriving between calls is returned by the next call; the wait remains bounded by the workflow step timeout. If a saved plan conversation is missing from its private jcode home, Cruise starts a fresh SDK session because the current plan is already in the prompt context.

`submit_plan` and `update_plan` only write the plan and return; approval remains Cruise's normal post-turn flow rather than a blocking SDK callback.

**Upgrade note:** After upgrading from an MCP-bridge release, remove the stale `mcpServers.cruise` entry from the source `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json` when `JCODE_HOME` is unset). Cruise strips it from each private session copy and leaves the source configuration unchanged.

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
| Claude | Native denies for `WebFetch`/`WebSearch`, `Agent`, and `Bash(<prefix> *)` (or `Bash(<prefix>*)` when ending in `=`), including in `bypassPermissions` |
| jcode | Merge `webfetch`/`websearch`/`browser` and `swarm` into `JCODE_DISABLED_TOOLS`; append Cruise's shell pre-tool gate without discarding user hooks |
| OpenCode | Native web/task/bash deny rules and Cruise plugin/bridge checks before tool execution |
| `command:` | Explicit restrictions are errors; implicit shell denials are not enforced, while the commit guard remains |

jcode selects existing pre-tool hooks from workflow env, process env, then
`[hooks].pre_tool` in config, appending the gate. A gate without existing hooks
uses a single command string; combining hooks needs a jcode release supporting
hook arrays. `JCODE_HOOKS_DISABLED` in workflow or process env disables the gate
and warns. Failure to prepare the gate warns as well. The gate returns `2` for
a denial or malformed policy/input, but jcode hook execution failures and other
exit codes are fail-open. Tool-list merging also applies with
`computer_use: true` when explicit web/subagent denials exist.

jcode/OpenCode shell matching splits compound commands, strips assignments
and common wrappers, examines shell `-c` scripts, compares executable basenames,
and normalizes Git global options. Heredoc body lines may produce false
positives. Claude uses native matching instead and does **not** normalize Git
global options: `git -C path commit` may evade a `git commit` prefix rule.
These are best-effort controls, not a shell interpreter, sandbox, or security
boundary. The prompt warning, reference-transaction hook, and CAS rollback
remain in place.

MCP denials filter only workflow `mcp_servers` before backend launch. User-level
backend MCP configuration and Cruise's own session tools remain unaffected.
Built-in planning/fix/ask/title/PR-description and workflow-generation turns
remain `full` without explicit or implicit tool restrictions.

## `sdk: opencode` - the OpenCode V2 client

Cruise's built-in Rust client drives only the workflow-required V2 API. Install the OpenCode V2 CLI and configure its providers/authentication separately. Cruise launches a short-lived loopback `opencode serve --stdio` process and stops it after each attempt. No pre-started or external server, V1 fallback, Python/Node installation, CLI auto-install, or authentication operation is provided.

```yaml
sdk: opencode
model: "provider/model#variant" # Replace with your OpenCode model
plan_model: "provider/planning-model"
interactive_planning: true
permission: full
steps:
  implement:
    prompt: "{input}"
```

All model fields accept `provider/model[:effort]` or native `provider/model#variant`, including workflow/planning/step models, array candidates, and resumed sessions. Common effort names and aliases are case-insensitive: `low` (`min`/`minimal`/`1`), `medium` (`med`/`2`), `high` (`3`), `xhigh` (`4`), and `max`. Recognized trailing suffixes are reserved; unknown suffixes such as `:free`/`:2026` and embedded `/` remain in the ID. Common effort cannot be combined with `#variant`; tierless `:off`/`:none`/`:0`/`:5` fail explicitly, while native `#none` remains valid.

Cruise resolves effort against the exact provider/model in OpenCode's live catalog, choosing the canonical variant ID when its `settings.reasoningEffort` is absent or agrees, otherwise a uniquely matching `settings.reasoningEffort` variant. Unsupported or ambiguous effort, an unlisted model, and catalog/API failures stop the attempt without fallback, before prompt submission or resume model changes. There is no guessed mapping, downgrade, or default substitution. Suffixless, native-variant, and omitted models need no catalog lookup. Arrays, retry chains, and provider wildcards preserve common effort as `:effort` so each candidate is validated separately. Effort and native variants share the base model's deduplication/cooldown identity.

Automatic retry/fallback is blocked after tool side effects, uncertain prompt acceptance, or unrecoverable event loss, even without displayed text. Omitted models use OpenCode's configured default. Defaults treat OpenCode as an independent backend, preventing inherited jcode/Claude models from leaking across backend changes. `CRUISE_SDK=opencode` uses the normal environment override path.

Existing global/project OpenCode configuration and credentials remain available. Cruise owns only its private XDG database, native plugin, bridge configuration, and saved planning conversations. It does not generate repository runtime files or modify the user's OpenCode database/configuration. Workflow/step environment values merge with inherited OS values and the commit guard. `OPENCODE_CONFIG_CONTENT` is parsed as JSONC and structurally overlaid; invalid content is an error.

The native V2 plugin exposes the existing `ask_user`, `submit_plan`, `update_plan`, `generate_title`, `submit_pr_metadata`, and `skip_step` tools, not an MCP server. Saved plan conversations are backend-owned and resumable. Missing or uncertain resume data is diagnosed, not silently replaced by a new conversation.

`computer_use: true` is rejected at workflow, step, group, after-PR, and parallel-child positions. `false` is allowed on prompt positions. `permission: full` permits unattended tool execution while preserving commit guards. `edit` permits direct edits but denies shell, subagents, generic/unknown execution tools, and Code Mode bypasses. `read-only` also denies mutation and retains safe Cruise tools plus workspace snapshot verification. Unknown MCP tools are denied in restricted modes. This is not an OS sandbox.

Stdio MCP entries become V2 local servers. `type: http` entries become remote Streamable HTTP servers with URL and headers preserved. Legacy `type: sse` is explicitly rejected, never converted. No unattended OAuth login is started. Workflow entries overlay the user's configuration without rewriting it.

Protocol baseline: OpenCode commit `74f8d4860f49c563909e6b039b0cc8b454ebce72`, investigated October 23, 2026. v2.1.3 is the initial real-CLI verification target, not a guarantee for every V2 release. Fixture checks and version output alone do not prove real-model/plugin compatibility. See [opencode-flow.yaml](../examples/opencode-flow.yaml).

## `sdk: claude` — the claude CLI in-process

`sdk: claude` drives the `claude` CLI in-process through claude-agent-sdk. Model references are plain `claude --model` names with the optional `:effort` suffix (forwarded as `--effort`; a `claude` CLI without that flag fails the step with `unknown option '--effort'`, which is classified permanent and never retried — cruise is verified against 2.1.250). Authentication is the claude CLI's own — its stored credentials or `ANTHROPIC_API_KEY` — unrelated to `jcode login`. With `permission: full` (the default) the CLI runs with permissions bypassed -- cruise workflows are unattended, so there is no console to answer a permission prompt on. `read-only` and `edit` run as `dontAsk` with tool deny lists. Cruise's workflow tools are exposed through the SDK.

## Startup OAuth expiration checks

Before agents start for `cruise plan`, `cruise run`, `cruise exec`, or TUI/WebUI session planning/execution, Cruise reads only credential files for the effective SDK candidates in that scope. Planning uses `plan_model` with `model` as its fallback. Execution includes workflow models and step overrides in ordinary steps, `after-pr`, groups, and parallel children. Model arrays, SDK-qualified entries, and effective `retry.fallback_chains` are included, even for candidates that may never run. There is no global check merely for opening the TUI/WebUI. Command backends, dry-runs, and `cruise workflow generate` are not checked; OpenCode is always skipped.

For the Claude SDK, Cruise reads `$CLAUDE_CONFIG_DIR/.credentials.json`, falling back to `~/.claude/.credentials.json` when unset. It examines `claudeAiOauth.expiresAt`, `refreshToken`, and optional `refreshTokenExpiresAt`. A non-empty value in the process environment or workflow-level `env` for any of `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`, or `CLAUDE_CODE_USE_FOUNDRY` skips this file check. This authentication is separate from jcode credentials. Sign in again with `claude auth login`, or for the corresponding jcode provider with `jcode login claude` / `jcode login openai`.

Exception: SDK title generation during foreground CLI approval, including `cruise plan --skip-planning`, uses only the inherited process environment and process working directory. Its expiration preflight therefore ignores workflow-level `env`, including `CLAUDE_CONFIG_DIR` and alternative-authentication variables. A workflow-only API key does not bypass this title-generation check; conclusively expired inherited OAuth credentials stop approval.

The check blocks only if access expiration is known and at or before the current time, and either no refresh token exists or its recorded expiration is also at or before the current time. An expired access token with a refresh token and no known refresh expiration passes without warning so the backend can refresh it. A still-valid access token passes even if the refresh token is expired. Unknown access expiration, missing or unreadable files, invalid JSON, and insufficient fields do not block startup. Timestamp values support milliseconds and defensive conversion of second-based values.

This is a read-only expiration preflight, not an authentication guarantee. It never runs auth-status CLIs, contacts providers, refreshes tokens, writes credentials, or takes credential locks. It cannot detect server-side refresh-token revocation and does not consult jcode's `auth-refresh-state.json`. Claude credentials stored only in macOS Keychain are not read, so a missing file is skipped. Step-level `env` API keys are not considered, and jcode bare-model provider resolution is left to jcode at runtime. The GitHub Action's `anthropic-api` / `openai-api` providers are outside the check.

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

The optional top-level `mcp_servers` map configures servers for all SDK prompt runs: workflow prompt steps (including parallel children), `after-pr` steps, planning turns, session-title generation, and PR-description generation. It is not a per-step setting, and a `workflow_call` callee's top-level MCP settings are ignored. Every effective applicable candidate, including SDK-qualified objects and authoritative retry-chain entries, must support the transport: stdio supports all SDKs, Streamable HTTP requires Claude or OpenCode, and legacy SSE requires Claude. Changing the workflow SDK does not override SDK-qualified objects. MCP servers cannot be used with the command backend.

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

For Claude, Cruise writes workflow entries to a per-run private JSON file with mode `0600` on Unix (an owner-only ACL on Windows) and passes the file path via `--mcp-config`; the SDK passes its in-process `cruise` tools separately as its own inline `--mcp-config` value. The user's own Claude MCP config remains enabled. `${VAR}` expansion for this backend is not guaranteed.


## Rate limits and fallback

OpenCode additionally uses structured V2 execution failures and retry barriers: local configuration/protocol failures, uncertain prompt acceptance, tool side effects, and unrecoverable event gaps are not automatically resent or switched to another model. Only the execution header `opencode model execution failed [provider.no-route]:` adds missing-model classification (including a `Model unavailable` body), after excluding HTTP 401/403/407, authentication/permission, and invalid-request failures. Generic unavailable text, other kinds, and appended stderr do not qualify. Existing rate-limit/server/network priority, retry budget, cooldown, visible-output checks, and replay barriers remain in force: a genuine no-route switches immediately to a configured usable fallback, never backs off on the same model. A server authentication failure or API-path 404 is not classified as a missing provider model. OpenCode provider wildcards preserve the full model ID and either `:effort` or `#variant`.

All SDK backends use policy mode when the top-level `retry:` block is present (see `top-level.md`) or when a workflow-level model array with fallback entries creates an implicit policy. In policy mode, HTTP 5xx, HTTP 4xx other than 429, 401, 403, 407, and 408, and network errors are retryable and the backoff uses `base_delay_ms` doubling to an 8s ceiling. Such a 4xx never retries the same model: it switches to the next chain entry under the same conditions as a 5xx, and what the failure says about the model decides the cooldown. A refused request leaves the left-behind model uncooled, unlike a 429/5xx/network skip, because the status describes the request; text naming the model as absent (`model_not_supported`, `model_not_found`, `model not found`, `unknown model`, `no such model`, `model does not exist`, `not a valid model`, read from the provider's own error rather than the appended CLI stderr) is classified as a missing model instead and does cool the left-behind model for the same 30 minutes, so later turns and steps stop selecting a model the provider does not have. A 400 qualifies as a switch even when the message reads permanent (`invalid request`, `context length`, `max_tokens`), since another model may accept the request — and `invalid_request_error: model not found` is read as a missing model, not as a permanent failure; a 401, 403, or 407 fails the step immediately, because no other model can answer an auth or permission failure; a 408 is classified as a network failure, so it takes the 5xx/network path — switch to a usable fallback first, same-model resend only when no fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0` — and its skipped model is cooled down like any other network skip; a failure carrying no status code (`unknown option '--effort'`) stays permanent and fails the step at once. A number in the failure text counts as an HTTP status only when it is a standalone three-digit number with `http`, `status`, `code`, `error`, `returned`, or `upstream` within the preceding 24 bytes, and source locations (`src/lib.rs:404:17`) and URL ports (`https://host:443/path`) never qualify; rate-limit wording, 5xx, and the network markers are decided before the missing-model wordings, which in turn are decided before any bare 4xx-looking number, so appended stderr noise cannot downgrade a 429 or a 503. Without either policy, SDK backends retry any provider failure classified as a limit—HTTP 429, `rate limit` / `too many requests` text, `usage limit`, `session limit`, or `overloaded`—against the same model with 2-second-doubling backoff (capped at 60s). The attempt budget is `--rate-limit-retries` either way; `--rate-limit-retries 0` means no retry and therefore no model switch, except for a jcode model reference that jcode refuses before startup (nothing was sent, so the next chain entry is tried immediately). An invalid OpenCode model reference fails before CLI launch without retry or fallback, regardless of the retry budget. Every retry starts a **fresh session** — resuming a partially-answered session would duplicate context — and a turn that already streamed visible text is never retried on another model. A model array with fallback entries always enables switching, even if `retry.model_fallback: false` is also present.
