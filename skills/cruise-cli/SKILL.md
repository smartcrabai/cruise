---
name: cruise-cli
1: description: Use when running, operating, or troubleshooting the `cruise` CLI or its default interactive keyboard client — the YAML-driven coding-agent workflow orchestrator that wraps coding-agent CLIs. Covers command selection (plan / --plan / draft / run / exec / list / clean / config / login), planning variants (--skip-planning / --grill / --formal-spec / --no-interactive-planning / --image / --repo), bounded `run --all --parallelism` execution, the live dashboard, the TUI's GUI-domain session workflows, session lifecycle and phases, workspace modes, config resolution, and runtime file layout. Trigger whenever the user asks how to start a cruise session, run or resume a workflow, manage or clean sessions, use the TUI, pick a workspace mode, or debug why a session is…
2: | Show / change app-level settings (e.g. GUI/TUI parallelism) | `cruise config` |
| Sign the default `jcode` backend in to a provider / store an API key / inspect what's configured | `cruise login` (TTY menu) / `cruise login <provider> --api-key` / `cruise login --status` |
---

cruise is a CLI that drives coding-agent CLIs (like `claude -p`) through a declarative YAML workflow: **plan → approve → run (write tests → implement → test → review) → open PR → after-pr automation**. This skill is the operator's manual — how to *drive* cruise. For writing the workflow YAML itself, see the **cruise-config** skill.

## Mental model

Work flows through **sessions**, each with a phase. The normal path is:

```
plan/draft  →  [AwaitingInput while `ask_user` waits]  →  AwaitingApproval  →  (approve)  →  Planned  →  run  →  Running  →  Completed  →  clean
```

- A **session** is a unit of work (one task → one plan → one run → usually one PR).
- **AwaitingInput** means SDK planning has persisted an unanswered `ask_user` question. Answering it resumes planning; `cruise list` can restart plan generation with **Generate Plan**.
- `cruise plan`/`--plan`/`draft` *create* sessions; `cruise run` *executes* them; `cruise list` *manages* them; `cruise clean` *garbage-collects* them.
- `cruise exec` is the **odd one out**: it runs a workflow against the current directory with a **transient session**, no worktree, and no PR. Terminal exec sessions are removed automatically; paused or interrupted sessions remain resumable by ID. `force_exec: true` enables the same path for direct plan entry points; `--no-force-exec` opts out once.
- Bare `cruise` opens the official interactive keyboard client for the full GUI-domain session workflows; use it for interactive management, while `cruise list` remains available as the CLI selector.

## Which command do I want?

