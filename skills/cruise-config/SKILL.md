---
name: cruise-config
description: Use when creating or editing a cruise YAML config file (cruise.yaml / .cruise.yaml). Covers step definitions, variables, flow control, groups, after-pr, and validation rules — full spec is split across reference docs.
---

cruise is a workflow orchestrator that drives coding agent CLIs like `claude -p` via a YAML config. This skill documents the config file format.

## When to use

- Creating a new `cruise.yaml` / `.cruise.yaml` / `cruise.yml` / `.cruise.yml`
- Adding or editing steps in an existing cruise config
- Designing a workflow (plan → implement → test → review → PR) in YAML
- Looking up field names, variable names, or validation rules

## Config file resolution

Config files are resolved in this priority order:

1. `-c/--config <path>` flag (highest priority; never prompts)
2. `CRUISE_CONFIG` environment variable (error if the file does not exist; never prompts)
3. Current directory: `./cruise.yaml` → `./cruise.yml` → `./.cruise.yaml` → `./.cruise.yml`
4. `~/.config/cruise/*.yaml` / `*.yml` (ASCII-sorted)
5. Built-in default (a 2-step `write-tests` → `implement` workflow) — implicit fallback only, never offered as a selectable choice

In a non-interactive context (stdin/stdout is not a TTY), the highest-priority candidate is adopted automatically. In an interactive terminal, when two or more real config files are found, an interactive selector is shown (a single real file is auto-selected).

## Minimal config

`steps` and exactly one of `command` / `sdk` are required.

```yaml
command: [claude, -p]
steps:
  implement:
    prompt: "{input}"
```

Or with the seher SDK backend instead of an external command (see [references/sdk.md](references/sdk.md)):

```yaml
sdk: seher
steps:
  implement:
    prompt: "{input}"
```

## Reference docs

The full spec is split into the files below. Load only the sections you need.

| Doc | Contents |
|-----|----------|
| [references/top-level.md](references/top-level.md) | Top-level structure, `command` and `{model}`, `sdk`, `description`, `pr_language`, `cleanup_after_pr`, hot-reload, rate-limit retry |
| [references/sdk.md](references/sdk.md) | The seher SDK backend: `sdk: seher`, mode keys, differences from command mode |
| [references/steps.md](references/steps.md) | The three step types: prompt, command, option; `instruction`, `timeout` |
| [references/variables.md](references/variables.md) | Template variables: `{input}`, `{prev.*}`, `{plan}`, `{pr.*}` |
| [references/flow-control.md](references/flow-control.md) | `next` / `skip` / `when.exists` / `if.file-changed` / `if.no-file-changes` / `if.fail` / `timeout` / legacy `fail-if-no-file-changes` |
| [references/groups.md](references/groups.md) | Step group definitions, call sites, validation rules |
| [references/after-pr.md](references/after-pr.md) | Steps that run after PR creation, plus constraints |
| [references/env-and-llm.md](references/env-and-llm.md) | Env-var merge rules, the `llm:` section for session-title generation |
| [examples/full-flow.yaml](examples/full-flow.yaml) | Complete example: plan → approve → implement → test → review → PR → after-pr |
| [examples/sdk-flow.yaml](examples/sdk-flow.yaml) | SDK-backend example: `sdk: seher` with mode keys |

## Authoring checklist

After writing or editing a config, verify each of the following:

1. **Required fields**: is `steps` present, plus exactly one of `command` / `sdk`? (Both set or neither set is a validation error.)
2. **Step type uniqueness**: each step primarily holds one of `prompt` / `command` / `option` (group-call steps are the exception and hold none of these).
3. **Variable availability**: when referencing `{prev.*}`, does the previous step produce that output? `{plan}` is only set during `cruise run`; `{pr.*}` is only available inside `after-pr`.
4. **`next:` targets**: do referenced step names exist (no typos)?
5. **`group:` call sites**: is the group defined, and does the call-site step avoid mixing `prompt` / `command` / `if:`?
6. **`if.no-file-changes`**: is exactly one of `fail` / `retry` set to true? Make sure it isn't used inside `after-pr` or in a group-level `if:`.
7. **`if.fail`**: is the value either an existing step name or `{ retry: true }`? Make sure it isn't used inside `after-pr` or in a group-level `if:`.
8. **`after-pr`**: does it avoid `fail-if-no-file-changes`, `if.no-file-changes`, and `if.fail`?
9. **`timeout`**: does every timeout string parse (`"30"`, `"5m"`, `"1h"` — positive, no other suffixes)?
10. **`when.exists`**: is the glob non-empty and syntactically valid? (Globs containing `{...}` variables are only validated at runtime.)
11. **YAML order**: steps execute in declaration order — does that match the intended flow?
