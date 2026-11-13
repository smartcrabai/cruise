# SDK reference

**Languages:** [English](../en/sdk.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/sdk.md) · [简体中文](../zh/sdk.md) · [Index](README.md)

## SDK Mode

Instead of spawning an external CLI via `command`, prompt steps can be driven through an SDK backend by setting the top-level `sdk` field. `command` and `sdk` are mutually exclusive; the default `jcode` backend is used only when neither remains set in the resolved workflow after applicable `default.yaml` layers are applied. Three values are accepted: `jcode`, `claude`, and `opencode`.

### Startup OAuth expiration checks

Before starting agents for `cruise plan`, `cruise run`, or `cruise exec`, and for session planning/execution in the TUI or WebUI, Cruise checks local OAuth credential files for the applicable SDK candidates only. Planning uses the effective `plan_model` (falling back to `model`); execution includes workflow models and step overrides, including `after-pr`, groups, and parallel children. Model arrays, SDK-qualified entries, and effective retry fallback chains are included, even if a fallback is not ultimately used. This is not a global check at TUI/WebUI startup. Command backends, dry-runs, `cruise workflow generate`, and `cruise workflow make` are outside this check.

- **Claude SDK:** reads `$CLAUDE_CONFIG_DIR/.credentials.json`, or `~/.claude/.credentials.json` when unset, using `claudeAiOauth.expiresAt`, `refreshToken`, and optional `refreshTokenExpiresAt`.
- **Jcode SDK:** checks only explicit providers `claude`, `claude-oauth`, and `claude-subprocess` in the source home's `auth.json`, and `openai` and `openai-oauth` in `openai-auth.json`. The source home is the process `JCODE_HOME`, or `~/.jcode` when unset. With multiple accounts, it stops only if every account is conclusively expired.
- **Skipped:** OpenCode, jcode bare models with no provider, API-key providers such as `anthropic-api` / `openai-api`, and all other providers. Missing/unreadable files, invalid JSON, or insufficient expiration data pass without blocking. For Claude, a non-empty process or workflow-level `env` value for `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`, or `CLAUDE_CODE_USE_FOUNDRY` also skips the file check.

Exception: SDK title generation during foreground CLI approval, including `cruise plan --skip-planning`, uses only the inherited process environment and process working directory. Its expiration preflight therefore ignores workflow-level `env`, including `CLAUDE_CONFIG_DIR` and alternative-authentication variables. A workflow-only API key does not bypass this title-generation check; conclusively expired inherited OAuth credentials stop approval.

Cruise stops only when the access token has expired and the refresh token is absent or its recorded expiration is at or before the current time. An expired access token with a refresh token and no known refresh expiration passes without warning, leaving automatic refresh to the backend. The check only reads files: it does not invoke auth-status CLIs, contact providers, refresh tokens, write credentials, or take credential locks. To sign in again, use `claude auth login`, `jcode login claude`, or `jcode login openai` as appropriate.

This is an expiration preflight, not proof that authentication will succeed. It cannot detect server-side revocation of refresh tokens and does not read jcode refresh-failure state or macOS Keychain credentials. Step-level `env` API keys are not considered. To bypass a false positive, set the process environment variable `CRUISE_SKIP_AGENT_AUTH_CHECK=1`; only the exact value `1` opts out, not `true` or whitespace-padded values.

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```

In all SDK backends, `model` / `plan_model` / per-step `model` are **model references** with the same override precedence as command mode (step `model` > top-level `model` / `plan_model`). For jcode and Claude, the optional `:effort` suffix selects a reasoning-effort tier (`low` / `medium` / `high` / `xhigh` / `max`, plus the aliases `minimal` / `min` / `med` and the numeric spellings `1`..`4`); `off` / `none` / `0` / `5` are also consumed but leave the effort unset, and any other `:` suffix (an OpenRouter `:free` variant, say) stays part of the model id. For jcode/Claude, an effort a provider or model does not support is ignored. OpenCode instead validates effort against its catalog and fails explicitly when unsupported (see below).

At workflow level, `model` and `plan_model` may also be arrays. In SDK mode, the first entry is the primary model and later entries form an implicit fallback chain. A model array with fallback entries enables model fallback automatically; an explicit `retry.fallback_chains` entry for the primary model takes precedence over the array tail. A 429, a 5xx, an HTTP 4xx other than 429, 401, 403, 407, and 408, or a network failure switches immediately to a usable fallback when `--rate-limit-retries` is above zero and no visible text was streamed (a 429 is retried on the same model only when no usable fallback is left); such a 4xx is never resent to the same model, since an identical request would fail the same way. 401, 403, and 407 are authentication or permission failures no other model can answer, so they fail the step at once instead of walking the chain. A 408 is a transport timeout classified as a network failure, so it takes the same immediate-switch path as a 5xx: it moves to a usable fallback first, and is only resent to the same model when no usable fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0`. A model skipped after a 429, a 5xx, or a network failure (a 408 included) is cooled down for 30 minutes in the current process, and so is one whose provider reported it as missing (`400 model_not_supported`, `404 model_not_found`), because there the model rather than the request is the defect; only a model left behind by a plain client-error switch is not cooled, since that status describes the request, not the model's health. In command mode, only the first entry is used and the historical same-model retry behavior remains unchanged.