| Goal | Command |
|------|---------|
| Plan a task, then approve it interactively (foreground) | `cruise plan "task"` |
| Plan in the background, review later, return immediately | `cruise --plan "task"` |
| I already wrote the plan myself — skip the LLM planning | `cruise plan --skip-planning "<plan text>"` (or `cruise --plan "…" --skip-planning`) |
| Interview me one question at a time, then write the plan | `cruise plan --grill "task"` (SDK backend (the default) + TTY) |
| Add Quint and Alloy formal specifications to the initial plan | `cruise plan --formal-spec "task"` |
| Disable SDK planning tools and have the agent write `plan.md` directly | `cruise plan --no-interactive-planning "task"` |
| Attach planning images | `cruise plan --image screenshot.png "task"` (repeat `--image` as needed) |
| Target a GitHub repo instead of a local directory | `cruise plan --repo owner/repo "task"` (also with `--plan`) |
| Just capture an idea now, plan later | `cruise draft "task"` |
| Execute the next approved (Planned) session | `cruise run` |
| Execute a specific session | `cruise run <session-id>` |
| Run a config right here, no plan/worktree/PR | `cruise exec "task"` (or `cruise "task"` with `force_exec: true`) |
| Execute every Planned or Suspended session back-to-back (live dashboard on TTY for non-dry runs) | `cruise run --all` |
| Run all planned or suspended sessions with bounded concurrency (one run) | `cruise run --all --parallelism 4` |
| Manage sessions interactively in the full keyboard client | `cruise` |
| Browse / approve / resume / delete sessions with the CLI selector | `cruise list` |
| Dump session state for scripts | `cruise list --json` |
| Automate, emit JSON, or run in CI | Existing CLI commands, especially `cruise list --json` and `cruise run` |
| Delete sessions whose PR is merged/closed or that are terminal no-PR exec/current-branch remnants | `cruise clean` |
1: description: Use when running, operating, or troubleshooting the `cruise` CLI or its default interactive keyboard client — the YAML-driven coding-agent workflow orchestrator that wraps coding-agent CLIs. Covers command selection (plan / --plan / draft / run / exec / list / clean / config / login), planning variants (--skip-planning / --grill / --formal-spec / --no-interactive-planning / --image / --repo), bounded `run --all --parallelism` execution, the live dashboard, the TUI's GUI-domain session workflows, session lifecycle and phases, workspace modes, config resolution, and runtime file layout. Trigger whenever the user asks how to start a cruise session, run or resume a workflow, manage or clean sessions, use the TUI, pick a workspace mode, or debug why a session is…
2: | Show / change app-level settings (e.g. GUI/TUI parallelism) | `cruise config` |
| Sign the default `jcode` backend in to a provider / store an API key / inspect what's configured | `cruise login` (TTY menu) / `cruise login <provider> --api-key` / `cruise login --status` |
| See what *would* run without executing | add `--dry-run` to `plan` / `run` / `exec` |

### Login

`cruise login` with no arguments opens Cruise's action menu when stdin, stdout, and stderr are TTYs. Choose **Sign in or configure a provider** to hand provider selection, OAuth, and API-key flows to jcode, **Store an API key directly** to enter a provider id and, unless `CRUISE_LOGIN_API_KEY` is set, a hidden key, or **View authentication status** to inspect the private jcode home. Esc or **Exit** closes the menu. Explicit `cruise login <provider>`, `cruise login <provider> --api-key`, and `cruise login --status` remain one-shot shortcuts for scripts and automation. TTY colors honor `NO_COLOR`; non-TTY or redirected invocation skips the menu and decoration and delegates directly to jcode.

> **Legacy shortcut:** `cruise "task"` with positional input and no subcommand is treated as `cruise plan "task"`. Piping (`echo "task" | cruise`) feeds the task on stdin.

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

   After **Approve** or **Execute now**, the step-skip selector lets you omit steps for this run; cancelling it returns to the action menu without approving or executing.

3. **Run.** `cruise run` picks up a `Planned` session, prompts for a **workspace mode** (below), reuses/creates the worktree, executes the workflow steps, creates a PR with `gh pr create`, then runs any `after-pr` steps. The session ends as `Completed` (or `Failed`). Transient exec sessions are excluded from automatic selection and `run --all`; resume them only with an explicit ID.

4. **Clean up.** `cruise clean` checks each `Completed` session's PR via `gh pr view` and deletes the session + worktree (and any leftover `--repo` clone) once the PR is merged or closed.

### `--skip-planning`

`--skip-planning` skips the planning call: the trimmed input (plus stored attachment paths, when present) is written to `plan.md`; empty/whitespace input is rejected. Foreground TTY use still opens the approval menu. Foreground non-TTY use auto-approves to `Planned`; background `cruise --plan … --skip-planning` also creates a `Planned` session immediately. SDK-mode foreground approval makes a separate `generate_title` call; background skip-planning and command mode derive the title from `plan.md`.

### `--grill` (interview planning)

`cruise plan --grill "task"` turns the plan step into an interview: the SDK agent asks you questions **one at a time** (via its `ask_user` tool), recommending an answer for each, until scope, edge cases, and the approach are pinned down — then writes `plan.md`. Constraints: requires the **SDK backend** (the default `jcode` backend qualifies; a `command:` config does not) and an **interactive terminal**; cruise errors out and discards the session otherwise. Conflicts with `--skip-planning`, and only affects the *initial* plan — Fix/Ask, replan, drafts, and background `--plan` use the standard prompt. The GUI equivalent is the **"Grill me"** toggle on the New Session form.

