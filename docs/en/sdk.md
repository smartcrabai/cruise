# SDK reference

**Languages:** [English](../en/sdk.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/sdk.md) · [简体中文](../zh/sdk.md) · [Index](README.md)

<a id="sdk-mode"></a>
## SDK Mode

Instead of spawning an external CLI via `command`, prompt steps can be driven through an SDK backend by setting the top-level `sdk` field. `command` and `sdk` are mutually exclusive; the default `jcode` backend is used only when neither remains set in the resolved workflow after applicable `default.yaml` layers are applied. Three values are accepted: `jcode`, `claude`, and `opencode`.

<a id="startup-oauth-expiration-checks"></a>
### Startup OAuth expiration checks

Cruise checks local OAuth credential files for the applicable SDKs before starting agents. It stops only when the access token has expired and the refresh token is absent or its recorded expiration is at or before the current time. To sign in again, use `claude auth login`, `jcode login claude`, or `jcode login openai` as appropriate.

This expiration preflight does not prove authentication will succeed. To bypass a false positive, set the process environment variable `CRUISE_SKIP_AGENT_AUTH_CHECK=1`; only the exact value `1` opts out.

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```

In all SDK backends, `model` / `plan_model` / per-step `model` are **model references** with the same override precedence as command mode (step `model` > top-level `model` / `plan_model`). For jcode and Claude, the optional `:effort` suffix selects a reasoning-effort tier (`low` / `medium` / `high` / `xhigh` / `max`, plus the aliases `minimal` / `min` / `med` and the numeric spellings `1`..`4`); `off` / `none` / `0` / `5` are also consumed but leave the effort unset, and any other `:` suffix (an OpenRouter `:free` variant, say) stays part of the model id. For jcode/Claude, an effort a provider or model does not support is ignored. OpenCode instead validates effort against its catalog and fails explicitly when unsupported (see below).

At workflow level, `model` and `plan_model` may be arrays. In SDK mode, the first entry is the primary model and later entries form an implicit fallback chain. Arrays with fallback entries enable model fallback automatically; an explicit `retry.fallback_chains` entry for the primary model takes precedence over the array tail.

- `"provider/model[:effort]"` (e.g. `provider/model:xhigh`) -- `sdk: jcode` parses the provider and model separately; an empty side (`"/model"`, `"provider/"`) fails when the prompt runs. `sdk: claude` treats everything except the `:effort` suffix as the model name.
- `"model"` (no `/`) for jcode/Claude -- no explicit provider is supplied. Configured OpenCode references require a provider.
- `"provider/model[:effort]"` or `"provider/model#variant"` for `sdk: opencode`. Embedded `/` and unknown colon suffixes such as `:free` or `:2026` remain in the model ID. Recognized trailing effort suffixes are reserved and cannot be combined with `#variant`.
- Unset -- no explicit model is supplied. OpenCode uses its configured default. A jcode prompt fails with a setup
  error if it still has no effective model; Claude and `command:` retain their
  existing no-model behavior.

<a id="subagent-model"></a>
### Subagent model

Set `subagent_model` at workflow level or on a prompt step to select the model
for spawned agents; a step value overrides the workflow value. It accepts a
single nonblank string or `{sdk: claude, model: haiku}` (also `sdk: jcode`),
not an array or fallback chain. Strings inherit the workflow SDK, defaulting to
jcode rather than a fallback attempt's SDK. Only attempts whose SDK matches the
resolved subagent SDK receive the setting; others keep backend defaults.
Validation rejects settings with no applicable model candidate.

```yaml
sdk: claude
model: sonnet
subagent_model: haiku
steps:
  review:
    prompt: Review the implementation, delegating checks as needed.
    subagent_model: {sdk: claude, model: sonnet}
```

- OpenCode subagent models and `command:` backends are unsupported. A mixed-SDK workflow may target a Claude/jcode candidate with an explicit SDK object. Recognized `:effort` suffixes are rejected, even `:off`; unknown suffixes such as `:free` remain part of the model ID. jcode requires a valid nonempty model.

<a id="sdk-jcode--the-jcode-sdk-default"></a>
### `sdk: jcode` — the jcode SDK (default)

`sdk: jcode` uses the official jcode Rust SDK and requires jcode v0.88.0 or newer. At launch, Cruise validates the SDK handshake and required capabilities (`sessions`, plus `session_tools` when custom tools are registered); it does not probe the CLI version. Upgrade jcode when a required capability is missing. The `model`, `plan_model`, and per-step `model` fields accept `provider/model[:effort]` or a bare model. Every actual jcode prompt must resolve to a model: ordinary prompts use step `model` then workflow/default `model`; planning calls use `plan_model` then `model`. A configured jcode default, `JCODE_MODEL`, or resume session does not replace a missing Cruise model. Missing models create or preserve the user `default.yaml` template and stop that request. The effort suffix is applied through the SDK.

<a id="computer-use"></a>
#### Computer use

Workflow-level `computer_use` defaults to `false`. Set it to `true` to allow prompt turns to use jcode's `macos_computer_use` desktop-control tool on macOS; a prompt step may override the workflow value. Built-in plan, fix-plan, and ask-plan turns follow the workflow value, while title and PR-description generation always run with computer use off. The tool is macOS-only and has no effect on Linux. Users must grant macOS Accessibility and Screen Recording permissions.