- `"provider/model[:effort]"` (e.g. `provider/model:xhigh`) -- `sdk: jcode` parses the provider and model separately; an empty side (`"/model"`, `"provider/"`) fails when the prompt runs. `sdk: claude` treats everything except the `:effort` suffix as the model name.
- `"model"` (no `/`) for jcode/Claude -- no explicit provider is supplied. Configured OpenCode references require a provider.
- `"provider/model[:effort]"` or `"provider/model#variant"` for `sdk: opencode`. Embedded `/` and unknown colon suffixes such as `:free` or `:2026` remain in the model ID. Recognized trailing effort suffixes are reserved and cannot be combined with `#variant`.
- Unset -- no explicit model is supplied. OpenCode uses its configured default. A jcode prompt fails with a setup
  error if it still has no effective model; Claude and `command:` retain their
  existing no-model behavior.

### Subagent model

Set `subagent_model` at workflow level or on a prompt step to select the model
for agents it spawns. A step value overrides the workflow value. It accepts a
single nonblank string or `{sdk: claude, model: haiku}` (also `sdk: jcode`), not
an array or fallback chain. Strings inherit the workflow SDK, defaulting to
jcode, not the SDK of a fallback attempt. Only attempts whose SDK matches the
resolved subagent SDK receive the setting; other attempts keep backend defaults.
Validation rejects a setting with no applicable model candidate.

```yaml
sdk: claude
model: sonnet
subagent_model: haiku
steps:
  review:
    prompt: Review the implementation, delegating checks as needed.
    subagent_model: {sdk: claude, model: sonnet}
```

- Claude sets `CLAUDE_CODE_SUBAGENT_MODEL` and
  `CLAUDE_CODE_SUBAGENT_MODEL_FORCE=1`, fixing all subagents including Explore
  and Plan. These injected values override same-named workflow/step `env:` keys.
  Without `subagent_model`, Cruise leaves those environment keys unchanged.
- jcode sets the session's swarm default before the prompt. A worker's explicit
  `model` still takes precedence, so this is a default override, not a force.
- OpenCode subagent models and `command:` backends are unsupported. A mixed-SDK
  workflow may target a Claude/jcode candidate with an explicit SDK object.
  Recognized `:effort` suffixes are rejected, even `:off`; unknown suffixes such
  as `:free` remain part of the model ID. jcode requires a valid nonempty model.
- Applies to workflow prompt steps, including groups, `after-pr`, parallel
  prompt children, and `github-review`, plus planning's plan/fix/ask turns.
  It does not apply to title generation, PR metadata, `workflow generate`, or `workflow make`.
  Non-prompt steps and group/workflow-call/parallel wrappers cannot set it.
- A called workflow's top-level value is ignored in favor of the caller's
  default; the callee's per-step overrides are retained.
- Layered `default.yaml` and `CRUISE_SUBAGENT_MODEL` support it, but application
  `config.json` / `config.yaml` defaults do not.

### `sdk: jcode` — the jcode SDK (default)

`sdk: jcode` uses the official jcode Rust SDK and requires jcode v0.88.0 or newer. At launch, Cruise validates the SDK handshake and required capabilities (`sessions`, plus `session_tools` when custom tools are registered); it does not probe the CLI version. Upgrade jcode when a required capability is missing. The `model`, `plan_model`, and per-step `model` fields accept `provider/model[:effort]` or a bare model. Every actual jcode prompt must resolve to a model: ordinary prompts use step `model` then workflow/default `model`; planning calls use `plan_model` then `model`. A configured jcode default, `JCODE_MODEL`, or resume session does not replace a missing Cruise model. Missing models create or preserve the user `default.yaml` template and stop that request. The effort suffix is applied through the SDK.