### `--no-interactive-planning`

`cruise plan --no-interactive-planning "task"` disables the SDK planning tools (`submit_plan`, `update_plan`, `ask_user`) for this invocation and asks the agent to write `plan.md` directly. Use it with tool-incapable providers. It conflicts with `--grill`; the GUI equivalent is **Non-interactive planning**.

### `--formal-spec`

`cruise plan --formal-spec "task"` keeps the ordinary Markdown implementation plan and adds both Quint and Alloy formal specifications. The initial-plan prompt requires valid syntax, semantic comments in the configured plan language, faithful meaning preservation, internally consistent state/transition models, invariant preservation, and reachable requested final states. It is off by default, may be combined with `--grill` or `--no-interactive-planning`, and works with command and SDK backends without requiring a TTY or SDK tools. It conflicts with `--skip-planning`, which uses the input verbatim. The TUI and desktop New Session forms provide the same toggle. The CLI and standard desktop/TUI workflows expose it only for the initial foreground plan request: it is not persisted and those workflows do not apply it to background `cruise --plan`, Fix, Ask, Replan, or generating a plan for an existing draft. The lower-level application API carries `formalSpec` on `Generate` requests and honors it for callers that invoke that operation directly, including eligible existing-draft Generate calls. When `force_exec: true` is configured, explicitly passing `--formal-spec` selects normal plan generation for that invocation.

### `--image`

`cruise plan --image screenshot.png "task"` attaches a PNG, JPG/JPEG, WebP, or GIF; repeat the flag for multiple files. Interactive plan input also detects dragged/pasted image paths. Cruise copies attachments into the session so paths remain stable and appends those stored paths to the planning input. Image attachments disable `force_exec` for that invocation so planning can consume them.

### `--repo` (GitHub repo sessions)

`cruise plan --repo owner/repo "task"` (also `cruise --plan "task" --repo owner/repo`) targets a GitHub repository instead of the current directory. cruise clones it via `gh repo clone` into `$XDG_DATA_HOME/cruise/clones/<session-id>/` and uses the clone as the session's base dir, so worktrees and PR creation work unchanged. Lifecycle: clone → plan → **clone removed on approval** (branch kept) → `cruise run` re-clones → execute → PR created → **clone removed again**. On failure/suspend the clone is kept so the session can resume; PR-creation failure marks the session `Failed` (retryable). Repo sessions are **pinned to worktree mode** (no current-branch option — the PR is the only output), and the resolved config (including the built-in default when no config file is found) is copied to `sessions/<id>/config.yaml` so it survives clone removal (including inlined `prompt_file` contents). The GUI equivalent is the **Directory / GitHub Repository** source toggle (repository picker backed by `gh repo list`).

## Workspace modes (chosen at `cruise run`)

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| Mode | What it does | When to use |
|------|--------------|-------------|
| **Worktree** (default) | Isolated git worktree under `$XDG_DATA_HOME/cruise/worktrees/<id>/`, new branch `cruise/<id>-<slug>`, auto-PR via `gh`. | The normal choice. Keeps your working copy untouched and supports independent PR-backed sessions. **Requires `gh` CLI.** |
| **Current branch** | Runs in place on the active branch. No worktree, no auto-PR. | Quick iterations on the current branch. Normal runs need a **clean working tree** and an **attached branch** (not detached HEAD); `exec`/`force_exec` may start dirty. On resume the branch must match. |

Non-interactive runs (piped stdin), `cruise run --all`, and `--repo` sessions always force worktree mode (for `--repo` the prompt is skipped entirely). Repo sessions snapshot the resolved workflow config before clone cleanup, so workflow-call expansions remain usable.

**Copy files into the worktree** by listing relative paths in a `.worktreeinclude` at the repo root (e.g. `.env`, `secrets/`). Absolute paths and `..` are ignored for safety.

## `cruise list` — phase → available actions

The interactive menu changes with the session's phase:

