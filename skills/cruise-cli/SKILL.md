---
name: cruise-cli
description: Use when running, operating, or troubleshooting the `cruise` CLI — the YAML-driven coding-agent workflow orchestrator that wraps `claude -p` and friends. Covers which subcommand to reach for (plan / --plan / draft / run / exec / list / clean / config), the session lifecycle and phases, worktree vs current-branch modes, config-file resolution, and runtime file layout. Trigger whenever the user asks how to start a cruise session, run or resume a workflow, manage/clean sessions, pick a workspace mode, or debug why a session is stuck or skipped — even if they don't name the exact subcommand. For *authoring* the workflow YAML itself (step fields, variables, groups), use the cruise-config skill instead.
---

cruise is a CLI that drives coding-agent CLIs (like `claude -p`) through a declarative YAML workflow: **plan → approve → run (write tests → implement → test → review) → open PR → after-pr automation**. This skill is the operator's manual — how to *drive* cruise. For writing the workflow YAML itself, see the **cruise-config** skill.

## Mental model

Work flows through **sessions**, each with a phase. The normal path is:

```
plan/draft  →  AwaitingApproval  →  (approve)  →  Planned  →  run  →  Running  →  Completed  →  clean
```

- A **session** is a unit of work (one task → one plan → one run → usually one PR).
- `cruise plan`/`--plan`/`draft` *create* sessions; `cruise run` *executes* them; `cruise list` *manages* them; `cruise clean` *garbage-collects* them.
- `cruise exec` is the **odd one out**: it runs a workflow against the current directory with **no session lifecycle, no worktree, no PR**. Reach for it for quick, throwaway, in-place runs.

## Which command do I want?

| Goal | Command |
|------|---------|
| Plan a task, then approve it interactively (foreground) | `cruise plan "task"` |
| Plan in the background, review later, return immediately | `cruise --plan "task"` |
| I already wrote the plan myself — skip the LLM planning | `cruise plan --skip-planning "<plan text>"` (or `cruise --plan "…" --skip-planning`) |
| Just capture an idea now, plan later | `cruise draft "task"` |
| Execute the next approved (Planned) session | `cruise run` |
| Execute a specific session | `cruise run <session-id>` |
| Execute every Planned session back-to-back | `cruise run --all` |
| Run a config right here, no plan/worktree/PR | `cruise exec "task"` |
| Browse / approve / resume / delete sessions | `cruise list` |
| Dump session state for scripts | `cruise list --json` |
| Delete sessions whose PR is merged/closed | `cruise clean` |
| Show / change app-level settings (e.g. GUI parallelism) | `cruise config` |
| See what *would* run without executing | add `--dry-run` to `plan` / `run` / `exec` |

> **Legacy shortcut:** `cruise "task"` with no subcommand is treated as `cruise plan "task"`. Piping (`echo "task" | cruise`) feeds the task on stdin.

## The session lifecycle, step by step

1. **Create a session.**
   - `cruise plan "task"` — runs the built-in plan step in an isolated *planning worktree*, then drops you into the **approve-plan menu** (below). Foreground.
   - `cruise --plan "task"` — creates the session and spawns a detached worker to generate the plan, then returns the session ID immediately. `cruise list` shows it as `Planning`, then `AwaitingApproval` (or `Plan Failed`).
   - `cruise draft "task"` — records the task as a `Draft` with no planning at all. Plan it later via **Generate Plan** in `cruise list`.

2. **Approve the plan.** The approve-plan menu offers:
   - **Approve** → session becomes `Planned`, ready to run.
   - **Fix** → give feedback; the plan step reruns with your input.
   - **Ask** → ask a question; the answer is shown, then the menu reappears.
   - **Execute now** → skip approval and run immediately.

3. **Run.** `cruise run` picks up a `Planned` session, prompts for a **workspace mode** (below), reuses/creates the worktree, executes the workflow steps, creates a PR with `gh pr create`, then runs any `after-pr` steps. The session ends as `Completed` (or `Failed`).

4. **Clean up.** `cruise clean` checks each `Completed` session's PR via `gh pr view` and deletes the session + worktree once the PR is merged or closed.

### `--skip-planning`

No LLM is called: your input is written verbatim to `plan.md` and the session goes straight to `AwaitingApproval`. Empty/whitespace input is rejected. Use it when you've already written the plan and just want cruise to execute it. Requires either `--plan` or the positional input form.