Each SDK run uses a private session home with inherited login records and copied
model catalogs. The runtime's `JCODE_HOME` points to the real session directory;
on Unix, a short symlink alias is used only for SDK socket paths. This satisfies
jcode's account-store safety checks without changing saved-session locations or
resumption.

#### Computer use

Workflow-level `computer_use` defaults to `false`. Set it to `true` to allow prompt turns to use jcode's `macos_computer_use` desktop-control tool on macOS. A prompt step may set `computer_use: true` or `false` to override the workflow default. Built-in plan, fix-plan, and ask-plan turns follow the workflow value; title and PR-description generation always run with computer use off. The tool is macOS-only and the setting has no effect on Linux. Every effective applicable model candidate must use jcode, including SDK-qualified objects and candidates selected by authoritative retry chains, and no `command:` backend may be set. Explicit jcode-only candidates can work under a Claude or OpenCode workflow SDK. Changing only the workflow SDK does not override incompatible SDK-qualified objects.

When computer use is off, Cruise adds `macos_computer_use` to `JCODE_DISABLED_TOOLS`, merging the first available list from workflow `env:`, the Cruise process environment, or `[tools].disabled` in the private copied `config.toml`. This reaches the private runtime's daemon, including swarm workers and subagents. When computer use is on with full permission and no explicit web/subagent denials, Cruise leaves `JCODE_DISABLED_TOOLS` unchanged and never removes a jcode-level disable to force-enable the tool. The user must grant macOS Accessibility and Screen Recording permissions.

## Permission modes

Workflow-level `permission` defaults to `full`. A prompt step may set `permission: read-only | edit | full` to override it. `full` keeps today's behavior. `edit` disables shell execution (Claude `Bash`, jcode `bash`) but keeps direct edit/write/patch tools. `read-only` also disables file mutation tools (Claude `Edit`/`Write`/`NotebookEdit`, jcode `edit`/`write`/`apply_patch`/`replace`). Restricted modes force `macos_computer_use` off even with `computer_use: true`, and Claude runs them with `dontAsk` so no permission prompt can block an unattended run. Jcode's mode-specific tools are merged into the selected `JCODE_DISABLED_TOOLS` list.

- `read-only` and `edit` are rejected with a `command:` backend, because cruise cannot control an arbitrary CLI's tools.
- `read-only` combined with `allow_commit: true` is a configuration error. `allow_commit: true` removes only the HEAD/ref commit guard and implicit Git denials, not explicit restrictions, and never widens the permission mode.
- A group call, `workflow_call`, or `parallel` wrapper cannot set `permission`. Set it on the inner prompt or parallel prompt child. A parallel block with any read-only child must contain only read-only prompt children.
- A `workflow_call` callee's top-level `permission` is ignored (the caller's default applies), while per-step overrides inside the callee are kept.
- A read-only step additionally fails with `ReadOnlyWorkspaceChanged` if the workspace snapshot changes during the step. This is change detection, not an OS sandbox or rollback, and it ignores `.git`, `target`, and `node_modules`.
- Built-in planning, fix/ask, title, PR-description, and workflow-generation turns always run with `full`, without `disallowed_tools` or implicit Git restrictions.

## Tool restrictions

`disallowed_tools` narrows SDK prompt capabilities without granting permissions:

```yaml
disallowed_tools: ["shell:npm publish"]
steps:
  review:
    prompt: Review without publishing or delegating.
    disallowed_tools: [web, subagent, "mcp:github", "shell:gh pr merge"]
```

| Value | Denied capability |
|-------|-------------------|
| `web` | Web fetch/search and backend browser tools where available |
| `subagent` | Subagent or swarm delegation |
| `mcp` | All workflow `mcp_servers` entries |
| `mcp:<name>` | One workflow MCP server; unknown names are errors |
| `shell:<prefix>` | Simple shell commands whose leading words match the prefix |

These are SDK-neutral values, not Claude `Bash(...)` syntax. Use `permission`
for blanket shell/edit restrictions; bare `shell` and `edit` are invalid.
Shell prefixes are trimmed and repeated whitespace is collapsed. They must be
nonempty and use only ASCII letters, digits, spaces, and `._/:=@+%-`.
Wildcards, quotes, commas, parentheses, shell operators, backslashes, newlines,
and control characters are rejected. A final word ending in `=` matches the
start of that word, for example `shell:git -c core.hooksPath=`.