| Phase | Actions |
|-------|---------|
| **Draft** | Generate Plan, Delete, Back |
| **Awaiting Input** | Generate Plan, Delete, Back |
| **Awaiting Approval** | Approve, Publish as Issue, Edit Settings, Delete, Back |
| **Planned** | Run, Publish as Issue, Edit Settings, Replan, Delete, Back |
| **Running** | Resume, Reset to Planned, Delete, Back |
| **Suspended** | Resume, Edit Settings, Reset to Planned, Delete, Back |
| **Failed** | Run, Edit Settings, Reset to Planned, Delete, Back |
| **Completed** | Open PR*, Reset to Planned, Delete, Back |
| **Planning** / **Plan Failed** | Edit Settings, Delete, Back (Approve/Publish as Issue appear only once a non-empty `plan.md` exists and planning has no error) |

\* Open PR shows only when the session has a PR URL.

- **Reset to Planned** clears the current step so the session re-runs from the start — the go-to recovery for a wedged `Running`/`Failed` session.
- **Replan** regenerates the plan from feedback while staying `Planned`.
- **Publish as Issue** publishes `plan.md` verbatim as a GitHub issue in the resolved repo, then deletes the local session. Optionally posts a follow-up `@cruise run` comment to trigger the Actions workflow (default off for `AwaitingApproval`, on for `Planned`, since publishing a `Planned` session replaces running it locally). If the comment fails to post, the issue stays but the local session is kept for a retry, which reuses that issue instead of creating a duplicate.

## `cruise` — interactive keyboard client

```sh
cruise
```

Typical flow: run `cruise`, press `n` to open **New Session** at the task question, type the task, then either press `Ctrl-P` for normal planning, `Ctrl-G` for grill planning, or `Ctrl-U` to use the input directly as the plan, or press `Tab` to answer the remaining questions one at a time and pick the launch mode at the end. Press `1` to inspect or act on sessions, then `3` to run the planned queue in **Run All**.

Bare `cruise` opens the official keyboard-only client beside the CLI and desktop GUI. It exposes the GUI's session-management workflows in a terminal. It requires an interactive TTY on macOS or Linux; non-TTY use is not supported. GitHub-backed workflows use the external `gh` CLI; current-branch-only work does not require `gh`.

### TUI screens and workflows

The TUI has exactly three views:

- **Sessions** — Browse the global session list. Each selected session has **Info**, **DAG**, **Plan**, and **Log** detail tabs. The DAG tab shows its node list plus the selected node's dependency and edge details; Markdown is parsed and styled. The view exposes the complete phase action matrix from [`cruise list` — phase → available actions](#cruise-list--phase--available-actions), including Ask and Option prompts, Clean, worktree/current-branch selection, Publish as Issue, and PR links.
- **New Session** — Create a session or draft through a step-by-step dialogue: one question is shown at a time with the answers so far listed above it and the remaining questions below. The questions are the task, images, source (local Directory or GitHub repository), working directory or repository, workflow config, skipped steps, workspace mode, dirty-tree allowance (current-branch runs only), formal specification, and finally the launch mode (normal planning, grill planning, input-as-plan, or save as draft). Questions that earlier answers make moot are skipped. `Ctrl-P`, `Ctrl-G`, `Ctrl-U`, and `Ctrl-S` start or draft the session from any question with the current answers. Directory and path answers offer completion, and history is recalled with the arrow keys; draft and selection history are retained.
- **Run All** — Run Planned or Suspended sessions with live parallelism, in-app status, and bell feedback. It uses the configured `run_all_parallelism`; the CLI's `cruise run --all --parallelism <N>` remains a separate one-run override. Plan and run streams continue while navigating between views.

Ask and Option prompts are handled in the TUI. Required prompts are queued; a single-session prompt opens automatically, while Run All shows a queue badge. PR and Issue URLs are shown as text. Only a dedicated PR/Issue URL action opens a URL, using `open` on macOS or `xdg-open` on Linux; other Markdown links remain textual. The CLI-only `login`, `config`, and `exec` operations remain CLI commands rather than TUI screens.

