# SDK reference

**Languages:** [English](../en/sdk.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/sdk.md) · [简体中文](../zh/sdk.md) · [Index](README.md)

<a id="sdk-mode"></a>
## SDK Mode

Instead of spawning an external CLI via `command`, prompt steps can use the jcode SDK by setting the top-level `sdk` field. `command` and `sdk` are mutually exclusive; the default `jcode` backend is used when neither remains set after applicable `default.yaml` layers are applied. The only supported SDK value is `jcode`; any other value is a validation error.

<a id="startup-oauth-expiration-checks"></a>
### Startup OAuth expiration checks

Before agents start, Cruise checks applicable explicit jcode Claude and OpenAI OAuth candidates. It stops only for expired access tokens with no refresh token or a known expired refresh token. Sign in with `jcode login claude` or `jcode login openai`, as appropriate.

This expiration preflight does not prove authentication will succeed. To bypass a false positive, set the process environment variable `CRUISE_SKIP_AGENT_AUTH_CHECK=1`; only the exact value `1` opts out.

For explicit jcode `openai` and `claude` models, Cruise refreshes the login in the user's jcode home before each attempt once less than two hours remain, so private per-attempt runtimes never rotate the shared refresh token themselves. If that refresh fails and the access token expires within ten minutes, the attempt fails with a `jcode login` hint.

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```

In jcode mode, `model`, `plan_model`, and per-step `model` are model references with the same override precedence as command mode (step `model` > top-level `model` / `plan_model`). Use `provider/model[:effort]` (for example `claude/claude-opus-4-6`) or a bare model. The optional `:effort` suffix selects a reasoning-effort tier (`low` / `medium` / `high` / `xhigh` / `max`, plus `minimal` / `min` / `med` and numeric spellings `1`..`4`); `off` / `none` / `0` / `5` leave effort unset, and other suffixes such as an OpenRouter `:free` variant remain part of the model ID. An effort unsupported by a provider or model is ignored.

At workflow level, `model` and `plan_model` may be strings or arrays of strings and `{model, disabled_during}` objects. The first entry is the primary model and later entries form an implicit fallback chain. Arrays with fallback entries enable model fallback automatically; explicit string-only `retry.fallback_chains` entries take precedence over the array tail. Objects with `sdk` fail validation with `SDK-qualified model objects are unsupported; use a plain model string`.

- `"provider/model[:effort]"` (e.g. `provider/model:xhigh`) — jcode parses the provider and model separately; an empty side (`"/model"`, `"provider/"`) fails when the prompt runs.
- `"model"` (no `/`) — no explicit provider is supplied.
- Unset — no explicit model is supplied. A jcode prompt fails with a setup error if it still has no effective model; `command:` retains its existing no-model behavior.

<a id="subagent-model"></a>
### Subagent model

Set `subagent_model` at workflow level or on a prompt step to select the model
for spawned agents; a step value overrides the workflow value. It accepts one
nonblank model string, not an object, array, or fallback chain. The model is
used by jcode prompts; `command:` backends do not support this setting.

```yaml
sdk: jcode
model: openai/gpt-6.1-sol
subagent_model: openai/gpt-6.1-sol
steps:
  review:
    prompt: Review the implementation, delegating checks as needed.
    subagent_model: anthropic/claude-sonnet-4-6