The effective list is the deduplicated **union of workflow defaults, step
restrictions, and implicit Git rules**. A step's `[]` cannot undo workflow
restrictions. Across user/project `default.yaml` and workflow layers, the
highest-precedence explicit workflow-level list replaces lower lists;
workflow `disallowed_tools: []` clears inherited defaults. With `edit` or
`read-only`, shell rules are dropped because shell is already disabled.

Nonempty lists are prompt-only, including parallel prompt children, prompts
inside groups or expanded workflows, `after-pr`, and `github-review` loops.
Command/option steps, parallel wrappers, group and `workflow_call` call sites,
and native `squash-merge` reject them. Called workflows retain step lists but
their top-level list is ignored in favor of the caller's. A `command:` prompt
backend rejects explicit restrictions and does not enforce implicit shell
rules; its existing commit guard remains active.

Unless `allow_commit: true`, SDK prompts implicitly deny these shell prefixes:

```text
git commit                 git merge                  git rebase
git cherry-pick            git revert                 git am
git pull                   git push                   git reset
git switch                 git checkout -b            git checkout -B
git update-ref             git config core.hooksPath  git -c core.hooksPath=
```

This also blocks `git reset HEAD file`; use `git restore --staged` to unstage.
It does not block `git checkout -- file`, `git stash create`, `git branch`
listing, or `git status`/`diff`/`log`. Prefix matching cannot distinguish all
safe and unsafe Git operations, so commit guards remain the final defense.
Unlike the repository-scoped commit guard, shell prefix denials also apply to
matching commands targeting temporary or unrelated repositories.

Backend enforcement and limits:

- **Claude:** native deny rules map `web` to `WebFetch`/`WebSearch`, `subagent`
  to `Agent`, and shell prefixes to `Bash(...)`. Denies remain active with
  `bypassPermissions`. Native Claude matching does **not** normalize Git global
  options, so `git -C path commit` may not match `git commit`.
- **jcode:** merges `webfetch`, `websearch`, `browser`, and/or `swarm` into
  `JCODE_DISABLED_TOOLS`, preserving user entries. Shell rules use Cruise's
  pre-tool gate appended to existing hooks, selected from workflow env,
  process env, then config `[hooks].pre_tool`. `JCODE_HOOKS_DISABLED` in workflow
  or process env disables the gate and produces a warning. Gate setup failure
  also warns. The gate exits `2` to block, including malformed policy/input;
  hook execution failures or other exit codes are fail-open in jcode. Older
  releases may not support the array form needed to combine existing hooks.
- **OpenCode:** native deny rules plus the Cruise plugin/bridge deny web tools,
  `task` delegation, and matching bash input before execution.
- **jcode/OpenCode shell matching:** splits compound commands, removes leading
  assignments and common wrappers (`env`, `command`, `builtin`, `exec`, `nohup`,
  `nice`, `time`, `timeout`, `stdbuf`, `xargs`), examines `sh`/`bash`/`zsh`/`dash`
  `-c` scripts, compares executable basenames, and normalizes Git global options.
  Heredoc body lines can cause false-positive denials. This is not a complete
  shell interpreter or a security boundary.

MCP restrictions filter only workflow-provided entries before backend launch.
User-level backend MCP configuration remains enabled and unaffected, as do
Cruise's own session tools. These controls are defense in depth, not a sandbox;
the prompt warning, reference-transaction hook, and CAS rollback remain.

**Upgrade note:** On macOS, earlier Cruise versions exposed `macos_computer_use` to every jcode prompt implicitly. Set workflow-level `computer_use: true` to retain that behavior.

The GitHub Action's shell bootstrap retains `jcode version --json`, `jcode login`, `jcode provider add`, and observational `jcode auth status`: it has no SDK client, and the SDK exposes neither the CLI version string nor these account/profile setup operations. The Cruise backend itself uses the SDK capability handshake after launch.

Cruise resolves the process `JCODE_HOME` as its source home, falling back to `~/.jcode`. Each Cruise session gets an isolated SDK home that inherits credentials and config; resumed plan turns reuse it. The private runtime receives `JCODE_CHECK_UPDATES=0` without rewriting the copied config. Plan homes persist until their Cruise session is deleted or cleaned, while non-resumable prompt homes are removed after each turn. Unclaimed homes are pruned after 24 hours only when a cross-process lock proves no run is active. Before removing a private home, Cruise stops only a live `jcode serve` process whose command and `JCODE_HOME`/`JCODE_RUNTIME_DIR`/`JCODE_SOCKET` identify that runtime; it never trusts a registry PID alone. The absolute source-home path is saved with resumable plan sessions so deletion still works when Cruise is launched from another directory.
Model catalog caches are copied into that private home; path-bound external credentials are copied as owner-only files instead of linked, inheriting only trust decisions already recorded for the original paths.