The New Session dialogue autosaves its answers 500 ms after a change. Other screen state is ephemeral. Destructive, external, and multi-stop actions require confirmation; quitting with active work confirms before cancelling it. Cancelling a run moves its session to `Suspended`; cancelling planning restores the prior state and plan. Empty text answers are rejected. Terminal state is restored on normal exit, panic, SIGTERM, and SIGHUP. Session errors stay in the app and session state; only terminal/root/event-loop failures exit the TUI.

### TUI keyboard map

Keys are fixed and cannot be configured:

| Key | Action |
|-----|--------|
| `1` / `2` / `3` | Switch to Sessions / New Session / Run All |
| `n` | Open New Session at its first question, the task, ready to type |
| `r` | Refresh |
| `?` | Show help |
| `q` / `Ctrl-C` | Quit; an active quit confirms and then cancels work |
| `Ctrl-P` | Create the New Session from the current answers and start normal planning |
| `Ctrl-G` | Create the New Session from the current answers and start grill planning |
| `Ctrl-U` | Create the New Session from the current answers using the input directly as the plan |
| `Ctrl-S` | Save the New Session answers as a draft |
| `Tab` / `Shift-Tab` | Next / previous question (Tab completes a path first when one matches); move between detail tabs elsewhere |
| Arrow keys / `j` / `k` / `PgUp` / `PgDn` / `Home` / `End` | Navigate; in the dialogue, move between choices, recall recent directories, discovered configs, or `gh` repositories, or move through the skipped-step list |
| `[` / `]` | Move between detail tabs |
| `a` | Open the action palette |
| `o` | Handle the prompt queue or open a dedicated PR/Issue URL, as the current context dictates |
| `f` | Follow the log |
| `Enter` | Accept the answer and move to the next question; on the launch question, start or draft the session; in the task and image editors, insert a newline |
| `Ctrl-Enter` | Move to the next question from the task or image editor |
| `Space` | Toggle the current choice or the highlighted skipped step |
| `Esc` | Back one question; at the first question, return to Sessions |

### TUI layout, logs, and concurrency

- At **120 or more columns**, the layout has a fixed **34-column sidebar** and a detail pane.
- At **80–119 columns**, it uses a single pane.
- Below **80x24**, it shows a resize notice.
- `NO_COLOR` is honored, and labels/statuses are never conveyed by color alone.
- Idle updates are event-driven; external state is polled every 3 seconds; active work uses a 100 ms spinner.
- In-memory logs are bounded: the session log keeps the latest 10,000 lines and Run All keeps the latest 2,000. Complete per-session output remains at `$XDG_DATA_HOME/cruise/sessions/<session-id>/run.log` (default `~/.local/share/cruise/sessions/<session-id>/run.log`).
- A child process never shares TUI stdin. Interactive child commands therefore cannot prompt through the TUI.
- Distinct sessions may run concurrently within one TUI process, but duplicate work for one session is rejected. Concurrent mutation of the same session by separate cruise processes is unsupported.

Use the CLI as the canonical client for automation, JSON, and CI/non-interactive use. `cruise list` and all existing CLI commands remain documented below; `cruise list --json` is the machine-readable session-state interface.

## Config-file resolution

`cruise run`/`plan`/`exec` resolve the **workflow YAML** in this order:

1. `-c/--config <path>` (must exist; no prompt). The special value `__builtin__` selects the built-in default workflow.
2. `CRUISE_CONFIG` env var (must exist; no prompt)
3. Current dir: `./cruise.yaml` → `.yml` → `./.cruise.yaml` → `./.cruise.yml`, then `./.cruise/*.yaml|*.yml` (ASCII-sorted), then `$XDG_CONFIG_HOME/cruise/workflows/*.yaml|*.yml`. Multiple candidates → interactive picker with a trailing **Built-in default** entry (TTY) or highest-priority auto-pick (non-interactive).
4. None found → a built-in default workflow (`builtin/cruise.yaml` in the source tree, embedded at build time), adopted without prompting.

