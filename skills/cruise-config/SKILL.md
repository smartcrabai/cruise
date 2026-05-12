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

When `-c/--config` is not specified, cruise searches in this order:

1. `-c/--config <path>` flag
2. `CRUISE_CONFIG` environment variable
3. Current directory: `./cruise.yaml` → `./cruise.yml` → `./.cruise.yaml` → `./.cruise.yml`
4. `~/.cruise/*.yaml` / `*.yml` (auto-selected if exactly one, prompted if multiple)
5. Built-in default (a 2-step `write-tests` → `implement` workflow)

## Minimal config

Only `command` and `steps` are required.

```yaml
command: [claude, -p]
steps:
  implement:
    prompt: "{input}"
```

## Reference docs

The full spec is split into the files below. Load only the sections you need.

| Doc | Contents |
|-----|----------|
| [references/top-level.md](references/top-level.md) | Top-level structure, `command` and `{model}`, `pr_language`, hot-reload, rate-limit retry |
| [references/steps.md](references/steps.md) | The three step types: prompt, command, option |
| [references/variables.md](references/variables.md) | Template variables: `{input}`, `{prev.*}`, `{plan}`, `{pr.*}` |
| [references/flow-control.md](references/flow-control.md) | `next` / `skip` / `if.file-changed` / `if.no-file-changes` / legacy `fail-if-no-file-changes` |
| [references/groups.md](references/groups.md) | Step group definitions, call sites, validation rules |
| [references/after-pr.md](references/after-pr.md) | Steps that run after PR creation, plus constraints |
| [references/env-and-llm.md](references/env-and-llm.md) | Env-var merge rules, the `llm:` section for session-title generation |
| [examples/full-flow.yaml](examples/full-flow.yaml) | Complete example: plan → approve → implement → test → review → PR → after-pr |

## Authoring checklist

After writing or editing a config, verify each of the following:

1. **Required fields**: are `command` and `steps` present?
2. **Step type uniqueness**: each step primarily holds one of `prompt` / `command` / `option` (group-call steps are the exception and hold none of these).
3. **Variable availability**: when referencing `{prev.*}`, does the previous step produce that output? `{plan}` is only set during `cruise run`; `{pr.*}` is only available inside `after-pr`.
4. **`next:` targets**: do referenced step names exist (no typos)?
5. **`group:` call sites**: is the group defined, and does the call-site step avoid mixing `prompt` / `command` / `if:`?
6. **`if.no-file-changes`**: is exactly one of `fail` / `retry` set to true? Make sure it isn't used inside `after-pr` or in a group-level `if:`.
7. **`after-pr`**: does it avoid `fail-if-no-file-changes` and `if.no-file-changes`?
8. **YAML order**: steps execute in declaration order — does that match the intended flow?
