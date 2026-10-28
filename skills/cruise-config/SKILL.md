---
name: cruise-config
description: Use when creating or editing a cruise YAML config file (cruise.yaml / .cruise.yaml). Covers inline and file-backed prompt steps (`prompt` / `prompt_file`), parallel prompt/command blocks, variables, flow control, groups, after-pr, and validation rules — full spec is split across reference docs.
---

cruise is a workflow orchestrator that drives coding agent CLIs like `claude -p` via a YAML config. This skill documents the config file format.

Workflow composition keeps string `workflow_call` references and supports required
callee `inputs` with string-valued caller `with` mappings. See
[steps.md](references/steps.md#workflow-call-inputs) for complete caller/callee
examples and [variables.md](references/variables.md#load-time-workflow-inputs)
for lexical scope, explicit forwarding, and preserved runtime escapes.

## When to use

- Creating a new `cruise.yaml` / `.cruise.yaml` / `cruise.yml` / `.cruise.yml`
- Adding or editing steps in an existing cruise config
- Designing a workflow (plan → implement → test → review → PR) in YAML
- Looking up field names, variable names, or validation rules

## Config file resolution

Config files are resolved in this priority order:

1. `-c/--config <path>` flag (highest priority; never prompts). `-c __builtin__` and `-c builtin:tdd` select the built-in `tdd` workflow; `-c builtin:<name>` selects a named catalog workflow. The former selectors `builtin:default`, `builtin:simple`, and `builtin:review` are unsupported. Use `cruise workflow list` to see the catalog and `cruise workflow eject <name> [--to user|project]` to copy a built-in for editing.
2. `CRUISE_CONFIG` environment variable (error if the file does not exist; never prompts)
3. Current directory: `./cruise.yaml` → `./cruise.yml` → `./.cruise.yaml` → `./.cruise.yml`
4. Current `.cruise/` directory: `*.yaml` / `*.yml` (ASCII-sorted), excluding the top-level `.cruise/default.yaml`
5. `~/.config/cruise/workflows/*.yaml` / `*.yml` (ASCII-sorted)
6. Built-in `tdd` (`builtin/tdd.yaml` in the source tree, embedded at build time: test-first steps + verify-review group (parallel read-only analyzers → consolidate → apply) + after-PR automation; uses the fallback `jcode` backend only when neither `command` nor `sdk` remains set in the resolved workflow after applicable `default.yaml` layers are applied) — selected by `-c __builtin__` or `-c builtin:tdd`.

The fixed built-in catalog and selector order is `tdd`, `refactor`, `add-tests`, `add-formal-specs`, `research`, and `security`. All six declare `after_pr` and `merge`, off when absent. `tdd` and `refactor` additionally declare `browser_e2e`, `app_e2e`, and `formal_methods`. `-f after_pr` enables the prelude without merge; `-f merge` runs the same prelude once and adds the final native synchronous squash merge. `research` stays read-only with `create_pr: false`, so merge never creates a PR or overrides its no-PR policy.

The shared six-step after-PR prelude is embedded with the binary and is not an
extra catalog workflow. Built-ins never fall back to local shared files, while
ordinary local YAML and inline YAML retain source-relative/base-directory
resolution. Eject materializes dependencies into standalone YAML without host
models, environment overrides, or defaults, preserves omitted root fields, and
refuses overwrites. Package artifacts and new builtin/repository/inline snapshots
also retain resolved content. Existing snapshots and step IDs are unchanged,
and file-backed sessions keep their live-reload contract.

`research` provides bounded read-only research with independent review and a cited report. `security` performs scoped, parallel security audits, triages findings, repairs confirmed safe defects, and independently verifies the results. `add-tests` inventories production behavior and real user-flow coverage before implementation and reports evidence-backed coverage/gap IDs; it does not impose numeric coverage quotas.

In a non-interactive context (stdin/stdout is not a TTY), the highest-priority candidate is adopted automatically. In an interactive terminal, an interactive selector lists all found config files and the built-in workflows with `tdd` first; with no config files found, `tdd` is adopted without prompting.

> `cruise workflow add owner/repo[/path][@ref]` installs a pinned, self-contained copy of a GitHub workflow into `~/.config/cruise/workflows/<name>.yaml` plus a `<name>.cruise-package.json` manifest. Hand-written YAMLs there are never touched by `update`/`remove`.

> User workflow YAMLs left directly in `~/.config/cruise/` are no longer discovered. Cruise emits a one-time warning and tells you to move them into `~/.config/cruise/workflows/`.

`$XDG_CONFIG_HOME/cruise/default.yaml` (when `XDG_CONFIG_HOME` is unset, macOS/Linux use `~/.config/cruise/default.yaml`; Windows uses `%APPDATA%\cruise\default.yaml` when `APPDATA` is set, otherwise it falls back to `~/.config/cruise/default.yaml`) and `./.cruise/default.yaml` provide workflow defaults, with project defaults taking precedence over user defaults; explicit `-c` or `CRUISE_CONFIG` selection of either defaults file is rejected. Environment overrides are applied after workflow and defaults layers. `pre_steps` from defaults are command-only steps inserted at the start of `steps` as `pre/<name>`; user defaults are applied before project defaults, and a project `pre_steps` entry with the same name replaces the user entry. A `default.yaml` inside `workflows/` is an ordinary selectable workflow.

Every actual jcode prompt must resolve to a Cruise model. If it does not,
Cruise stops before starting jcode and creates the user-level `default.yaml`
template only when no file exists. Replace its `model: null` with an available
model reference; project defaults are never auto-created. Claude and external
`command` backends retain their existing no-model behavior.

## Minimal config

`steps` is required. `command` and `sdk` are mutually exclusive; the default `jcode` SDK backend is used only when neither remains set in the resolved workflow after applicable `default.yaml` layers are applied.

```yaml
command: [claude, -p]
steps:
  implement:
    prompt: "{input}"
```

Or with an SDK backend instead of an external command (see [references/sdk.md](references/sdk.md)) — `sdk: jcode` (the default; uses jcode-sdk and requires a compatible jcode CLI for its API bridge) or `sdk: claude` (drives the claude CLI in-process), or `sdk: opencode` (built-in Rust client for a locally managed OpenCode V2 CLI, with `provider/model#variant` references):

```yaml
sdk: jcode
steps:
  implement:
    prompt: "{input}"
```

```yaml
sdk: claude
model: claude-sonnet-4-6   # plain model reference ("provider/model[:effort]" under sdk: jcode)
steps:
  implement:
    prompt: "{input}"
```

## Reference docs

The full spec is split into the files below. Load only the sections you need.

| Doc | Contents |
|-----|----------|
| [references/top-level.md](references/top-level.md) | Top-level structure, `command` and `{model}`, `sdk`, `description`, language settings (`languages.pr` / `languages.plan`, deprecated fields, and locale inference), `cleanup_after_pr`, workflow-only `create_pr`, `force_exec`, hot-reload, rate-limit retry |
| [references/sdk.md](references/sdk.md) | SDK backends: jcode SDK (default, requires jcode CLI v0.88.0+), claude-agent-sdk, and OpenCode V2; model references, session tools, workflow-level MCP servers, and differences from command mode |
| [references/steps.md](references/steps.md) | Step types and file-backed prompts: prompt, `prompt_file`, `output_file`, command, option, parallel, `github-review` (after-pr only); child restrictions, `instruction`, `timeout` |
| [references/variables.md](references/variables.md) | Template variables: `{input}`, `{prev.*}`, `{plan}`, `{file:...}`, `{plan.language}`, `{pr.*}` |
| [references/flow-control.md](references/flow-control.md) | `next` / `skip` / `when.exists` / `if.file-changed` / `if.no-file-changes` / `if.fail` / `timeout` / migration from the removed `fail-if-no-file-changes` |
| [references/groups.md](references/groups.md) | Step group definitions, call sites, validation rules |
| [references/after-pr.md](references/after-pr.md) | Steps that run after PR creation, plus constraints |
| [references/env-and-llm.md](references/env-and-llm.md) | Env-var merge/override rules, jcode OpenAI priority processing (`JCODE_OPENAI_SERVICE_TIER`), and SDK/command session-title generation |
| [examples/full-flow.yaml](examples/full-flow.yaml) | Complete example: plan → approve → implement → test → review → PR → after-pr |
| [examples/sdk-flow.yaml](examples/sdk-flow.yaml) | SDK-backend example: `sdk: jcode` with model references |
| [examples/opencode-flow.yaml](examples/opencode-flow.yaml) | OpenCode V2 example: `sdk: opencode`, `provider/model#variant`, and permission boundaries |
| [examples/claude-flow.yaml](examples/claude-flow.yaml) | SDK-backend example: `sdk: claude` with plain claude model names |
| [examples/prompt-file.yaml](examples/prompt-file.yaml) | Prompt step example using an external `prompt_file` |
| [examples/file-artifacts.yaml](../../examples/file-artifacts.yaml) | Save an initial prompt response and compare it in a later prompt with `{file:...}` |

## Authoring checklist

After writing or editing a config, verify each of the following:

1. **Required fields**: is `steps` present? `command` and `sdk` must not both be set (a validation error); when neither remains set in the resolved workflow after applicable `default.yaml` layers are applied, the default `jcode` backend runs. When `sdk` is set, is it `jcode`, `claude`, or `opencode`? (Any other value is a validation error.)
2. **Step type uniqueness**: each executable step holds one of `prompt` / `prompt_file` / `command` / `option` / `parallel` (pure `group` and `workflow_call` call sites hold none of these). Parallel children must be prompt/command steps; apply the parent and child restrictions in [references/steps.md](references/steps.md#parallel-step). Use `prompt_file` for long prompts; relative paths are resolved relative to the configuration file (or the called workflow's directory).
3. **Variable availability**: when referencing `{prev.*}`, does the previous step produce that output? `{plan}` is set by both `cruise run` and `cruise exec` (exec points it at an empty `plan.md` placeholder); `{pr.*}` is only available inside `after-pr`. Literal braces must be escaped Rust-`format!`-style (`{{` / `}}`) — an unescaped `{`/`}` that isn't a valid variable reference is a validation error, not passed through literally.
4. **`next:` targets**: do referenced step names exist (no typos)?
5. **`group:` call sites**: is the group defined, and does the call-site step avoid mixing `prompt` / `prompt_file` / `command` / `if:`?
6. **`if.no-file-changes`**: is the value either `retry` or `failed`? Make sure it isn't used inside `after-pr` or in a group-level `if:`.
7. **`if.fail`**: is the value either an existing step name or a mapping? Only `{ retry: true }` retries the current step — `{}`, `{ retry: false }`, and mappings with unknown keys parse and simply fall through without retrying. Make sure it isn't used inside `after-pr` or in a group-level `if:`.
8. **`after-pr`**: does it avoid `if.no-file-changes` and `if.fail`? A `github-review` step is valid only directly in `after-pr`, needs exactly one of `prompt` / `prompt_file`, a `timeout`, unique `bots`, and no `command` / `option` / `parallel` / `group` / `workflow_call`. If using native merge, is `squash-merge: true` the final direct step, gated exactly by `flag: merge`, with no incompatible fields? See [references/after-pr.md](references/after-pr.md#opt-in-native-synchronous-squash-merge).
9. **`timeout`**: does every timeout string parse (`"30"`, `"5m"`, `"1h"` — positive, no other suffixes)?
10. **`when.exists`**: is the glob non-empty and syntactically valid? (Globs containing `{...}` variables are only validated at runtime.)
11. **YAML order**: steps execute in declaration order, except children within a `parallel` block run concurrently and join before the next step — does that match the intended flow?
12. **Retry loops**: Conditional cycles are allowed when a normal exit is possible. Execution preflight rejects a reachable cycle only when no normal exit is reachable from the actual start position, considering configured and user-selected skips. A branch that may enter an exitless cycle produces a warning, not a blanket rejection. Runtime edge budgets still apply.

## Generated configs

`cruise workflow generate` drafts a workflow with an LLM and saves it only after the `exec` preflight passes. Preflight does not catch unknown top-level keys (silently ignored) and does not prove command safety. Review any generated YAML against the authoring checklist above before running it.
