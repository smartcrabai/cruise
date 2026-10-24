---
name: cruise-plan
description: Use when a coding agent should author an implementation plan itself (instead of letting cruise's LLM planning step write it) and register it as a local Cruise session or a GitHub Actions issue. Covers evidence-based plan quality, the required plan.md format including explicit workflow/flag/skip settings, the pinned foreground non-TTY session command, verification of saved settings, and issue-specific limits. Trigger when asked to "write a plan for cruise", "queue this task as a cruise session", "create a cruise session from this plan", or "file this plan as an issue" / "register a plan as an issue". For *driving* cruise (run/list/clean) see cruise-cli; for authoring workflow YAML see cruise-config.
---

cruise normally generates `plan.md` with its own LLM planning step. With `--skip-planning`, **you** are that planning step: your input is written directly to the session's `plan.md` after surrounding whitespace is trimmed (attachment references are included when `--image` is used), and no LLM-based planning call runs. In the foreground `cruise plan` form, a TTY still enters the Approve/Fix/Ask/Execute/Publish menu after that write; non-TTY input auto-approves to `Planned`. The session then stores an empty `input` with `input_as_plan: true`; `plan.md` holds the task text and `{input}` resolves to its current content.

`force_exec: true` is evaluated only for a `Local` target. If no explicit opt-out is present, local planning runs the workflow directly in the current directory — no plan, worktree, or PR. `--repo <OWNER/REPO>` resolves to a `PlanTarget::Repo` session, so it follows the session planning path instead of the local `force_exec` shortcut. For foreground `cruise plan`, `--no-force-exec`, `--grill`, or any attached image opts out of `force_exec`; the root `cruise --plan` form supports the `--no-force-exec` and `--image` opt-outs (it has no `--grill` flag).
The built-in `refactor`, `add-tests`, `add-formal-specs`, and `security` workflows set `force_exec: true` by default; use `--no-force-exec` to retain the normal plan/session path.

Formal specifications, E2E checks, and after-PR behavior are execution settings, not initial-planning modes; choose only flags declared by the selected workflow, as described below.

## Choose workflow and execution settings

Choose these before writing the plan or creating a session; select by the actual task, not by habit:

| Workflow | Choose it for |
|---|---|
| `builtin:tdd` | Default for ordinary feature work and bug fixes; test-first implementation. |
| `builtin:refactor` | Explicit behavior-preserving refactors. |
| `builtin:add-tests` | Auditing or improving repository tests, quality gates, and CI. |
| `builtin:add-formal-specs` | Requested executable Quint, Alloy, and Gherkin specifications with CI. |
| `builtin:research` | Read-only investigation and a cited report. |
| `builtin:security` | Evidence-led security audit and repair of confirmed defects. |

Use a custom workflow only when the user/project explicitly requires it or code/config evidence shows no built-in can do the task. Do not create custom YAML just to restate a built-in.

All six built-ins declare `after_pr` and `merge`; both are off unless explicitly enabled. `merge` performs the after-PR follow-up and a host-managed synchronous squash merge, so require explicit user authorization; it is never implicit. Only `tdd` and `refactor` also declare `browser_e2e`, `app_e2e`, and `formal_methods`. Enable E2E only when the changed surface warrants it; enable `formal_methods` only when useful formal specifications fit the task. The dedicated `add-formal-specs` workflow does not need that flag. `research` is read-only with `create_pr: false`, so opted-in after-PR/merge actions have no PR to act on. Undeclared flags are errors.

Default to no skipped steps. Record `Skipped steps: none`; only list exact skippable step IDs with a task-specific reason when exclusion is justified. Never skip required tests, security controls, or verification to make work easier. Plan prose records intent but does not change the saved workflow, flags, or skipped steps.


## Workflow

1. **Investigate the codebase first.** Read the relevant code before writing a word of the plan. Every claim in the plan must be backed by `file:line` evidence.
2. **Choose execution settings.** Pin the workflow, decide each declared flag's on/off value, and identify any justified skipped steps before drafting.
3. **Write the plan** following the format contract below, including its mandatory `## Execution settings` section.
4. **Create the session** with the pinned foreground non-TTY command below. It auto-approves to `Planned`; it does not execute the task.
5. **Verify** the phase and config with `cruise list --json`; confirm any non-default flags or skipped steps in the supported settings UI before running.

## Plan quality bar (same as cruise's built-in planning prompt)

This is what cruise demands of its own LLM planner — meet or beat it:

- **Per-requirement verdict.** For each requirement, decide "change needed" or "no change needed". A "no change needed" or "already correct" verdict **must** cite the supporting code as `file:line` — evidence is mandatory, not optional.
- **Classify reference material.** If the task points at external implementations, state whether each is a *bug-fix hint* or a *design approach to adopt*. If you narrow scope relative to the reference's intent, say why in the plan.
- **Resolve ambiguities by reading code**, not by guessing. Leave no "TBD during implementation" items that code investigation could settle now.
- **Identify the impact scope.** When adding parameters/fields, list **every call site** that needs wiring — the implementer will be told to grep-verify this.
- **Give the implementer concrete guidance:**
  - Existing implementation patterns to imitate (`file:line`). If similar processing already exists, citing it is required.
  - Anti-patterns specific to this task, if any (e.g. "don't add a fallback here, the value is always present").
- **Code-volume target.** Give each independently actionable implementation element a rough code-volume target (approximate changed-line count or small/medium/large), as a soft goal that never trades away correctness, error handling, tests, or other required work.

## plan.md format contract

```markdown
# <Concise task title>

## Requirements
- <requirement> → change needed / not needed (evidence: src/foo.rs:42)

## Design / Approach
<chosen approach; alternatives rejected and why; patterns to follow (file:line)>

## Execution settings
- Workflow: <exact built-in selector or config path, and why it fits>
- Flags: <every declared flag as NAME=on/off, with reasons for enabling or excluding task-relevant checks; use `none` only when the workflow declares no flags>
- Skipped steps: <none, or exact step IDs and task-specific reasons>

## Impact Scope
- <files/modules touched; for new params: every call site needing wiring>

## Implementation Steps
1. <ordered, concrete steps>

## Testing Notes
<intended behavior & interfaces, precisely enough to write tests BEFORE the implementation exists; happy path / error / boundary cases worth covering>

## Anti-patterns to avoid
- <task-specific traps, if any>
```

Rules:

- **Non-empty content** (enforced) — empty or whitespace-only plans are rejected; input is trimmed.
- **Start with a `#` heading** — cruise derives the session title from the first markdown heading of any level (no heading → falls back to the first content line, list markers stripped). Not enforced, but a missing heading yields a poor title.
- **Execution settings are mandatory.** Name the exact workflow, state every flag declared by it as on or off, and write `Skipped steps: none` unless justified exact IDs are selected. Flags absent are off. This section documents the intended settings; it does not configure the session.
- **Testing Notes must stand alone.** The built-in `tdd` workflow writes tests *from the plan, before any implementation* — spell out behavior and interfaces (names, signatures, error types), not "test the new function".

## Creating the session

Run from the **target repository's root** — the session binds to the current directory (worktrees, config resolution).

### Local session — pin the selected workflow

Prepare `plan.md` with the required `## Execution settings` section, then run:

```sh
cruise plan --skip-planning --no-force-exec -c builtin:tdd < plan.md
```

Replace `builtin:tdd` with the exact workflow chosen above, or use `-c path/to/cruise.yaml` for an intentionally selected custom config. `--no-force-exec` prevents built-ins such as `refactor`, `add-tests`, `add-formal-specs`, and `security` from executing directly instead of creating a session. Append `--flag NAME` for each declared flag explicitly set on; flags are off when absent. Do not pass undeclared names. The root `cruise --plan` form has no `-c`, so do not use it when the workflow must be pinned.

This foreground command is recommended because its non-TTY stdin is auto-approved to `Planned`; it makes no planning LLM call and does not run the task. Registration is execution-ready, so use it only when the user asked to register a session. Do not run `cruise run` unless execution was requested. `--image <PATH>` is repeatable and appends copied attachment paths to the plan text.

`--skip-planning` skips plan generation, not every model call: foreground approval in SDK mode can still invoke `generate_title`. In a TTY, this command enters the approval menu instead of auto-approving; its skip-step prompt may preselect history, so clear any exclusions not justified by the current plan.

### Set non-empty skipped steps

There is no non-TTY CLI option for `skipped_steps`. After creating the session, use the interactive `cruise list` → select the session → **Edit Settings** flow to choose exact steps and declared flag values. Reopen the settings and confirm the saved choices before running. If the required skip settings cannot be applied and confirmed through this UI, do not run the session or claim it is fully configured. Do not edit session JSON directly.

### Verify

```sh
cruise list --json | jq '.[] | select(.id=="<session-id>") | {id, phase, title, config_source, config_path}'
# expect phase "Planned" and the selected workflow/config
```

`cruise list --json` does **not** include flags or skipped steps. Confirm any non-default flag values and every non-empty skip list with **Edit Settings**; the plan text and JSON listing are not proof that those settings were saved.


## Creating a GitHub issue instead (the @cruise Actions integration)

When the target repository has the cruise GitHub Action installed (`.github/workflows/cruise.yml` + an API-key secret — see `docs/github-actions.md`), a plan can be registered as a **GitHub issue** instead of a local session. Execution then happens on GitHub Actions and ends in a draft PR, with the whole exchange reviewable in the issue thread.

The `## Execution settings` section in an issue body is documentation only: it does not select the Actions workflow or set execution flags/skips. The action's `config` input or checkout repository config determines the workflow; its `flags` input supplies declared flags for `run`. The issue body cannot pin a local `-c` config, and the documented action interface has no per-run `skipped_steps` input. Before triggering an issue, confirm the repo-owned Actions/config path implements the required workflow and flags. If non-empty user-selected skips are required but that path cannot set them, do not trigger it; use a local session with the supported settings UI when appropriate.

How it maps: for `@cruise run`, the action resolves the plan from (1) the last plan comment posted by the cruise bot, else (2) the **issue title + body**. A locally-authored plan therefore goes in the issue body:

```sh
gh issue create --repo <owner>/<repo> \
  --title "<task title — same as the plan's # heading>" \
  --body-file plan.md
```

Then either leave it for review (someone with write access comments `@cruise run` when ready), or trigger immediately with a separate comment:

```sh
gh issue comment <issue#> --repo <owner>/<repo> --body "@cruise run"
```

cruise can also file the issue for you, without leaving the CLI: a session in `AwaitingApproval` or `Planned` whose plan is readable offers **Publish as Issue** in the `cruise list` action menu, and the same entry is in the `cruise plan` approve menu. It resolves `owner/repo` from the session's `--repo` value, falling back to the `origin` remote of its base directory, creates the issue with the session title and the plan as the body verbatim, asks `Post an @cruise run comment after creating the issue?` (default no), then deletes the local session together with its worktree or temporary clone. On failure the session is kept; retrying reuses the already-created issue instead of filing a duplicate.

Rules specific to this path:

- **Never put the string `@cruise` inside the issue title or body** unless you intend to execute the moment the issue is created — `issues: opened` concatenates the title and body and fires on any mention there. The token right after the trigger phrase selects the command: `plan`, `fix`, `exec`, or `run`; with no token or an unknown one, the default *run* fires. Keep title and body mention-free and trigger with a separate comment.
- **Do not post the plan as an issue comment.** The action only trusts `<!-- cruise:plan -->` comments authored by its own bot identities (`cruise-agent[bot]` / `github-actions[bot]`); a hand-posted marker comment is deliberately ignored (plan-spoofing defense). The issue body is the supported channel for pre-authored plans.
- **Revising the plan = edit the issue body** (`gh issue edit <n> --repo <owner>/<repo> --body-file plan-v2.md`). `@cruise fix` only revises bot-posted plan comments (plans produced by `@cruise plan`), not issue bodies.
- The trigger comment must come from a user with **write access**, unless the actor is a bot whose login is included in `allowed_bots` (or `allowed_bots` is `*`), because `gate.sh` accepts those bots without checking collaborator permission; it should be exactly `@cruise run` — any extra text in it is appended to the plan as additional instructions.
- Same quality bar and format contract as above; the plan is consumed as written (no LLM rewrite), except that the action strips HTML comments, `<img>` tags, and invisible Unicode controls from the issue text and cruise trims surrounding whitespace. The issue title duplicating the plan's `#` heading is fine.

Choose by where execution should happen: a local session (`cruise run` on this machine) vs an issue (GitHub Actions, async, reviewable thread, draft PR at the end). For an issue, configure and verify the repository-owned Actions workflow/config separately; the plan body alone never applies the local execution settings.

## Gotchas

- **Workflow config is resolved and validated at session-creation time.** If resolution finds no config, the built-in `tdd` workflow (`builtin/tdd.yaml` in the source tree, embedded at build time) applies; if a found config is invalid, creation fails. Use foreground `cruise plan -c builtin:<name>` or `-c <path>` to pin the selected config; `-c __builtin__` selects generic `tdd`. The root `cruise --plan` form does not accept `-c`.
- **A planning worktree is created even with `--skip-planning` for git sessions** (under `$XDG_DATA_HOME/cruise/worktrees/<id>/`) and is reused by `cruise run` for local sessions. For non-git directories, planning falls back to the base directory, but `cruise run` needs a git repository (worktree mode is the non-TTY default) and fails otherwise. For `--repo` sessions, approval removes the planning worktree and temporary clone, so `cruise run` re-clones the repository and creates a fresh execution worktree.
- **One task = one session.** Don't pack multiple unrelated tasks into one plan; create separate pinned foreground sessions.
- **Plan text is used verbatim** — no LLM cleans it up afterwards (only surrounding whitespace is trimmed; the GitHub Action additionally strips HTML comments, `<img>` tags, and invisible Unicode controls). Typos in file paths or step ordering go straight to the implementer.
- **The plan file lives at `$XDG_DATA_HOME/cruise/sessions/<session-id>/plan.md`** (`$HOME/.local/share/cruise/...` when `XDG_DATA_HOME` is unset). Read it to confirm what was registered, and edit it there to correct a registered plan.
- **A session parked in `AwaitingInput`** (SDK planning is waiting for an unanswered question) is not stuck: `cruise list` → select the session → **Generate Plan** reruns planning and moves it to `AwaitingApproval`, where `Approve` is available. The same menu entry appears for `Draft`. `AwaitingApproval` offers `Approve`, `Publish as Issue`, and `Edit Settings`; `Planned` offers `Run`, `Publish as Issue`, `Edit Settings`, and `Replan`. Publish as Issue is hidden when the plan is missing or errored.