<a id="permission-modes"></a>
### Permission modes

Workflow-level `permission` defaults to `full`. A prompt step may set `permission: read-only | edit | full` to override it. `full` keeps today's behavior. `edit` disables shell execution (Claude `Bash`, jcode `bash`) but keeps direct edit/write/patch tools. `read-only` also disables file mutation tools (Claude `Edit`/`Write`/`NotebookEdit`, jcode `edit`/`write`/`apply_patch`/`replace`). Restricted modes force `macos_computer_use` off even with `computer_use: true`; Claude runs them with `dontAsk` so no permission prompt can block an unattended run. Jcode's mode-specific tools are merged into the selected `JCODE_DISABLED_TOOLS` list.

- `read-only` and `edit` are rejected with a `command:` backend, because Cruise cannot control an arbitrary CLI's tools.
- `read-only` with `allow_commit: true` is a configuration error. `allow_commit: true` removes only the HEAD/ref commit guard and implicit Git denials, not explicit restrictions, and never widens the permission mode.
- A group call, `workflow_call`, or `parallel` wrapper cannot set `permission`. Set it on the inner prompt or parallel prompt child. A parallel block with any read-only child must contain only read-only prompt children.
- A `workflow_call` callee's top-level `permission` is ignored (the caller's default applies), while its per-step overrides are kept.
- A read-only step fails with `ReadOnlyWorkspaceChanged` if the workspace snapshot changes during the step. This is change detection, not an OS sandbox or rollback, and it ignores `.git`, `target`, and `node_modules`.
- Built-in planning, fix/ask, title, PR-description, and workflow-generation turns always run with `full`, without `disallowed_tools` or implicit Git restrictions.

<a id="tool-restrictions"></a>
### Tool restrictions

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

MCP restrictions filter only workflow-provided entries before backend launch. User-level backend MCP configuration and Cruise's own session tools remain enabled. Claude shell-prefix denials do not normalize Git global options, so `git -C path commit` may not match `git commit`. In jcode, `JCODE_HOOKS_DISABLED` in workflow or process environment disables the shell gate, and hook execution failures are fail-open. These controls are defense in depth, not a sandbox; the prompt warning, reference-transaction hook, and CAS rollback remain.

<a id="sdk-claude--the-claude-cli-in-process"></a>
### `sdk: claude` — the claude CLI in-process

`sdk: claude` drives the `claude` CLI in-process through claude-agent-sdk. Model references are ordinary `claude --model` names with an optional `:effort` suffix forwarded as `--effort`. A CLI without that option fails with `unknown option '--effort'`, a permanent error that is never retried. Cruise is verified against 2.1.250. Authentication uses the claude CLI's stored credentials or `ANTHROPIC_API_KEY`, not `jcode login`. The CLI runs with permissions bypassed because workflows are unattended and no console can answer prompts. `computer_use: true` is rejected at config load because Claude Code computer use requires an interactive session.

<a id="tool-less-non-interactive-planning"></a>
### Tool-less (non-interactive) planning

By default, SDK-mode planning uses session tools to ask questions and submit or revise the plan.

Set `interactive_planning: false` to turn that off. Planning then embeds the target plan-file path in the prompt and asks the agent to write `plan.md` directly — exactly like the `command` backend. The resulting `plan.md` is read back afterward (falling back to the agent's captured output if the file was not written).

```yaml
interactive_planning: false   # tool-less, file-based planning
```

`--grill` requires the interactive planning flow and is rejected when `interactive_planning` is off. The field has no effect in `command` mode, which is always file-based.

<a id="sdk-opencode---the-opencode-v2-client"></a>
### `sdk: opencode` - the OpenCode V2 client

Install the OpenCode CLI with `cruise install opencode` or separately, then authenticate it.

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

Use `provider/model[:effort]` or native `provider/model#variant` for workflow, planning, and step models. Effort tiers include `low`, `medium`, `high`, `xhigh`, and `max`; an effort suffix and native `#variant` cannot be combined.

Omitting the model delegates to OpenCode's configured default. Workflow and step `env:` values reach prompt tools with the inherited OS environment and commit guard. Existing OpenCode global/project configuration and authentication remain in use.

Cruise planning, questions, title generation, and PR metadata use existing tools through a native V2 plugin, not an MCP server. `computer_use: true` is unsupported. `permission: edit` allows direct file edits but denies shell, subagents, and unknown execution tools; `read-only` also denies mutation while preserving safe Cruise tools and the workspace snapshot check. Unknown MCP and Code Mode execution paths are denied in restricted modes. These are tool restrictions, not an OS sandbox.

Workflow MCP entries support local `stdio` and remote Streamable HTTP (`type: http`). Legacy `type: sse` is rejected with a named-server diagnostic, never silently converted. No OAuth login is started during unattended runs. Workflow entries overlay OpenCode configuration without changing the source files.

See [the OpenCode workflow example](../../skills/cruise-config/examples/opencode-flow.yaml).

<a id="mcp-servers-mcp_servers"></a>
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
