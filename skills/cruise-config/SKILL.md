---
name: cruise-config
description: Use when creating or editing a cruise YAML config file (cruise.yaml / .cruise.yaml). Covers inline and file-backed prompt steps (`prompt` / `prompt_file`), variables, flow control, groups, after-pr, and validation rules — full spec is split across reference docs.
---

cruise is a workflow orchestrator that drives coding agent CLIs like `claude -p` via a YAML config. This skill documents the config file format.

## When to use

- Creating a new `cruise.yaml` / `.cruise.yaml` / `cruise.yml` / `.cruise.yml`
- Adding or editing steps in an existing cruise config
- Designing a workflow (plan → implement → test → review → PR) in YAML
- Looking up field names, variable names, or validation rules

## Config file resolution

Config files are resolved in this priority order:

1. `-c/--config <path>` flag (highest priority; never prompts). The special value `-c __builtin__` selects the built-in default workflow even when config files exist
2. `CRUISE_CONFIG` environment variable (error if the file does not exist; never prompts)
3. Current directory: `./cruise.yaml` → `./cruise.yml` → `./.cruise.yaml` → `./.cruise.yml`
4. Current `.cruise/` directory: `*.yaml` / `*.yml` (ASCII-sorted)
5. `~/.config/cruise/workflows/*.yaml` / `*.yml` (ASCII-sorted)
6. Built-in default (`builtin/cruise.yaml` in the source tree, embedded at build time: test-first steps + verify-review group + after-PR automation, run on the default `jcode` SDK backend) — also explicitly selectable via `-c __builtin__`, the **Built-in default** entry at the end of the interactive selector, or the GUI's **Built-in default** option

In a non-interactive context (stdin/stdout is not a TTY), the highest-priority candidate is adopted automatically. In an interactive terminal, an interactive selector lists all found config files with a trailing **Built-in default** entry; with no config files found, the built-in default is adopted without prompting.

> User workflow YAMLs left directly in `~/.config/cruise/` are no longer discovered. Cruise emits a one-time warning and tells you to move them into `~/.config/cruise/workflows/`.

## Minimal config

`steps` is required. `command` and `sdk` are mutually exclusive; omitting both runs prompts on the default `jcode` SDK backend.

```yaml
command: [claude, -p]
steps:
  implement:
    prompt: "{input}"
```

Or with an SDK backend instead of an external command (see [references/sdk.md](references/sdk.md)) — `sdk: jcode` (the default — drives the jcode CLI) or `sdk: claude` (drives the claude CLI in-process):

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
| [references/top-level.md](references/top-level.md) | Top-level structure, `command` and `{model}`, `sdk`, `description`, language settings (`languages.pr` / `languages.plan`, deprecated fields, and locale inference), `cleanup_after_pr`, `force_exec`, hot-reload, rate-limit retry |
| [references/sdk.md](references/sdk.md) | SDK backends: `sdk: jcode` (default; jcode CLI subprocess, model references, `cruise login` auth) and `sdk: claude` (in-process claude CLI), differences from command mode |
| [references/steps.md](references/steps.md) | Step types and file-backed prompts: prompt, `prompt_file`, command, option; `instruction`, `timeout` |
| [references/variables.md](references/variables.md) | Template variables: `{input}`, `{prev.*}`, `{plan}`, `{plan.language}`, `{pr.*}` |
| [references/flow-control.md](references/flow-control.md) | `next` / `skip` / `when.exists` / `if.file-changed` / `if.no-file-changes` / `if.fail` / `timeout` / legacy `fail-if-no-file-changes` |
| [references/groups.md](references/groups.md) | Step group definitions, call sites, validation rules |
| [references/after-pr.md](references/after-pr.md) | Steps that run after PR creation, plus constraints |
| [references/env-and-llm.md](references/env-and-llm.md) | Env-var merge/override rules and SDK/command session-title generation |
| [examples/full-flow.yaml](examples/full-flow.yaml) | Complete example: plan → approve → implement → test → review → PR → after-pr |
| [examples/sdk-flow.yaml](examples/sdk-flow.yaml) | SDK-backend example: `sdk: jcode` with model references |
| [examples/claude-flow.yaml](examples/claude-flow.yaml) | SDK-backend example: `sdk: claude` with plain claude model names |
| [examples/prompt-file.yaml](examples/prompt-file.yaml) | Prompt step example using an external `prompt_file` |

## Authoring checklist

After writing or editing a config, verify each of the following:

1. **Required fields**: is `steps` present? `command` and `sdk` must not both be set (a validation error); when neither is set, the default `jcode` backend runs. When `sdk` is set, is it `jcode` or `claude`? (Any other value is a validation error.)
2. **Step type uniqueness**: each step primarily holds one of `prompt` / `prompt_file` / `command` / `option` (group-call steps are the exception and hold none of these). Use `prompt_file` for long prompts; relative paths are resolved relative to the configuration file (or the called workflow's directory).
3. **Variable availability**: when referencing `{prev.*}`, does the previous step produce that output? `{plan}` is only set during `cruise run`; `{pr.*}` is only available inside `after-pr`. Literal braces must be escaped Rust-`format!`-style (`{{` / `}}`) — an unescaped `{`/`}` that isn't a valid variable reference is a validation error, not passed through literally.
4. **`next:` targets**: do referenced step names exist (no typos)?
5. **`group:` call sites**: is the group defined, and does the call-site step avoid mixing `prompt` / `prompt_file` / `command` / `if:`?
6. **`if.no-file-changes`**: is the value either `retry` or `failed`? Make sure it isn't used inside `after-pr` or in a group-level `if:`.
7. **`if.fail`**: is the value either an existing step name or `{ retry: true }`? Make sure it isn't used inside `after-pr` or in a group-level `if:`.
8. **`after-pr`**: does it avoid `if.no-file-changes` and `if.fail`?
9. **`timeout`**: does every timeout string parse (`"30"`, `"5m"`, `"1h"` — positive, no other suffixes)?
10. **`when.exists`**: is the glob non-empty and syntactically valid? (Globs containing `{...}` variables are only validated at runtime.)
11. **YAML order**: steps execute in declaration order — does that match the intended flow?
12. **Retry loops**: is any top-level step cycle mixing conditional jumps (`if.file-changed`, `if.fail` goto) with unconditional sequential edges? Such cycles are rejected at startup — confine retry loops inside a group under `groups:` with `max_retries` (see [references/flow-control.md](references/flow-control.md)).
