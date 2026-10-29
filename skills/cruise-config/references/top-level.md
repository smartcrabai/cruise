# Top-level structure

`inputs: [task, label]` optionally declares a called workflow's required string
inputs. Names must be unique and match `[A-Za-z_][A-Za-z0-9_]*`. There are no
defaults, optional inputs, or CLI input flags. A root loaded directly cannot
declare nonempty inputs because it has no caller. `inputs` is workflow-only and
is rejected in user/project `default.yaml`. Callers supply the exact declared
names through `workflow_call` plus `with`; see [steps.md](steps.md#workflow-call-inputs).

`model` and `plan_model` accept a single string or an ordered array mixing strings and `{sdk: claude, model: claude-sonnet-5-5:medium}` objects. Strings inherit the effective workflow SDK, defaulting to jcode. Objects override only their own candidate when `sdk` is present, otherwise they inherit that workflow SDK too. This also works in application `config.json` / `config.yaml` and layered `default.yaml`; `planModel` is an application-config alias. Objects are array-only, require a nonblank `model` and at least one of `sdk` or nonempty `disabled_during`, reject unknown keys and explicit null SDKs, and cannot coexist with `command` anywhere in either list. String-only command arrays retain first-entry behavior. Environment model overrides replace the whole list. See `sdk.md` for fallback precedence, SDK-owned resume IDs, capability validation across candidates, and full window semantics.

`disabled_during` is a nonempty list of user-declared unavailable windows on `model` / `plan_model` array objects, for example `{model: provider/main, disabled_during: ["mon-fri 01:00-04:00"]}`. Grammar is `[days ]HH:MM-HH:MM[ offset]`: case-insensitive `mon`..`sun`, comma lists and inclusive wrapping ranges, daily when omitted. UTC is the default; `UTC`, `Z`, and fixed offsets up to `+14:00` / `-14:00` are supported, not IANA zones, local time, or DST. Windows are start-inclusive/end-exclusive; `24:00` is end-only, equal times are invalid, and `00:00-24:00` means all day. Overnight windows use their start weekday. These declarations exclude candidates independently of cooldown and retry budget, not by inspecting prices. They are checked at prompt startup and fallback switches, never by interrupting a stream; no usable candidates means an error. Matching SDK/base-model identities in retry chains share exclusions. A step model override inherits the workflow list and its windows only when its resolved SDK and base model both match the primary candidate; other overrides do not. `subagent_model`, step window fields, and windows on string-only retry-chain entries are unsupported; capability checks still include disabled candidates.

```yaml
command:                  # LLM invocation command (array). Mutually exclusive with `sdk`.
  - claude
  - --model
  - "{model}"
  - -p

# sdk: jcode              # Alternative backend: use jcode-sdk (the fallback
                          # when neither `command` nor `sdk` remains set in the
                          # resolved workflow after applicable `default.yaml` layers; see sdk.md).
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
# subagent_model: haiku   # SDK-only: one string or {sdk: claude, model: haiku}; no effort
interactive_planning: true # Optional: enable SDK plan tools (default: true)
permission: full           # Optional: prompt permission read-only | edit | full (default: full)
# disallowed_tools: [web, "shell:npm publish"] # Optional: SDK prompt deny list
computer_use: false       # Optional: enable jcode's macOS computer-use tool (default: false)
languages:                # Optional: prompt languages; defaults to English
  pr: English             # Language for auto-generated PR title/body
  plan: English           # Language for built-in planning prompts

env:                      # Optional: environment variables applied to every step
  API_KEY: sk-...

# Optional MCP servers for every SDK prompt run (not supported with command:). On Windows, command: steps run via cmd.exe /C.
# mcp_servers:
#   local_search:
#     command: npx
#     args: ["-y", "@example/search"]

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
create_pr: true           # Optional, workflow-only: automatically commit/push and create a PR (default: true)
force_exec: false         # Optional: execute direct plan entry points in place (default: false)

```
`steps` is required. Setting both `command` and `sdk` is a validation error (an empty `command` array counts as "not set"); the `jcode` backend is used only when neither `command` nor `sdk` remains set in the resolved workflow after applicable `default.yaml` layers are applied. When `sdk` is set it must be `jcode`, `claude`, or `opencode` — any other value is a validation error. `steps` is held as an `IndexMap`, so declaration order is the execution order. When a group's `if.file-changed` target is outside the group, its `max_retries` requires one additional global loop-protection budget unit; the group example above uses `max_retries: 3` with a top-level `max_retries: 4`.

Unknown top-level keys are silently ignored, so obsolete fields from older
configs (for example `worktree:` or `state:`) still load without error. Step
configuration, `retry:` blocks, MCP server entries, and SDK-qualified objects
inside model arrays reject unknown fields. A typo at the workflow level is
accepted and has no effect, while the same typo inside these strict structures
fails the load (see [steps.md](steps.md)).

## Workflow defaults (`default.yaml`)

A `default.yaml` may set only `command`, `sdk`, `model`, `plan_model`, `subagent_model`,
`max_retries`, `interactive_planning`, `languages`, `cleanup_after_pr`,
`force_exec`, `computer_use`, `permission`, `disallowed_tools`, `env`, `retry`, `mcp_servers`, and
`pre_steps`. Unknown keys and workflow-only keys (`description`, `steps`,
`groups`, `after-pr`, and `create_pr`) are rejected.

`pre_steps` is a map from a name to a command step with `command` and optional
`env`, `timeout`, `skip`, and `when` fields. Defaults insert these steps before
workflow steps as `pre/<name>`. Names must be non-empty and must not contain
`/`; a generated `pre/<name>` that collides with a workflow step is rejected.

User defaults are applied before project defaults, so project values take
precedence. `command` and `sdk` are replaced together when either is set;
`env` and `mcp_servers` merge by key, with the higher-precedence value winning.
Other defaulted fields fill only values the workflow leaves unset, so explicit
workflow values take precedence. Environment overrides are applied afterward.

If defaults specify a different backend than the workflow, filling an unset
`subagent_model` is a backend collision, just like `model` or `plan_model`.
Set it explicitly in the workflow or remove it from those defaults. This check
runs before `CRUISE_SDK` overrides.

`disallowed_tools` lists replace across default layers rather than merge.
Explicit workflow `disallowed_tools: []` clears inherited defaults, but a step
list is unioned with the effective workflow list and cannot relax it. Values
are `web`, `subagent`, `mcp`, `mcp:<name>`, and `shell:<prefix>`; see
[steps.md](steps.md) for syntax, implicit Git rules, and prompt-only positions.
Backend-kind mismatch validation also covers defaults that would fill an unset
`disallowed_tools`, even an empty list. Set it explicitly in the workflow or
remove the mismatched default. Called workflows ignore their top-level list
and preserve their step lists under the caller's default.

Application settings in `$XDG_CONFIG_HOME/cruise/config.json` or `config.yaml`
may also set `model` and `plan_model` (alias `planModel`) as a string or an
ordered primary-plus-fallback array. JSON takes precedence when both files
exist; otherwise an existing YAML file is used. Settings updates preserve the
selected format; when neither file exists, they create JSON. Invalid selected
files fail rather than falling back to the other format.
These model defaults have lower precedence than either `default.yaml` layer
or the workflow; environment overrides still win, and step models override
the effective ordinary model. SDK mode derives fallback chains from these
arrays just as for workflow arrays; command mode uses only the first entry.
If no layer sets `plan_model`, planning uses the effective `model`.
File-backed sessions reload these settings; saved snapshots retain the
models and fallback policy resolved when they were created.

For an actual jcode prompt, the resolved `model` must identify a model. If it
does not, Cruise stops before starting jcode and creates the user-level
`$XDG_CONFIG_HOME/cruise/default.yaml` template when no file exists. The
template contains `model: null`; replace it with an available
`provider/model[:effort]` reference or bare model. Cruise never creates a
project defaults file, and it does not auto-create this template during draft,
dry-run, workflow discovery, or non-jcode execution. See [SDK backends](sdk.md)
for model precedence and recovery details.

OpenCode accepts `provider/model[:effort]` or native `provider/model#variant`, never both suffixes together. Common tiers (`low`/`medium`/`high`/`xhigh`/`max`) and aliases (`min`/`minimal`/`1`, `med`/`2`, `3`, `4`) are case-insensitive, reserved trailing suffixes. Cruise validates effort against the live model catalog; unsupported/ambiguous effort, an unlisted model, or tierless `:off`/`:none`/`:0`/`:5` fails explicitly without fallback, not by guessing or downgrading. Unknown suffixes such as `:free`/`:2026` remain in the ID. Native variants and omitted models retain existing behavior, including OpenCode's configured default. This applies to workflow, planning, step, array, and resumed model selections. See [SDK details](sdk.md) and [the OpenCode example](../examples/opencode-flow.yaml).

## `command` vs `sdk`

There are four prompt-execution backends:

- `command:` — spawn an external CLI (e.g. `claude -p`) and write the prompt to its stdin.
- `sdk: jcode` — use the jcode SDK; the fallback only when neither `command` nor `sdk` remains set in the resolved workflow after applicable `default.yaml` layers are applied. Requires jcode v0.88.0 or newer; Cruise validates the SDK handshake and required capabilities when it starts. `model` / `plan_model` / per-step `model` accept `"provider/model[:effort]"` or a bare model reference. See [sdk.md](sdk.md) for details.
- `sdk: claude` — drive the `claude` CLI in-process via claude-agent-sdk. Model references are plain `claude --model` names with an optional `:effort` suffix; authentication is the claude CLI's own. See [sdk.md](sdk.md) for details.

- `sdk: opencode` - use the built-in Rust V2 client with a locally managed OpenCode CLI. Install and authenticate that CLI separately. Models use `provider/model[:effort]` with catalog validation or native `provider/model#variant`; no V1 fallback or external server connection is provided. `computer_use: true` is unsupported. See [sdk.md](sdk.md).

## `mcp_servers`

`mcp_servers` is an optional top-level map of MCP server names to Claude Code server entries. It applies to every SDK prompt run, including workflow and `after-pr` steps, planning turns, session-title generation, and PR-description generation. It is not a per-step setting, but prompt `disallowed_tools: [mcp]` or `["mcp:<name>"]` can exclude workflow entries. User-level backend MCP configuration and Cruise's own tools are unaffected. A called workflow's own top-level value is ignored in favor of the caller's value.

Stdio entries omit `type` or set `type: stdio`, and require a non-blank `command`. HTTP and SSE entries use `type: http` or `type: sse` and require an `http://` or `https://` URL. Every effective applicable candidate, including SDK-qualified objects and authoritative retry-chain entries, must support the transport: stdio supports all SDKs, Streamable HTTP requires Claude or OpenCode, and legacy SSE requires Claude. Changing the workflow SDK does not override SDK-qualified objects. Stdio entries cannot set `url` or provide non-empty `headers`; remote entries cannot set `command` or provide non-empty `args` or `env`. Names must use only ASCII letters, digits, `_`, or `-`, and `cruise` is reserved. Unknown entry keys are rejected. A non-empty map is invalid with `command:`.

Values are passed verbatim without Cruise `{variable}` substitution. jcode writes workflow servers into the private session copy of `$JCODE_HOME/mcp.json`, preserving the source and preferring workflow entries over same-named entries in that source file. jcode's later Claude Code and project-local config layers can override them; `${VAR}` and `${VAR:-default}` are expanded from the runtime environment, including workflow `env:` values. Claude receives workflow entries through a per-run private JSON file with mode `0600` on Unix (an owner-only ACL on Windows), passed by path via `--mcp-config`; Cruise's own in-process tools are passed separately as the SDK's inline `--mcp-config` value, and the user's Claude MCP config remains enabled. `${VAR}` expansion for the Claude backend is not guaranteed.

`computer_use` defaults to `false` and is supported only with the `jcode`
backend. When enabled, it allows prompt turns to use jcode's macOS computer-use
tool; a prompt step can override the workflow setting (see [steps.md](steps.md)).
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
model wins over its array tail. Rate limits switch immediately to the next usable fallback
and retry the current model only when none exists. 5xx, HTTP 4xx other than 429, 401, 403, 407, and 408, and
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

## `subagent_model`

Optional default model for spawned agents on Claude/jcode prompt steps and
planning's plan/fix/ask turns. Accepts one nonblank string or an object such as
`{sdk: claude, model: haiku}`, never an array or fallback chain. Strings resolve
against the workflow SDK (jcode when omitted); objects target only their named
SDK's attempts. A step value overrides this default, and the resolved SDK must
have an applicable model candidate. Recognized `:effort` suffixes, OpenCode
subagent models, and `command:` backends are rejected.

Layered `default.yaml` supports this field; application model defaults and a
`CRUISE_SUBAGENT_MODEL` environment override do not. Title generation, PR
metadata, and `workflow generate` do not use it. A called workflow's top-level
value is ignored, but its step overrides survive. See [sdk.md](sdk.md) for
Claude's forced model, jcode's swarm default, and fallback behavior.

## `description`

Free-form text shown alongside the file name in the CLI/WebUI config selectors. Purely informational; no effect on execution.

```yaml
description: Full TDD flow with review loop
```

## `flags`

Declares named on/off switches that gate steps. Names contain only ASCII letters, digits, `_` and `-`. Each entry may have a `description` for interactive flag selectors. Every declared flag is off when absent. Enable one with `--flag NAME`, enable a comma-separated selection with `--flag NAME1,NAME2`, or repeat `--flag`; explicit `NAME=on|off|true|false` assignments set or clear individual values. Every declared flag must be referenced by at least one step, and a step cannot reference an undeclared flag. On a gate, `flag: name` remains the compatible single-name form; `flag: [name_a, name_b]` requires a non-empty list and enables the step if **any** listed flag is on.

```yaml
flags:
  deploy:
    description: Run the deploy steps

steps:
  build:
    command: make build
  deploy:
    command: make deploy
    flag: deploy        # skipped when deploy=off
```

`flag:` gates use the compatible single-name string or a non-empty list with ANY/OR semantics. A gate is permitted on a step, group or `workflow_call` call site, or parallel parent; a call-site gate is applied to every expanded step. It is not permitted on parallel children, group definitions' steps, or a callee's own steps. See [flow-control.md](flow-control.md) and [steps.md](steps.md).

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

- Does not apply to sessions started from an embedded built-in workflow.
- The current step must still exist in the new config.

## `cleanup_after_pr`

When set to `true`, cruise deletes the local git worktree and its branch after the PR has been created successfully.

```yaml
cleanup_after_pr: true   # remove worktree + branch once the PR is open
```

- Has no effect in **current-branch mode** (no worktree exists to remove).
- Has no effect for **`--repo` sessions** when PR creation occurs (the clone is removed after PR creation regardless of this flag).
- For `--repo` sessions, post-PR clone/worktree cleanup errors are silently ignored and the removed-clone notice is printed unconditionally; only non-repo worktree cleanup errors are downgraded to warnings. The session is still marked `Completed`.
- Override per-run with `--cleanup-after-pr` / `--no-cleanup-after-pr` CLI flags (takes precedence over config and session-level setting).

## `create_pr`

`create_pr` is a workflow-only boolean that defaults to `true`. Setting it to
`false` skips Cruise's automatic commit, push, PR creation, `after-pr`, and
PR-based cleanup, while retaining the normal session, artifacts, and worktree.
It is useful for persistent no-PR workflows such as research; a local
worktree does not need `gh`/`glab` when PR creation is disabled, but `--repo`
still needs the appropriate forge CLI to access and clone the repository.
When `after_pr` or `merge` was opted in but no PR can be created, Cruise skips
those unavailable actions and prints the reason. `merge` never overrides
`create_pr: false` or a read-only workflow such as `research`.

```yaml
create_pr: false
```

With PR creation enabled, a successful workflow that leaves no new branch
commits completes without attempting PR creation. Existing branch commits
remain eligible for a PR even if the current run made no edits. If no PR is
created, opted-in after-PR actions are skipped with a reason and no merge is
attempted. Explicit `if.no-file-changes: retry|failed` outcomes still apply;
disabling PR creation does not suppress a configured no-file-change failure or
retry.

`create_pr` is rejected in user and project `default.yaml` files and is not a
CLI option.


See [after-pr.md](after-pr.md) for steps that run after PR creation.

## `force_exec`

When `true`, direct plan entry points use the same current-directory execution
path as `cruise exec`: no planning, worktree, or PR. It applies to
`cruise "<input>"`, `cruise plan "<input>"`, and `cruise --plan "<input>"`.

```yaml
force_exec: true
```

Use `--no-force-exec`, `--repo`, `--grill`, or image attachments to keep normal
planning. `--skip-planning` and `--no-interactive-planning` do not disable direct execution. Background `--plan`
runs foreground because no plan worker is needed. `force_exec` never creates a
PR or merges; if `after_pr` or `merge` was opted in, unavailable after-PR
actions are skipped with a reason.

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
does the same, with no `Retry-After` wait, and retries the current model only
when no usable fallback exists. A retryable 4xx only
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
      - provider-a/model-a
      - provider-b/model-b
```

Chain entries are `"provider/model"`, `"provider/*"` (keeps the failing model id, swaps only the provider), or a bare model name. A switched-to model gets a fresh retry budget and no delay; every retry starts a fresh session; a turn that already streamed visible text is never retried on another model; a model skipped because of a 429, a 5xx, a network failure, or an error naming it as missing remains skipped for 30 minutes in this process (in-memory, not persisted across processes), while a client-error switch keeps its model immediately selectable. The attempt budget stays `--rate-limit-retries` — `retry:` adds no second count. Top-level `max_retries` is unrelated: it is the graph loop-protection ceiling, not a retry budget for prompts.