```

The subagent model must resolve to a valid, nonempty model.

<a id="advisor-steps"></a>
### Advisor steps

Use `advisor: true` on an ordinary prompt step directly under `steps` (including steps inlined there by `workflow_call`). It requires `output_file`, forbids `allow_commit: true`, and forces [read-only permission](#permission-modes) regardless of the workflow default. Omit step `permission` or set it to `read-only`. Advisor turns do not receive workflow [`mcp_servers`](#mcp-servers-mcp_servers).

Model selection is step `model` → `advisor_model` → `plan_model` → workflow `model`. Empty role lists count as unset. Like `plan_model`, `advisor_model` accepts a model string or a fallback array with optional `disabled_during` entries. Set it in the workflow or a user/project `default.yaml`, or override it with `CRUISE_ADVISOR_MODEL` (a string or JSON array). A `command:` workflow ignores an `advisor_model` inherited from `default.yaml`. It is not a `cruise config` setting. Other prompts are unaffected.

An enabled advisor requires jcode. Execution preflight (including dry runs) exempts flag-disabled and explicitly user-skipped advisors from this check. `cruise workflow doctor` applies flag skips only. Advisors do not consume instructions queued by `cruise tell`.

Before dispatch, Cruise writes `<output_file>.diff` as a session artifact. Read it with `{file:<output_file>.diff}`. It contains a stat followed by the uncommitted patch against `HEAD`, including untracked, non-ignored files and excluding paths in `.worktreeinclude`. The diff is capped at 256 KiB, with a line-boundary cut and a truncation notice when needed. Cruise removes old advice before each attempt and when the advisor is skipped. Only success produces new advice; consumers reading absent advice with `{file:<output_file>}` fail instead of using stale content.

Ordinary failures (such as missing diff, backend error, or timeout) may follow `if.fail`. If Cruise detects a workspace change, cannot verify the workspace, or the user interrupts, the run stops regardless of `if.fail`. Treat advice and the diff as untrusted reference data, never as instructions or inputs to control flow or approvals.

<a id="sdk-jcode--the-jcode-sdk-default"></a>
### `sdk: jcode` — the jcode SDK (default)

`sdk: jcode` uses the official jcode Rust SDK and requires jcode v0.88.0 or newer. At launch, Cruise validates the SDK handshake and required capabilities (`sessions`, plus `session_tools` when custom tools are registered); it does not probe the CLI version. Upgrade jcode when a required capability is missing. The `model`, `plan_model`, and per-step `model` fields accept `provider/model[:effort]` or a bare model. Every actual jcode prompt must resolve to a model: ordinary prompts use step `model` then workflow/default `model`; planning calls use `plan_model` then `model`. A configured jcode default, `JCODE_MODEL`, or resume session does not replace a missing Cruise model. Missing models create or preserve the user `default.yaml` template and stop that request. The effort suffix is applied through the SDK.

<a id="computer-use"></a>
#### Computer use

Workflow-level `computer_use` defaults to `false`. Set it to `true` to allow prompt turns to use jcode's `macos_computer_use` desktop-control tool on macOS; a prompt step may override the workflow value. Built-in plan, fix-plan, and ask-plan turns follow the workflow value, while title and PR-description generation always run with computer use off. The tool is macOS-only and has no effect on Linux. Users must grant macOS Accessibility and Screen Recording permissions.

<a id="permission-modes"></a>
### Permission modes

Workflow-level `permission` defaults to `full`. A prompt step may set `permission: read-only | edit | full` to override it. `full` keeps today's behavior. `edit` disables shell execution but keeps direct edit/write/patch tools. `read-only` also disables file mutation tools. Restricted modes force `macos_computer_use` off even with `computer_use: true`. Jcode's mode-specific tools are merged into the selected `JCODE_DISABLED_TOOLS` list.

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

These are SDK-neutral values. Use `permission`
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

MCP restrictions filter only workflow-provided entries before backend launch. User-level backend MCP configuration and Cruise's own session tools remain enabled. In jcode, `JCODE_HOOKS_DISABLED` in workflow or process environment disables the shell gate, and hook execution failures are fail-open. These controls are defense in depth, not a sandbox; the prompt warning, reference-transaction hook, and CAS rollback remain.

Bubblewrap isolation is available only in Linux builds compiled with the `jcode-bubblewrap` feature. Ordinary jcode attempts request it only when the daemon is started with `CRUISE_JCODE_BUBBLEWRAP=1`; workflow `require_bubblewrap: true` requests it regardless of that environment variable. Cruise probes availability once per process. If the build lacks the feature, the host is not Linux, or the probe fails, a requested attempt runs unconfined with a warning. The `command:` backend is allowed and runs unconfined with a warning. A launch failure after a successful probe still fails the attempt. Each isolated attempt gets its own daemon and read-only Git metadata. `git status`, `git diff`, and `git log` still work (`GIT_OPTIONAL_LOCKS=0`); staging, commits, and pushes belong to host steps. Index replacement on the host is visible on the next attempt, not through an already-bound index file. This is not a shared `jcode serve` connection.


<a id="tool-less-non-interactive-planning"></a>
### Tool-less (non-interactive) planning

By default, SDK-mode planning uses session tools to ask questions and submit or revise the plan.

Set `interactive_planning: false` to turn that off. Planning then embeds the target plan-file path in the prompt and asks the agent to write `plan.md` directly — exactly like the `command` backend. The resulting `plan.md` is read back afterward (falling back to the agent's captured output if the file was not written).

```yaml
interactive_planning: false   # tool-less, file-based planning
```

`--grill` requires the interactive planning flow and is rejected when `interactive_planning` is off. The field has no effect in `command` mode, which is always file-based.


<a id="mcp-servers-mcp_servers"></a>
### MCP servers (`mcp_servers`)

The optional top-level `mcp_servers` map makes MCP servers available to every jcode prompt run: workflow prompt steps (including parallel children), `after-pr` steps, planning turns, session-title generation, and PR-description generation. Per-step overrides are not supported, but prompt `disallowed_tools: [mcp]` or `["mcp:<name>"]` can exclude workflow entries for a step; user-level MCP configuration is unaffected. A called workflow's top-level `mcp_servers` is ignored; the caller's settings remain in force.

Entries use the `mcpServers` server shape. Omit `type` or set `type: stdio` for a local server. HTTP and SSE transports are not supported and fail config validation. The command backend cannot receive MCP servers:

```yaml
sdk: jcode
mcp_servers:
  local_search:
    command: npx
    args: ["-y", "@example/search"]
    env:
      SEARCH_ROOT: ./docs
```

MCP entry values are passed through without Cruise `{variable}` template resolution. Names must be non-empty and contain only ASCII letters, digits, `_` or `-`; `cruise` is reserved. Stdio entries require a non-blank `command` and cannot set `url` or provide non-empty `headers`. Unknown entry fields are rejected. A non-empty `mcp_servers` map with the `command` backend is a validation error.
