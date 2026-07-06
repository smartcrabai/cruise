---
name: cruise-plan
description: Use when a coding agent should author an implementation plan itself (instead of letting cruise's LLM planning step write it) and register it as a cruise session via `--skip-planning`, or as a GitHub issue for the @cruise Actions integration. Covers cruise's plan-quality best practices (the same bar as the built-in plan prompt), the required plan.md format, the exact commands to create the session — both background (`cruise --plan … --skip-planning`) and foreground non-TTY (`cruise plan --skip-planning …`) land in `Planned` and are ready for `cruise run` immediately — and how to file the plan as an issue (plan in the issue body, optional `@cruise run` trigger comment). Trigger when asked to "write a plan for cruise", "queue this task as a cruise session", "create a cruise session from this plan", or "file this plan as an issue" / "プランをissueとして登録". For *driving* cruise (run/list/clean) see cruise-cli; for authoring workflow YAML see cruise-config.
---

cruise normally generates `plan.md` with its own LLM planning step. With `--skip-planning`, **you** are that planning step: your text is written verbatim to the session's `plan.md` and the downstream workflow (built-in default: write tests → implement, then PR creation) executes against it. This skill is the contract for doing that well.

## Workflow

1. **Investigate the codebase first.** Read the relevant code before writing a word of the plan. Every claim in the plan must be backed by `file:line` evidence.
2. **Write the plan** following the format contract below.
3. **Create the session** with one of the `--skip-planning` commands.
4. **Verify** with `cruise list --json`.

## Plan quality bar (same as cruise's built-in planning prompt)

This is what cruise demands of its own LLM planner — meet or beat it:

- **Per-requirement verdict.** For each requirement, decide "change needed" or "no change needed". A "no change needed" or "already correct" verdict **must** cite the supporting code as `file:line` — evidence is mandatory, not optional.
- **Classify reference material.** If the task points at external implementations, state whether each is a *bug-fix hint* or a *design approach to adopt*. If you narrow scope relative to the reference's intent, say why in the plan.
- **Resolve ambiguities by reading code**, not by guessing. Leave no "TBD during implementation" items that code investigation could settle now.
- **Identify the impact scope.** When adding parameters/fields, list **every call site** that needs wiring — the implementer will be told to grep-verify this.
- **Give the implementer concrete guidance:**
  - Existing implementation patterns to imitate (`file:line`). If similar processing already exists, citing it is required.
  - Anti-patterns specific to this task, if any (e.g. "don't add a fallback here, the value is always present").

## plan.md format contract

```markdown
# <Concise task title>

## Requirements
- <requirement> → change needed / not needed (evidence: src/foo.rs:42)

## Design / Approach
<chosen approach; alternatives rejected and why; patterns to follow (file:line)>

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
- **Testing Notes must stand alone.** The default workflow writes tests *from the plan, before any implementation* — spell out behavior and interfaces (names, signatures, error types), not "test the new function".

## Creating the session

Run from the **target repository's root** — the session binds to the current directory (worktrees, config resolution).

### Background — session lands in `Planned` (ready to run)

```sh
cruise --plan stdin --skip-planning <<'EOF'
# Add retry to the uploader
...
EOF
```

Use the `stdin` sentinel + heredoc for multiline plans (no shell-quoting hazards; the quoted `'EOF'` keeps backticks and `$` intact). Inline also works: `cruise --plan "<plan>" --skip-planning`. No LLM is called on this path; the command prints the session ID and returns immediately. The session lands directly in `Planned` — `cruise run` can pick it up with no human approval step.

**Default to this form** — it lets you queue work from any context (GUI, agent, shell). The session lands in `Planned` and `cruise run` (or `cruise run --all`) will pick it up automatically; no human approval step is required.

### Foreground non-TTY — auto-approved straight to `Planned`

```sh
cat plan.md | cruise plan --skip-planning            # plan on stdin
cruise plan --skip-planning "$(cat plan.md)"         # or as the positional arg
cruise plan --skip-planning -c path/to/cruise.yaml "$(cat plan.md)"   # explicit config
```

When stdin is not a TTY (always true for an agent's shell), the approve menu is skipped and the session is **auto-approved to `Planned`** — `cruise run` will execute it with no human review. Only use this when the user explicitly wants unattended queuing. The root-level shorthand `cruise --skip-planning "<plan>"` behaves the same (legacy no-subcommand path). Two caveats: auto-approval calls an LLM for title generation when the config has an `llm:` block (the plan itself is still used verbatim), and the positional `"$(cat plan.md)"` form breaks if the plan starts with `-` (clap reads it as a flag) — prefer the stdin form.

### Verify

```sh
cruise list --json | jq '.[] | select(.id=="<session-id>") | {id, phase, title}'
# expect phase "Planned" in both cases
```

## Creating a GitHub issue instead (the @cruise Actions integration)

When the target repository has the cruise GitHub Action installed (`.github/workflows/cruise.yml` + an API-key secret — see `docs/github-actions.md`), a plan can be registered as a **GitHub issue** instead of a local session. Execution then happens on GitHub Actions and ends in a draft PR, with the whole exchange reviewable in the issue thread.

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

Rules specific to this path:

- **Never put the string `@cruise` inside the issue title or body** unless you intend to execute the moment the issue is created — `issues: opened` fires a bare *run* on any body/title mention. Keep the body mention-free and trigger with a separate comment.
- **Do not post the plan as an issue comment.** The action only trusts `<!-- cruise:plan -->` comments authored by its own bot identities (`cruise-agent[bot]` / `github-actions[bot]`); a hand-posted marker comment is deliberately ignored (plan-spoofing defense). The issue body is the supported channel for pre-authored plans.
- **Revising the plan = edit the issue body** (`gh issue edit <n> --repo <owner>/<repo> --body-file plan-v2.md`). `@cruise fix` only revises bot-posted plan comments (plans produced by `@cruise plan`), not issue bodies.
- The trigger comment must come from a user with **write access** (the gate rejects others) and should be exactly `@cruise run` — any extra text in it is appended to the plan as additional instructions.
- Same quality bar and format contract as above; the plan is consumed verbatim. The issue title duplicating the plan's `#` heading is fine.

Choose by where execution should happen: local session (`cruise run` on this machine, immediate) vs issue (GitHub Actions, async, reviewable thread, draft PR at the end).

## Gotchas

- **Workflow config is resolved and validated at session-creation time.** If resolution finds no config, the built-in 2-step `write-tests → implement` workflow applies; if a found config is invalid, creation fails. Pass `-c` (foreground form only) to pin a specific config.
- **A planning worktree is created even with `--skip-planning`** (under `$XDG_DATA_HOME/cruise/worktrees/<id>/`); it is reused by `cruise run`. Non-git directories fall back to running in place.
- **One task = one session.** Don't pack multiple unrelated tasks into one plan; queue several sessions instead (`cruise --plan … --skip-planning` per task).
- **Plan text is used verbatim** — no LLM cleans it up afterwards. Typos in file paths or step ordering go straight to the implementer.