> To *write* or edit that YAML, switch to the **cruise-config** skill.

## Runtime file layout (XDG)

| Kind | Path (default) |
|------|----------------|
| User workflow YAML configs (`workflows/*.yaml` / `*.yml`) | `$XDG_CONFIG_HOME/cruise/workflows/` → `~/.config/cruise/workflows/` |
| App settings (`config.json`) | `$XDG_CONFIG_HOME/cruise/` → `~/.config/cruise/` |
| Sessions + worktrees + `--repo` clones | `$XDG_DATA_HOME/cruise/` → `~/.local/share/cruise/` (sessions with no filesystem config path, including `-c __builtin__`, keep a `sessions/<id>/config.yaml` snapshot) |
| State (`history.json`, `new_session_draft.json`) | `$XDG_STATE_HOME/cruise/` → `~/.local/state/cruise/` |

> Older versions kept everything under `~/.cruise/`. If migrating, move workflow YAMLs to `~/.config/cruise/workflows/`, `config.json` to `~/.config/cruise/`, `sessions/`+`worktrees/` to `~/.local/share/cruise/`, and use `git worktree move`/`repair` for worktrees. Cruise also warns if workflow YAMLs are left directly in `~/.config/cruise/` (legacy location) instead of the `workflows/` subdirectory.

## Operational notes & gotchas

- **`gh` CLI is required** for worktree mode (PR creation) and PR-backed `cruise clean` checks. Current-branch and `exec` don't need it.
- **`cruise clean` also removes terminal no-PR exec/current-branch sessions** without calling `gh`; resumable sessions and ordinary planned sessions are retained.
- **`--all`** runs Planned or Suspended sessions sequentially by default, regardless of `cruise config --set-parallelism` (that value governs the **desktop GUI and TUI**). If a session state file cannot be reloaded for the final summary, that session is reported as `Failed` with the state path and error, and the batch still completes.
- **`--parallelism <N>`** is a one-run override for `cruise run --all` (default `1`, must be >= 1, requires `--all`). Each session still runs in its own worktree; one failure does not stop the other workers, and Ctrl+C suspends active sessions and stops new scheduling. It never reads or changes the persisted `cruise config --set-parallelism` value (that value governs the **desktop GUI and TUI**).
- In an interactive terminal, a non-dry `cruise run --all` shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output is retained in `sessions/{id}/run.log`. Non-TTY and dry-run invocations keep the normal log output and final summary.
- **Hot-reload:** during `cruise run`, the config is re-read between steps when its mtime changes — tweak prompts mid-run without restarting (only for external configs, and the current step must still exist).
- **Retries:** Without an SDK fallback policy, HTTP 429 uses exponential backoff (2s → 60s), default 5 tries; an SDK `retry:` block or workflow-level model array with fallback entries also makes 5xx/network failures retryable and can switch to fallback models using the same `--rate-limit-retries` budget. Loop edges are bounded by `--max-retries` (default 3).
- **Stuck session?** `cruise list` → the session → **Reset to Planned** to restart it cleanly, or **Resume** to continue a `Running`/`Suspended` one.

## Common recipes

```sh
# Fire-and-forget: queue several plans in the background, approve later from `list`
cruise --plan "add retry to the uploader"
cruise --plan "migrate config to XDG paths"
cruise                            # review/approve each when ready

# I wrote the plan myself; just run it
cruise plan --skip-planning "$(cat my-plan.md)"
cruise run

# Drain the queue
cruise run --all                 # every Planned or Suspended session, worktree mode, live dashboard on TTY for non-dry runs, summary at the end

# Drain the queue with bounded concurrency (this invocation only)
cruise run --all --parallelism 4

# Throwaway run against the current branch, no PR
cruise exec "tidy up the imports in src/"

# Preview without executing
cruise run --dry-run

# Feed session state to a script
cruise list --json | jq '.[] | select(.phase=="Failed")'

# Garbage-collect merged/closed work
cruise clean
```