`ask_user` keeps its pending question alive across jcode's 120-second SDK callback deadline. After about 110 seconds without an answer, the tool returns a successful continuation notice asking the model to call `ask_user` again with the identical question; it does not prompt the user again, and an answer received between calls is returned by the next call. The wait remains bounded by the workflow step timeout. When a saved plan conversation is absent from its private jcode home, Cruise starts a fresh SDK session because the current plan is already included in the prompt.

`submit_plan` and `update_plan` only write the plan and return; Cruise's existing approval flow happens after the model turn and does not hold an SDK tool callback open.

**Upgrade note:** After upgrading from an MCP-bridge release, remove the stale `mcpServers.cruise` entry from the source `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json` when `JCODE_HOME` is unset). Cruise strips it from each private session copy and leaves the source configuration unchanged. Until the entry is removed, jcode sessions outside Cruise will continue to launch it. Cruise exits with migration instructions on stderr instead of treating it as legacy plan input, so jcode reports the `cruise` MCP server as failed.

The backend exposes Cruise session tools through jcode's session-tools support.

### `sdk: claude` — the claude CLI in-process

`sdk: claude` drives the `claude` CLI in-process through claude-agent-sdk. Model references are plain `claude --model` names with the optional `:effort` suffix, forwarded as `--effort`. A `claude` CLI without that flag fails the step with `unknown option '--effort'`, which is classified permanent and never retried. Cruise is verified against 2.1.250. Authentication is the claude CLI's own, using its stored credentials or `ANTHROPIC_API_KEY`, unrelated to `jcode login`. The CLI runs with permissions bypassed because Cruise workflows are unattended, so there is no console to answer a permission prompt on. Cruise's workflow tools are exposed through the SDK. `computer_use: true` is rejected at config load because Claude Code's computer use is only available in interactive sessions and Cruise cannot enable it.

### Tool-less (non-interactive) planning

By default, SDK-mode planning uses session tools to ask questions and submit or revise the plan.