## Workspace modes (chosen at `cruise run`)

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| Mode | What it does | When to use |
|------|--------------|-------------|
| **Worktree** (default) | Isolated git worktree under `$XDG_DATA_HOME/cruise/worktrees/<id>/`, new branch `cruise/<id>-<slug>`, auto-PR via `gh`. | The normal choice. Keeps your working copy untouched; supports parallel sessions. **Requires `gh` CLI.** |
| **Current branch** | Runs in place on the active branch. No worktree, no auto-PR. | Quick iterations on the current branch. Needs a **clean working tree** and an **attached branch** (not detached HEAD). On resume the branch must match. |

Non-interactive runs (piped stdin) and `cruise run --all` always force worktree mode.

**Copy files into the worktree** by listing relative paths in a `.worktreeinclude` at the repo root (e.g. `.env`, `secrets/`). Absolute paths and `..` are ignored for safety.

## `cruise list` — phase → available actions

The interactive menu changes with the session's phase:

| Phase | Actions |
|-------|---------|
| **Draft** | Generate Plan, Delete, Back |
| **AwaitingApproval** | Approve, Delete, Back |
| **Planned** | Run, Replan, Delete, Back |
| **Running** | Resume, Reset to Planned, Delete, Back |
| **Suspended** | Resume, Reset to Planned, Delete, Back |
| **Failed** | Run, Reset to Planned, Delete, Back |
| **Completed** | Open PR*, Reset to Planned, Delete, Back |
| **Planning** / **Plan Failed** | Delete, Back (Approve appears only once a non-empty `plan.md` exists) |

\* Open PR shows only when the session has a PR URL.

- **Reset to Planned** clears the current step so the session re-runs from the start — the go-to recovery for a wedged `Running`/`Failed` session.
- **Replan** regenerates the plan from feedback while staying `Planned`.

## Config-file resolution

`cruise run`/`plan`/`exec` resolve the **workflow YAML** in this order:

1. `-c/--config <path>` (must exist; no prompt)
2. `CRUISE_CONFIG` env var (must exist; no prompt)
3. Current dir: `./cruise.yaml` → `.yml` → `./.cruise.yaml` → `./.cruise.yml`, then `$XDG_CONFIG_HOME/cruise/*.yaml|*.yml`. Multiple candidates → interactive picker (TTY) or highest-priority auto-pick (non-interactive).
4. None found → a built-in 2-step `write-tests → implement` workflow.

> To *write* or edit that YAML, switch to the **cruise-config** skill.

## Runtime file layout (XDG)

| Kind | Path (default) |
|------|----------------|
| User YAML configs + app settings (`config.json`) | `$XDG_CONFIG_HOME/cruise/` → `~/.config/cruise/` |
| Sessions + worktrees | `$XDG_DATA_HOME/cruise/` → `~/.local/share/cruise/` |
| State (`history.json`, `new_session_draft.json`) | `$XDG_STATE_HOME/cruise/` → `~/.local/state/cruise/` |

> Older versions kept everything under `~/.cruise/`. If migrating, move configs to `~/.config/cruise/`, `sessions/`+`worktrees/` to `~/.local/share/cruise/`, and use `git worktree move`/`repair` for worktrees.

## Operational notes & gotchas

- **`gh` CLI is required** for worktree mode (PR creation) and `cruise clean` (PR status). Current-branch and `exec` don't need it.
- **`cruise clean` skips sessions with no PR URL or an open PR.** A `Completed` session can lack a PR URL if `gh pr create` failed — check session logs or run `gh pr create` manually.
- **`--all` runs sequentially** in the CLI regardless of `cruise config --set-parallelism` (that value only governs the **desktop GUI**).
- **Hot-reload:** during `cruise run`, the config is re-read between steps when its mtime changes — tweak prompts mid-run without restarting (only for external configs, and the current step must still exist).
- **Rate limits (HTTP 429)** retry with exponential backoff (2s → 60s), default 5 tries; tune with `--rate-limit-retries`. Loop edges are bounded by `--max-retries` (default 10).
- **Stuck session?** `cruise list` → the session → **Reset to Planned** to restart it cleanly, or **Resume** to continue a `Running`/`Suspended` one.

## Common recipes

```sh
# Fire-and-forget: queue several plans in the background, approve later from `list`
cruise --plan "add retry to the uploader"
cruise --plan "migrate config to XDG paths"
cruise list                      # review/approve each when ready

# I wrote the plan myself; just run it
cruise plan --skip-planning "$(cat my-plan.md)"
cruise run

# Drain the queue
cruise run --all                 # every Planned session, worktree mode, summary table at the end

# Throwaway run against the current branch, no PR
cruise exec "tidy up the imports in src/"

# Preview without executing
cruise run --dry-run

# Feed session state to a script
cruise list --json | jq '.[] | select(.phase=="Failed")'

# Garbage-collect merged/closed work
cruise clean
```