Set `interactive_planning: false` to turn that off. Planning then embeds the target plan-file path in the prompt and asks the agent to write `plan.md` directly — exactly like the `command` backend. The resulting `plan.md` is read back afterward (falling back to the agent's captured output if the file was not written).

```yaml
interactive_planning: false   # tool-less, file-based planning
```

`--grill` requires the interactive planning flow and is rejected when `interactive_planning` is off. The field has no effect in `command` mode, which is always file-based.

### `sdk: opencode` - the OpenCode V2 client

Cruise includes a Rust client for the workflow-required OpenCode V2 API, not a Python/Node SDK dependency or a provider-specific LLM client. Install the OpenCode CLI explicitly with `cruise install opencode` or separately, and authenticate it. Cruise owns a short-lived loopback `opencode serve --stdio` process and stops it after each attempt. Workflow execution does not connect to an external server, install the CLI, authenticate for you, or fall back to V1.

```yaml
sdk: opencode
model: "provider/model#variant" # Replace with a model in your OpenCode setup
plan_model: "provider/planning-model"
interactive_planning: true
permission: full
steps:
  implement:
    prompt: "{input}"
```

Use `provider/model[:effort]` or native `provider/model#variant` for workflow, planning, and step models, including array candidates and resumed sessions. Common effort names and aliases are case-insensitive: `low` (`min`/`minimal`/`1`), `medium` (`med`/`2`), `high` (`3`), `xhigh` (`4`), and `max`. Cruise validates the requested tier against OpenCode's live model catalog and selects a matching variant, never guesses or downgrades. Unsupported or ambiguous effort, an unlisted model, and tierless suffixes `:off`/`:none`/`:0`/`:5` fail explicitly before sending a prompt or changing a resumed model, without fallback. Common effort and native `#variant` are mutually exclusive. Native variants (including `#none`), suffixless models, and omitted models keep their existing behavior.

Model arrays use the existing failure-driven fallback policy. A genuine OpenCode `provider.no-route` execution failure (including `Model unavailable`) is treated as a missing model and switches immediately to a configured usable fallback with the existing budget and cooldown. Generic unavailable text, other failure kinds, appended stderr, and authentication/permission or invalid-request failures do not qualify for this special handling. Visible output and replay barriers still prevent switching. OpenCode forbids automatic retry or fallback after tool side effects, uncertain prompt acceptance, or unrecoverable event loss, even if no text was displayed. Omitted models are delegated to OpenCode's configured default. Workflow and step `env:` values reach prompt tools along with the inherited OS environment and commit guard. Existing OpenCode global/project configuration and authentication remain in use. Cruise keeps its own database, plugin, and resumable plan conversations in private XDG storage, not the project or the user's OpenCode database. It does not rewrite the user's configuration. Invalid `OPENCODE_CONFIG_CONTENT` is diagnosed rather than discarded.

Cruise planning, questions, title generation, and PR metadata use the existing tools through a native V2 plugin, not an MCP server. `computer_use: true` is explicitly unsupported. `permission: edit` allows direct file edits but denies shell, subagents, and unknown execution tools. `read-only` denies mutation too, while preserving safe Cruise tools and the workspace snapshot check. Unknown MCP and Code Mode execution paths are denied in restricted modes. These are tool restrictions, not an OS sandbox.

Workflow MCP entries support local `stdio` and remote Streamable HTTP (`type: http`). Legacy `type: sse` is rejected with a named-server diagnostic, never silently converted. No OAuth login is started during unattended runs. Workflow entries overlay OpenCode configuration without changing the source files.

The protocol baseline is OpenCode commit `74f8d4860f49c563909e6b039b0cc8b454ebce72` (investigated October 23, 2026). OpenCode v2.1.3 was identified as the first real-CLI verification target, not a blanket compatibility guarantee. Runtime V2 capability checks and fixture tests do not by themselves establish real-model or plugin interoperability.

See [the OpenCode workflow example](../../skills/cruise-config/examples/opencode-flow.yaml).

### MCP servers (`mcp_servers`)

The optional top-level `mcp_servers` map makes MCP servers available to every SDK prompt run: workflow prompt steps (including parallel children), `after-pr` steps, planning turns, session-title generation, and PR-description generation. It works with `sdk: jcode`, with the default `jcode` backend only when neither `sdk` nor `command` remains set in the resolved workflow after applicable `default.yaml` layers are applied, and with `sdk: claude` or `sdk: opencode`. Per-step overrides are not supported, but prompt `disallowed_tools: [mcp]` or `["mcp:<name>"]` can exclude workflow entries for a step; user-level MCP configuration is unaffected. A called workflow's top-level `mcp_servers` is ignored; the caller's settings remain in force.

Entries use the Claude Code `mcpServers` server shape. Omit `type` or set `type: stdio` for a local server. Every effective applicable candidate, including SDK-qualified objects and authoritative retry-chain entries, must support the transport: stdio supports all SDKs, HTTP requires Claude or OpenCode, and legacy SSE requires Claude. Changing the workflow SDK does not override SDK-qualified objects. The command backend cannot receive MCP servers:

```yaml
sdk: claude
mcp_servers:
  local_search:
    command: npx
    args: ["-y", "@example/search"]
    env:
      SEARCH_ROOT: ./docs
  remote_docs:
    type: http
    url: https://mcp.example.test/server
    headers:
      Authorization: "Bearer replace-with-your-token"
```

MCP entry values are passed through without Cruise `{variable}` template resolution. Names must be non-empty and contain only ASCII letters, digits, `_` or `-`; `cruise` is reserved. Stdio entries require a non-blank `command` and cannot set `url` or provide non-empty `headers`. `http` and `sse` entries require a URL beginning with `http://` or `https://` and cannot set `command` or provide non-empty `args` or `env`. Unknown entry fields are rejected. A non-empty `mcp_servers` map with the `command` backend is a validation error.

With jcode, Cruise merges the workflow entries into the private session copy of `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json` when `JCODE_HOME` is unset). Workflow entries replace same-named entries from that source file, and the source is never modified. jcode then applies its normal config merge order, so same-named servers from `~/.claude.json`, `~/.claude/mcp.json`, or project-local `.jcode/mcp.json`, `.mcp.json`, and `.claude/mcp.json` take precedence over the workflow copy. jcode expands `${VAR}` and `${VAR:-default}` using the runtime environment, including workflow-level `env:` values.

With `sdk: claude`, workflow MCP entries are written to a per-run private JSON file with mode `0600` on Unix (an owner-only ACL on Windows), and the file path is passed via `--mcp-config`; the SDK passes Cruise's in-process tools separately as its own inline `--mcp-config` value. The user's own Claude MCP configuration remains enabled. `${VAR}` expansion for these Claude entries is not guaranteed.
