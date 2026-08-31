# cruise

<p align="center">
  <img src="src-tauri/icons/icon.png" alt="cruise logo" width="160" height="160">
</p>

A CLI tool that orchestrates coding agent workflows defined in a YAML config file.

Cruise wraps CLI coding agents such as `claude -p` and drives them through a declarative workflow: plan -> approve -> write tests -> implement -> test -> review -> open PR -> post-PR automation. It handles variable passing between steps, conditional branching, and loop control.

> **Note:** This project supports macOS and Linux only. **Windows is not supported** and Windows binaries are not built or tested. Development and testing happen primarily on macOS; Linux has not been fully verified.

## Prerequisites

- [`gh` CLI](https://cli.github.com/) -- required for worktree mode (PR creation and cleanup). Not needed when using current-branch mode.
- [`jcode` CLI](https://github.com/1jehuang/jcode) v0.81.1 or newer -- required by the default SDK backend (`sdk: jcode`); sign in with `cruise login` after installing. Not needed when every config you run uses `command:` or `sdk: claude`.
- [`claude` CLI](https://code.claude.com/docs/en/quickstart) -- required only by `sdk: claude`, which drives it in-process. Verified against 2.1.250; a `:effort` model suffix maps to `claude --effort`, so a CLI without that flag fails the step with `unknown option '--effort'` (a permanent error, not retried). Authentication is the CLI's own, not `cruise login`.

## Installation

### cargo install

```sh
cargo install cruise
```

### Homebrew

```sh
brew install smartcrabai/tap/cruise
```

### GUI (Desktop App)

A desktop GUI is also available. Download the latest installer from [GitHub Releases](https://github.com/smartcrabai/cruise/releases):

| Platform | Format |
|----------|--------|
| macOS (Apple Silicon) | `.dmg` |
| Linux (x86_64) | `.deb`, `.AppImage` |

#### macOS GUI Installation

After downloading the DMG and copying `cruise.app` to `/Applications`, run the following in Terminal before the first launch:

```sh
xattr -cr /Applications/cruise.app
```

This removes the Gatekeeper quarantine attribute, allowing the app to launch.

## Usage

```sh
# Create a session (plan -> approve)
cruise plan "implement the feature"

# Create a session and generate the plan in the background
cruise --plan "implement the feature"

# Interview-style planning: answer one question at a time, then the plan is written (SDK backend + TTY)
cruise plan --grill "implement the feature"

# Plan against a GitHub repository instead of a local directory (temporary clone)
cruise plan --repo owner/repository "implement the feature"

# Background planning from stdin
echo "implement the feature" | cruise --plan stdin

# Save the task as a draft (no plan yet); generate the plan later from `cruise list`
cruise draft "implement the feature"

# Execute the approved session
cruise run

# Execute a config directly in the current directory (no plan, no worktree, no PR)
cruise exec "do this"

# Open the full interactive keyboard client
cruise tui

# List and manage sessions with the CLI selector
cruise list

# Remove closed/merged PR sessions and terminal no-PR sessions
cruise clean

# Legacy: no subcommand is treated as `cruise plan`
cruise "implement the feature"
```

### TUI (Interactive Keyboard Client)

`cruise tui` is cruise's official interactive keyboard client: the third client beside the CLI and desktop GUI. It preserves the existing CLI behavior while bringing the GUI's session-management workflows to a terminal.

```sh
# Open the interactive client
cruise tui
```

Typical flow: run `cruise tui`, press `2` to create a session from **New Session**, press `1` to inspect or act on it in **Sessions**, then press `3` to run the planned queue in **Run All**.

The TUI requires an interactive TTY on macOS or Linux. It is an interactive client, not an automation interface: use the CLI for automation, JSON output, and CI. GitHub-backed workflows use the external [`gh` CLI](https://cli.github.com/); the TUI does not bundle or replace it. Current-branch-only work does not require `gh`.

#### Screens and workflows

The TUI has three views:

- **Sessions** -- Browse the global session list. Select a session to view its **Info**, **DAG**, **Plan**, or **Log** detail tab and use the full phase action matrix documented under [`cruise list` Actions](#cruise-list-actions). The DAG tab shows its node list plus the selected node's dependency and edge details; Markdown is parsed and styled. This includes Ask and Option prompts, Clean, worktree/current-branch selection, Publish as Issue, and PR links.
- **New Session** -- Create a session or draft from a local Directory or GitHub source. The form supports workflow config selection, skipped steps, task text and images, **Use input as plan**, **Grill me**, and **Non-interactive planning**. Directory and repository paths are typed with completion; draft and selection history are retained as described in [New Session Form Persistence](#new-session-form-persistence).
- **Run All** -- Run Planned or Suspended sessions with live parallelism, in-app status, and bell feedback. Distinct sessions may run concurrently in one TUI process; duplicate work for one session is rejected.

PR and Issue URLs are shown as text. The dedicated PR/Issue URL action opens them with `open` on macOS or `xdg-open` on Linux; other Markdown links remain textual. CLI-only `login`, `config`, and `exec` operations remain available through their CLI commands rather than TUI screens.

The New Session form autosaves 500 ms after a change. Other screen state is ephemeral. Required prompts are queued; a single-run prompt opens automatically, while Run All shows a queue badge. Destructive, external, and multi-stop actions ask for confirmation. Quitting while work is active confirms before cancelling it. Cancelling a run moves its session to `Suspended`; cancelling planning restores the prior state and plan. Empty text answers are rejected. Terminal state is restored on normal exit, panic, SIGTERM, and SIGHUP. Session errors stay in the app and session state; only terminal/root/event-loop failures exit the TUI.

#### Keyboard map

The TUI is keyboard-only. Keys are fixed and cannot be configured:

| Key | Action |
|-----|--------|
| `1` / `2` / `3` | Switch to Sessions / New Session / Run All |
| `r` | Refresh |
| `?` | Show help |
| `q` / `Ctrl-C` | Quit; an active quit confirms and then cancels work |
| `Tab` / `Shift-Tab` | Move focus forward / backward |
| Arrow keys / `j` / `k` / `PgUp` / `PgDn` / `Home` / `End` | Navigate |
| `[` / `]` | Move between detail tabs |
| `a` | Open the action palette |
| `o` | Handle the prompt queue or open a dedicated PR/Issue URL, as the current context dictates |
| `f` | Follow the log |
| `Enter` | Edit or commit a single-line field; in a multiline field, insert a newline |
| `Space` (not editing) | Toggle the source or focused option, cycle available directory/config/repository choices, or toggle the selected skipped step |
| `Esc` | Leave edit mode |
| Focused submit button | Submit the current form |

#### Layout, logs, and process behavior

- At **120 or more columns**, the layout uses a fixed **34-column sidebar** and a detail pane.
- At **80--119 columns**, the layout becomes a single pane.
- Below **80x24**, the TUI shows a resize notice.
- `NO_COLOR` is honored; labels and statuses are never conveyed by color alone.
- Idle updates are event-driven. External state is polled every 3 seconds, and active work uses a 100 ms spinner.
- The in-memory session log is bounded to the latest 10,000 lines; the Run All view is bounded to the latest 2,000 lines. Complete per-session run output remains at `$XDG_DATA_HOME/cruise/sessions/<session-id>/run.log` (by default `~/.local/share/cruise/sessions/<session-id>/run.log`).
- The TUI cannot hand its stdin to an interactive child command: child processes never share TUI stdin, so such commands cannot prompt through the TUI.
- Concurrent mutation of the same session by separate cruise processes is unsupported. Within one TUI process, distinct sessions can run concurrently, and Run All uses the configured run-all parallelism.

The CLI remains the canonical client for automation, JSON (`cruise list --json`), and CI/non-interactive use. See the [CLI Reference](#cli-reference) for the existing command workflows.

### CLI Reference

```
cruise [OPTIONS] [INPUT] [COMMAND]

Commands:
  plan         Create an implementation plan for a task
  draft        Save a task description as a draft without generating a plan
  run          Execute a planned session
  exec         Execute the workflow config directly in the current directory
  tui         Open the interactive keyboard client for session management
  list         List and manage sessions interactively
  clean        Remove sessions with closed/merged PRs or terminal no-PR sessions
  config       Show or update application-level configuration
  login        Sign cruise's default SDK backend (jcode) in to a provider

Options:
      --plan <INPUT>           Create a plan in the background and return immediately
      --no-force-exec          Ignore force_exec: true and plan as usual
```

#### `cruise tui`

```
cruise tui
```

Opens the keyboard-only TUI. It requires a macOS/Linux interactive TTY; see [TUI (Interactive Keyboard Client)](#tui-interactive-keyboard-client) for its screens, workflows, keymap, responsive layout, and log behavior. Use the existing CLI commands for automation, JSON, and CI.

#### `cruise plan`

```
cruise plan [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description

Options:
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ for the built-in default (see Config File Resolution)
      --dry-run                    Print the plan step without executing it
      --no-force-exec              Ignore force_exec: true and plan as usual
      --skip-planning              Use the input directly as the plan, skipping LLM-based plan generation
      --grill                      Interview-style planning: the agent asks one question at a time, then writes the plan (requires the SDK backend and a TTY; conflicts with --skip-planning)
      --no-interactive-planning    Disable interactive planning tools for this session; the agent writes plan.md directly (conflicts with --grill)
      --repo <OWNER/REPO>          GitHub repository to clone into a temporary directory for planning and execution
      --rate-limit-retries <N>     Maximum number of rate-limit retries per LLM call [default: 5]
```

`cruise plan` creates an isolated git worktree at `$XDG_DATA_HOME/cruise/worktrees/<session-id>/` before invoking the LLM, so plan-phase edits never touch your working copy. The same worktree is reused by `cruise run` in Worktree mode, or cleaned up automatically when you pick Current-branch mode or cancel planning. Non-git directories fall back to running in place with a warning.

With `--skip-planning`, no LLM is called: the (trimmed) input is written straight to `plan.md` and the session goes directly to `Planned`, ready for `cruise run` with no approval step. Empty or whitespace-only input is rejected. Use this when you've already written the plan yourself and just want cruise to execute it. The desktop GUI exposes the same behavior via the **"Use input as plan (skip LLM planning)"** checkbox on the New Session form (the submit button changes from "Generate plan" to "Create session").

With `--grill`, the plan step becomes an interview: instead of writing the plan in one shot, the SDK agent asks you questions **one at a time** (via the `ask_user` tool) — recommending an answer for each — until scope, edge cases, and the implementation approach are fully pinned down, and only then writes `plan.md`. It requires an SDK backend -- `sdk: jcode`, `sdk: claude`, or a config that names neither `sdk:` nor `command:` and therefore runs on the default `jcode` backend -- plus an interactive terminal; cruise errors out (and discards the session) otherwise. `--grill` conflicts with `--skip-planning` and applies only to initial plan generation — Fix/Ask turns, replans, drafts, and background planning use the standard prompt. The desktop GUI exposes the same behavior via the **"Grill me"** toggle on the New Session form (mutually exclusive with "Use input as plan").

With `--no-interactive-planning`, the interactive planning tools (`submit_plan` / `update_plan` / `ask_user`) are disabled for this session even if the workflow config has `interactive_planning: true`. The agent writes `plan.md` directly instead — exactly like the `command` backend. The flag conflicts with `--grill` (which requires the interactive tools). It is equivalent to setting `interactive_planning: false` in the workflow config but only affects the current session. The desktop GUI exposes the same behavior via the **"Non-interactive planning"** checkbox on the New Session form (mutually exclusive with "Grill me").

With `--repo <owner>/<repository>`, the session targets a GitHub repository instead of the current directory. The repository is cloned via `gh repo clone` into `$XDG_DATA_HOME/cruise/clones/<session-id>/`, which becomes the session's base directory, so the existing worktree and PR machinery work on the clone unchanged. The clone is removed once the plan is approved (the branch name is kept), re-created by `cruise run`, and removed again after the PR has been created; on failure or suspend it is kept so the session can be resumed or retried (PR-creation failure marks the session `Failed`, not `Completed`). Repo sessions always run in Worktree mode — the no-PR current-branch mode is not available — and the resolved workflow config (including the built-in default when no config file is found) is copied to `sessions/<session-id>/config.yaml` so it stays readable after the clone is removed (including inlined `prompt_file` contents). `--repo` also works with background planning (`cruise --plan "task" --repo owner/repository`). The desktop GUI exposes the same behavior via the **Directory / GitHub Repository** source toggle on the New Session form, with a repository picker backed by `gh repo list` (free-form `owner/repository` input is accepted too).

#### `cruise draft`

```
cruise draft [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description (omit to prompt interactively; reads from stdin when piped)

Options:
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ for the built-in default
```

Saves the input as a `Draft` session without invoking the LLM. The plan can be generated later by choosing **Generate Plan** from `cruise list`. Useful when you have an idea you want to capture immediately but don't want to start (or pay for) planning yet.

#### `cruise run`

```
cruise run [OPTIONS] [SESSION]

Arguments:
  [SESSION]  Session ID to execute (if omitted, picks from pending sessions)

Options:
      --all                        Run all planned or suspended sessions (live dashboard on interactive terminals for non-dry runs)
      --parallelism <N>            Max number of sessions `--all` executes concurrently (must be >= 1; default: 1)
      --max-retries <N>            Maximum number of times a single loop edge may be traversed [default: 3]
      --rate-limit-retries <N>     Maximum number of rate-limit retries per step [default: 5]
      --dry-run                    Print the workflow flow without executing it
      --cleanup-after-pr           Delete local worktree and branch after PR creation
      --no-cleanup-after-pr        Keep local worktree and branch after PR creation
```

`--all` runs every Planned or Suspended session in sequence by default. Worktree mode is always forced (even if the session was originally started in current-branch mode). After all sessions finish, a summary table is printed showing the outcome and PR link for each session. If a session state file cannot be reloaded for the summary, that session is reported as `Failed` with the state path and error, and the batch still completes. `--all` and `[SESSION]` are mutually exclusive.

`--parallelism <N>` is an invocation-scoped override that runs up to `N` sessions concurrently during `--all`. It defaults to `1` (sequential) when omitted, must be at least `1`, and is rejected unless `--all` is present. Each concurrent session still runs in its own worktree, failures in one session do not stop the other workers, and Ctrl+C suspends the running sessions and stops scheduling new ones. This flag does not read or modify the persisted GUI/TUI setting (`cruise config --set-parallelism`).

When stderr is an interactive terminal and `--dry-run` is not set, `run --all` shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output remains in that session's `sessions/{id}/run.log`. In non-TTY environments such as CI, or during `--dry-run`, it keeps the normal log output and final summary behavior.

#### `cruise exec`

```
cruise exec [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description bound to {input} (optional if your config doesn't reference {input})

Options:
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ for the built-in default
      --max-retries <N>            Maximum number of times a single loop edge may be traversed [default: 3]
      --rate-limit-retries <N>     Maximum number of rate-limit retries per step [default: 5]
      --dry-run                    Print the workflow flow without executing it
```
Runs the workflow steps directly in the current directory: no plan is generated, no git worktree is created, and no PR is opened automatically. A transient session is recorded while the workflow runs, then automatically removed when it reaches a terminal phase. Sessions paused for input (`Running`) or interrupted with Ctrl+C (`Suspended`) are kept and can be resumed with `cruise run <id>`; exec sessions are excluded from `cruise run` automatic selection and `cruise run --all`. Existing uncommitted changes are allowed; cruise runs on top of them without stashing, committing, or resetting the working tree, and warns that workflow-generated files may be mixed with those changes. An attached branch is still required.

When `force_exec: true` is set in the workflow config, `cruise "task"`, `cruise plan "task"`, and `cruise --plan "task"` use the current-directory execution path without planning, worktree, or PR creation. `--no-force-exec`, `--repo`, `--grill`, and image attachments opt out; `--skip-planning` and `--no-interactive-planning` do not. Background `--plan` runs foreground because there is no plan worker for direct execution.

#### `cruise --plan`

```
cruise --plan <INPUT|stdin> [--skip-planning] [--repo <OWNER/REPO>]
```

Creates the session immediately, starts plan generation in a detached worker, and returns the new session ID. While the worker is still running, `cruise list` shows the session as `Planning`. If generation fails, the session remains in `AwaitingApproval` phase internally but `cruise list` shows `Plan Failed`, and approval stays disabled until planning succeeds.

Adding `--skip-planning` skips the background worker entirely: the input is written directly as `plan.md` and the session is created already in `Planned` — no approval step needed. The flag also works without `--plan` (e.g. `cruise --skip-planning "task"`), in which case it behaves like `cruise plan --skip-planning "task"`.

`--repo <owner>/<repository>` is accepted here too and behaves as described under [`cruise plan`](#cruise-plan): the repository is cloned into a temporary directory and the session targets the clone. `--grill` is not available on this path — background planning has no interactive user to interview.

#### `cruise list`

```
cruise list [OPTIONS]

Options:
      --json   Print all sessions as a JSON array to stdout instead of opening the interactive selector
```

With no flags, opens an interactive session browser whose menu depends on each session's phase (see [`cruise list` Actions](#cruise-list-actions)). With `--json`, prints every session as a JSON array (id, phase, input, PR URL, plan-error info, ...) and exits -- useful for scripting or feeding session state to external tooling.

#### `cruise config`

```
cruise config [OPTIONS]

Options:
      --set-parallelism <N>
          Set the maximum number of sessions the desktop GUI and TUI run concurrently in `run --all` mode.

          Must be >= 1. Omit to show the current configuration. The CLI always runs `run --all` sequentially.
```

Shows or updates application-level settings stored in `$XDG_CONFIG_HOME/cruise/config.json` (default: `~/.config/cruise/config.json`) -- this is separate from the per-workflow YAML configs. With no flags, prints the current configuration. `--set-parallelism <N>` sets `run_all_parallelism` (default `1`), which controls how many sessions the **desktop GUI and TUI** execute in parallel during `run --all`. The CLI ignores this setting; use the one-shot `cruise run --all --parallelism <N>` flag instead.

#### `cruise login`

```
cruise login [OPTIONS] [PROVIDER]

Arguments:
  [PROVIDER]  Provider to sign in to (e.g. `claude`, `openai`, `anthropic-api`); omit for jcode's interactive picker

Options:
      --api-key  Store an API key for PROVIDER non-interactively instead of running the OAuth flow
                 (key read from `CRUISE_LOGIN_API_KEY`, an echo-less prompt, or piped stdin; requires PROVIDER)
      --status   List the providers configured in cruise's jcode home and the models available to them
```

Manages credentials for the default `sdk: jcode` backend. Everything is stored in cruise's own jcode home (`$XDG_DATA_HOME/cruise/jcode-home`, default `~/.local/share/cruise/jcode-home`), never in your `~/.jcode` and never in a cruise config file. `cruise login` hands the terminal to `jcode login` (interactive picker / OAuth flow); `--api-key` feeds a key to jcode's storage without exposing it on a command line and requires the `PROVIDER` argument (`cruise login --api-key anthropic-api`). See [SDK Mode](#sdk-mode).

#### `cruise clean`

```
cruise clean
```

Checks each Completed session's PR status via `gh pr view`. Sessions whose PR is closed or merged are deleted along with their worktrees (and any leftover `--repo` clone). Terminal exec/current-branch sessions that cannot have a PR, including legacy exec remnants, are deleted without a GitHub status check. Suspended sessions and ordinary planned sessions remain available for resumption.

> **Note:** A session may lack a PR URL if `gh pr create` failed or was not reached. PR-backed sessions without a PR URL are retained; inspect the session logs or re-run PR creation manually with `gh pr create`.

## Session Management

Cruise stores session data in `$XDG_DATA_HOME/cruise/sessions/` (default: `~/.local/share/cruise/sessions/`). Sessions whose workflow has no filesystem config path—including an explicit `-c __builtin__` selection—store the resolved YAML in `sessions/<session-id>/config.yaml` so they remain runnable without rediscovery.

### Runtime File Layout

Cruise follows the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/) and splits its runtime files across three directories:

| Kind | Path |
|------|------|
| User workflow YAML configs (`workflows/*.yaml` / `*.yml`) | `$XDG_CONFIG_HOME/cruise/workflows/` (default: `~/.config/cruise/workflows/`) |
| Application settings (`config.json`) | `$XDG_CONFIG_HOME/cruise/` (default: `~/.config/cruise/`) |
| Sessions, worktrees, and temporary `--repo` clones | `$XDG_DATA_HOME/cruise/` (default: `~/.local/share/cruise/`) |
| State files (`history.json`, `new_session_draft.json`) | `$XDG_STATE_HOME/cruise/` (default: `~/.local/state/cruise/`) |

> **Migrating from `~/.cruise/`?** Earlier versions stored everything under `~/.cruise/`. Move `*.yaml`/`*.yml` into `~/.config/cruise/workflows/`, `config.json` into `~/.config/cruise/`, `sessions/` and `worktrees/` into `~/.local/share/cruise/`, and `history.json`/`new_session_draft.json` into `~/.local/state/cruise/`. Use `git worktree move` (or `git worktree repair`) when relocating worktree directories.
>
> **Workflow configs previously in `~/.config/cruise/*.yaml` / `*.yml`?** Automatic user-config discovery now looks for workflow YAMLs only in the `workflows/` subdirectory. Move them to `~/.config/cruise/workflows/`; cruise prints a warning if it finds YAML files left directly in `~/.config/cruise/`.

### Session Lifecycle

1. **`cruise plan "task"`** -- Runs the built-in plan step in an isolated planning worktree to generate an implementation plan, then presents an approve-plan menu.
2. **`cruise --plan "task"`** -- Creates the session immediately and generates the plan in the background. Review it later from `cruise list`.
3. **`cruise draft "task"`** -- Records the task as a `Draft` session without running the plan step. Use **Generate Plan** from `cruise list` to start planning when you're ready.
4. **Approve-plan menu** -- Choose one of:
   - **Approve** -- Mark the session as ready to run.
   - **Fix** -- Provide feedback; the plan step reruns with your input.
   - **Ask** -- Ask a question; the answer is shown before the menu reappears.
   - **Execute now** -- Skip approval and run immediately.

   After approving (or choosing "Execute now"), a **step skip selector** is shown if the workflow config defines more than zero steps. A multi-select prompt lists all steps (grouped steps appear as a parent with children); toggle any steps you want to skip for this run. The selection is persisted per config file in `$XDG_STATE_HOME/cruise/history.json` and pre-selected as the default for the next session using the same config. Cancelling the selector returns to the approve-plan menu without approving or executing the session.

5. **`cruise run`** -- Picks up the approved session, reuses (or creates) the git worktree under `$XDG_DATA_HOME/cruise/worktrees/<session-id>/`, executes the workflow steps, automatically creates a PR with `gh pr create`, then runs any configured `after-pr` steps.

Sessions remain in `$XDG_DATA_HOME/cruise/sessions/` until their PR is closed or merged, after which `cruise clean` will remove them.

> **`cruise exec`** is a separate path with a transient lifecycle: it executes in the current directory without planning, worktree creation, or PR creation, and removes its session after terminal completion. Paused or interrupted exec sessions remain resumable by ID. `force_exec: true` enables the same path for direct plan entry points; use `--no-force-exec` to opt out once. See [`cruise exec`](#cruise-exec).

### `cruise list` Actions

The interactive session list shows a menu of actions depending on the session's phase:

| Phase | Available Actions |
|-------|-------------------|
| **Draft** | Generate Plan, Delete, Back |
| **AwaitingApproval** | Approve, Publish as Issue, Edit Settings, Delete, Back |
| **Planned** | Run, Publish as Issue, Edit Settings, Replan, Delete, Back |
| **Running** | Resume, Reset to Planned, Delete, Back |
| **Suspended** | Resume, Edit Settings, Reset to Planned, Delete, Back |
| **Failed** | Run, Edit Settings, Reset to Planned, Delete, Back |
| **Completed** | Open PR*, Reset to Planned, Delete, Back |

\* Open PR is shown only when the session has a PR URL.

`cruise list` may also show `Planning` while `--plan` is still running, or `Plan Failed` when background planning wrote a durable `plan_error`. Those states only offer `Delete` and `Back`; `Approve` and `Publish as Issue` appear only after a non-empty `plan.md` is available.

- **Generate Plan** -- Start planning for a `Draft` session (transitions it through the normal planning flow).
- **Approve** -- Approve the plan and transition the session to the Planned phase.
- **Publish as Issue** -- Publish `plan.md`, unchanged, as a GitHub issue in the resolved repo, then delete the local session. Prompts whether to also post a follow-up `@cruise run` comment so the `@cruise` GitHub Action picks it up (default: off for `AwaitingApproval`, on for `Planned`). If the issue is created but that comment fails to post, the session is kept so you can retry (the existing issue is reused, not duplicated) or comment manually.
- **Run / Resume** -- Execute (or continue) the session.
- **Replan** -- Provide feedback to re-generate the plan; the session stays in the Planned phase.
- **Open PR** -- Open the session's pull request in the browser via `gh pr view --web`.
- **Reset to Planned** -- Reset the session back to the Planned phase, clearing the current step and allowing it to be re-run from the beginning.
- **Delete** -- Permanently remove the session.
- **Back** -- Return to the session list.

## Config File Resolution

cruise resolves the workflow config as follows:

1. **`-c/--config` flag** -- highest priority. The specified file must exist or cruise exits with an error. No prompt is shown. The special value `-c __builtin__` explicitly selects the built-in default workflow (see 4. below) even when config files exist.
2. **`CRUISE_CONFIG` environment variable** -- if set, used directly (error if the file does not exist). No prompt is shown.
3. Otherwise, cruise collects every candidate from the following locations and presents them as choices:
   - `./cruise.yaml` -> `./cruise.yml` -> `./.cruise.yaml` -> `./.cruise.yml` (current directory)
   - `./.cruise/*.yaml` / `*.yml` (current directory), sorted by filename
   - `$XDG_CONFIG_HOME/cruise/workflows/*.yaml` / `*.yml` (default: `~/.config/cruise/workflows/`), sorted by filename

   When stdin and stdout are both TTYs, candidates are shown in an interactive selector and the user picks one. A **Built-in default** entry is always offered at the end of the list, so the built-in default remains selectable even when config files are found; with only that entry present, it is auto-picked. In non-interactive contexts (piped stdin, scripts) the highest-priority candidate is taken automatically without a prompt.
4. **No candidate found** -- cruise falls back to a built-in default workflow (`builtin/cruise.yaml` in the source tree, embedded at build time); no config file is required, but you'll usually want one.

The `description:` field of each config file is shown next to its filename in both the CLI selector and the GUI, making it easier to tell similar files apart. The GUI's config selector offers **Built-in default** alongside *Auto* so a session can be pinned to the embedded default regardless of discovered files.

## Config File Reference

### Basic Structure

```yaml
command:                   # LLM invocation command (mutually exclusive with `sdk`)
  - claude
  - --model
  - "{model}"
  - -p

# sdk: jcode              # alternative to `command`: drive the jcode CLI (this is the default
                          # when neither `command` nor `sdk` is set -- see SDK Mode)
# sdk: claude             # alternative: drive the claude CLI in-process via claude-agent-sdk

description: |             # one-line summary shown next to the filename in selectors (optional)
  Team-shared review-heavy flow with auto-PR.

model: sonnet             # default model for all prompt steps (optional)
plan_model: opus          # model used for the built-in plan step (optional)
languages:                # prompt languages (optional; defaults to English)
  pr: English             # language for auto-generated PR title/body
  plan: English           # language used by built-in planning prompts
# force_exec: false       # execute direct plan entry points in place (use --no-force-exec to opt out)
# Deprecated compatibility fields: pr_language and plan_language

env:                      # environment variables applied to all steps (optional)
  API_KEY: sk-...
  PROJECT: myproject

groups:                   # step group definitions (optional)
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  step_name:
    # step configuration

after-pr:                # optional: steps that run automatically after PR creation
  step_name:
    # step configuration (same format as `steps`)
```

### Dynamic Model Selection

When the `command` array contains a `{model}` placeholder, cruise resolves it at runtime based on the effective model for each step:

- **Model specified** (via top-level `model` or step-level `model`): replaces `{model}` with the model name.
- **No model specified**: removes the `{model}` argument and its immediately-preceding `--model` flag automatically.

A step-level `model` field overrides the top-level `model` default for that step only.

The same Rust-`format!`-style brace escaping used for template variables (see [Variable Reference](#variable-reference)) applies here: `{{model}}` is the literal string `{model}`, not the placeholder. Any other unescaped `{name}`, an empty `{}`, an unclosed `{`, or a lone `}` is a template syntax error.

```yaml
command:
  - claude
  - --model
  - "{model}"      # replaced at runtime, or --model/{model} pair is stripped if no model
  - -p

model: sonnet      # default; steps without model: use this

steps:
  planning:
    model: opus    # overrides the default for this step only
    prompt: "Create a plan for: {input}"
```

### SDK Mode

Instead of spawning an external CLI via `command`, prompt steps can be driven through an SDK backend by setting the top-level `sdk` field. `command` and `sdk` are mutually exclusive; omitting both selects the default `jcode` backend. Two values are accepted: `jcode` and `claude`.

```yaml
sdk: jcode        # optional -- this is the default when neither `command` nor `sdk` is set

model: anthropic-api/claude-opus-4-6   # "provider/model[:effort]" for ordinary prompt steps
plan_model: openai-api/gpt-5.5:high    # model for the built-in plan step (falls back to `model`)
```

In both SDK backends, `model` / `plan_model` / per-step `model` are **model references** with the same override precedence as command mode (step `model` > top-level `model` / `plan_model`). The optional `:effort` suffix selects a reasoning-effort tier (`low` / `medium` / `high` / `xhigh` / `max`, plus the aliases `minimal` / `min` / `med` and the numeric spellings `1`..`4`); `off` / `none` / `0` / `5` are also consumed but leave the effort unset, and any other `:` suffix (an OpenRouter `:free` variant, say) stays part of the model id. An effort a provider or model does not support is ignored.

- `"provider/model[:effort]"` (e.g. `openai-api/gpt-5.5:xhigh`) -- under `sdk: jcode` this names the provider and the model separately; a `/` with an empty side (`"/model"`, `"provider/"`) fails the step with a clear error when the prompt runs, not at config-validation time. Under `sdk: claude` there is no provider part: everything except the `:effort` suffix goes to `claude --model` verbatim, so a `provider/model` value reaches the CLI as one model id and is rejected by it.
- `"model"` (no `/`) -- the provider is left to the backend's own resolution.
- Unset -- the backend's configured default provider/model is used.

#### `sdk: jcode` -- the jcode CLI (default)

`sdk: jcode` drives the [`jcode`](https://github.com/1jehuang/jcode) CLI (`jcode run`) as a subprocess. jcode v0.81.1 or newer is required; an older binary is rejected with a clear error. The provider part of a model reference is a jcode provider id -- the values `jcode login --help` lists (`jcode provider list` prints only a curated subset and omits API-key providers such as `anthropic-api`); `cruise login --status` shows the ones cruise can already authenticate as. Custom OpenAI-compatible endpoints are added as jcode's own `[providers.<name>]` profiles (`jcode provider add`) rather than anything cruise-specific.

Credentials, sessions, and configuration live in cruise's own jcode home (`$XDG_DATA_HOME/cruise/jcode-home`, default `~/.local/share/cruise/jcode-home`), kept completely separate from your own `~/.jcode` -- cruise never reads or writes it. Sign in with [`cruise login`](#cruise-login); running `sdk: jcode` with no authenticated provider fails with an error pointing at `cruise login`.

Because jcode cannot register custom tools in-process, cruise's tools (`ask_user`, `submit_plan`, ...) reach the model through a stdio MCP server and appear as `mcp__cruise__<tool>`. One caveat: jcode also merges MCP configuration from the run directory (`.jcode/mcp.json`, `.mcp.json`, `.claude/mcp.json`), which takes precedence over cruise's registration. A project-local MCP server named `cruise` is rejected with an error (it would shadow cruise's tools); other project-local servers are loaded but pointed out with a warning.

#### `sdk: claude` -- the claude CLI in-process

`sdk: claude` drives the `claude` CLI in-process through claude-agent-sdk, with cruise's tools exposed as `mcp__cruise__<tool>`. Model references are plain `claude --model` names with the optional `:effort` suffix, which is forwarded as `claude --effort` (a CLI too old for that flag fails the step -- see [Prerequisites](#prerequisites)); authentication is the claude CLI's own (its stored credentials or `ANTHROPIC_API_KEY`), unaffected by `cruise login`. The CLI runs with permissions bypassed -- cruise workflows are unattended, so there is no console to answer a permission prompt on.

#### Tool-less (non-interactive) planning

By default, SDK-mode planning drives the plan through custom tools (`submit_plan` / `update_plan` / `ask_user`); both SDK backends support them.

Set `interactive_planning: false` to turn that off. Planning then embeds the target plan-file path in the prompt and asks the agent to write `plan.md` directly — exactly like the `command` backend — and registers no custom tools. The resulting `plan.md` is read back afterward (falling back to the agent's captured output if the file was not written, same as `command` mode).

```yaml
interactive_planning: false   # tool-less, file-based planning
```

`--grill` requires the interactive tool-based flow and is rejected when `interactive_planning` is off. The field has no effect in `command` mode, which is always file-based.

### Prompt Languages

`languages.pr` controls the language used for the auto-generated PR title and body, and `languages.plan` controls the language used by cruise's built-in planning prompts. `CRUISE_LANGUAGE_PR` and `CRUISE_LANGUAGE_PLAN` override the corresponding YAML values; blank environment values are ignored. Otherwise, nested fields take precedence over deprecated `pr_language` / `plan_language`, then the first supported locale from `LC_ALL`, `LC_MESSAGES`, `LANG`, or `LANGUAGE`, then `English`. The deprecated fields remain supported as fallbacks.

```yaml
languages:
  pr: Japanese             # PR title/body will be generated in Japanese
  plan: Japanese           # generated/updated plans and plan answers will be in Japanese
```

For compatibility with older configs:

```yaml
pr_language: Japanese     # Deprecated; use languages.pr instead.
plan_language: Japanese   # Deprecated; use languages.plan instead.
```

The effective values are available to built-in templates as `{pr.language}` and `{plan.language}`.

### Session Title Generation

After plan approval, cruise generates a concise session title (up to 80 characters) shown in `cruise list` and the GUI sidebar instead of the raw task input. The behavior depends on the backend:

- **SDK mode (`sdk:` set, or neither `sdk:` nor `command:` set -- the default `jcode` backend)** -- cruise invokes the agent with the `generate_title` SDK tool, using the same model resolution as the plan step (`plan_model` -> `model`, then the backend's own default). If the call fails, cruise falls back to extracting the title from `plan.md`.
- **Command mode (`command:` set)** -- no LLM is called for title generation. The title is derived automatically from the first heading or first non-empty line in the generated `plan.md`.

No additional configuration is required.

### Environment Variables

Environment variables can be set at two levels. Step-level values override top-level values for that step only. Values support template variable substitution.

The CLI and desktop GUI also apply these process-level workflow overrides when loading a session config: `CRUISE_MODEL`, `CRUISE_PLAN_MODEL`, `CRUISE_SDK`, `CRUISE_LANGUAGE_PR`, `CRUISE_LANGUAGE_PLAN`, `CRUISE_CLEANUP_AFTER_PR`, `CRUISE_INTERACTIVE_PLANNING`, and `CRUISE_FORCE_EXEC`. String values are trimmed and blank values are ignored; boolean values accept `true`, `false`, `1`, or `0`. Language settings fall back to locale inference from `LC_ALL`, `LC_MESSAGES`, `LANG`, then `LANGUAGE` when no explicit language is configured.

```yaml
env:                        # top-level: applied to all steps
  ANTHROPIC_API_KEY: sk-...
  TARGET_ENV: production

steps:
  deploy:
    command: ./deploy.sh
    env:                    # step-level: merged over top-level env
      TARGET_ENV: staging   # overrides top-level value for this step only
      LOG_LEVEL: debug
```

### Step Types

#### Prompt Step (LLM call)

```yaml
steps:
  planning:
    model: claude-opus-4-5        # model to use (optional; overrides top-level model)
    instruction: |                # system prompt (optional)
      You are a senior engineer.
    prompt: |                     # prompt body (use either prompt or prompt_file)
      Create an implementation plan for:
      {input}
    timeout: 10m                  # per-step timeout (optional; see Step Timeout)
    env:                          # environment variables for this step (optional)
      ANTHROPIC_MODEL: claude-opus-4-5
```

For longer prompts, load the prompt body from a file with `prompt_file`:

```yaml
steps:
  implement:
    prompt_file: prompts/implement.md
```

`prompt_file` may be an absolute path, a `~/` path, a path relative to the
configuration file, or a GitHub blob/raw URL. A bare file name is resolved next
to the configuration file. GitHub URLs are fetched via `gh api` at config-load
time. Files referenced inside a `workflow_call` are
resolved relative to the called workflow's directory. File contents are kept
verbatim and use the same variable expansion as `prompt`; `prompt` and
`prompt_file` are mutually exclusive. For repo-backed sessions whose config lives in the temporary clone, the session
snapshot stores the resolved prompt contents, so they remain usable after the
clone is removed.
Resolution follows these rules:

| `prompt_file` value | Resolution |
| --- | --- |
| Absolute path | Used as-is |
| `~/...`, `~`, or `~user/...` | Expanded from the current user's or named user's home directory |
| `./...`, `../...`, or a bare file name | Relative to the directory containing the config file |
| GitHub blob/raw URL | Fetched from GitHub |
| Relative path inside a called workflow | Relative to the called workflow's directory (or remote directory) |

Absolute and `~` paths refer to the local filesystem. In a GitHub-hosted
workflow, relative non-URL values are resolved as paths in the remote directory; local
`~` paths are rejected. A direct GitHub blob/raw URL can be used explicitly.

#### Command Step (shell execution)

```yaml
steps:
  run_tests:
    command: cargo test           # single command (required)
    timeout: 5m                   # per-step timeout (optional; see Step Timeout)
    env:                          # environment variables for this step (optional)
      RUST_LOG: debug

  lint_and_test:
    command:                      # list of commands: run sequentially, stop on first failure
      - cargo fmt --all
      - cargo clippy -- -D warnings
      - cargo test
```

#### Step Timeout

Any step may set `timeout:` to abort the step if it runs too long. Accepted formats:

| Suffix | Meaning | Example |
|--------|---------|---------|
| (none) | Seconds | `timeout: "30"` |
| `m` | Minutes | `timeout: 5m` |
| `h` | Hours | `timeout: 1h` |

When a timeout fires:

- **Command steps**: the child process is killed and the step is treated as a failure (non-zero exit). `{prev.success}` is `false` and the workflow follows the normal failure path (see `if.fail` below).
- **Prompt steps**: the LLM call is aborted and the step is treated as a failure.

Invalid timeout strings are rejected at config validation time. Timeouts are also honoured for steps defined inside groups and `after-pr`.

#### Option Step (interactive selection)

Each item in `option` is either a `selector` (menu choice) or a `text-input` (free-text prompt). The optional `plan` field resolves to a file path whose contents are displayed in a bordered panel before the menu is shown:

```yaml
steps:
  review_plan:
    plan: "{plan}"               # optional: display contents of this file before the menu
    option:
      - selector: Approve and continue   # shown in selection menu
        next: implement
      - selector: Revise the plan
        next: planning
      - text-input: Other (free text)    # shows a text prompt when selected;
        next: planning                   # entered text is available as {prev.input}
      - selector: Cancel
        next: ~                          # null next = end of workflow
```

### Post-PR Automation (`after-pr`)

Use `after-pr` for steps that should run automatically after `cruise run` successfully creates a pull request. `after-pr` uses the same step format as `steps`, so you can define inline or file-backed prompt steps (`prompt` / `prompt_file`), command steps, and grouped steps there as well.

```yaml
steps:
  implement:
    prompt: "{input}"

  test:
    command: cargo test

after-pr:
  notify:
    command: "echo 'PR #{pr.number} created: {pr.url}'"

  label:
    command: "gh pr edit {pr.number} --add-label enhancement"
```

`after-pr` steps run only after PR creation succeeds. They can use all normal template variables plus the PR-specific variables listed below.

### Flow Control

#### Explicit next step

```yaml
steps:
  step_a:
    command: echo "hello"
    next: step_c                  # jump over step_b
  step_b:
    command: echo "skipped"
  step_c:
    command: echo "world"
```

#### Skipping a step

```yaml
steps:
  optional_step:
    command: cargo fmt
    skip: true                    # always skip

  fix_errors:
    command: cargo fix
    skip: prev.success            # skip if the variable "prev.success" resolves to "true"
```

The `skip` field accepts a static boolean (`true`/`false`) or a variable reference string. When a variable reference is given, the step is skipped if that variable's current value is `"true"`.

#### Conditional execution by file existence (`when.exists`)

`when.exists` is a pre-execution condition that **skips the step unless at least one file matches the given glob**. Use it to run a step only when relevant files are present -- for example, a Rust-specific review step that should be a no-op in a repo with no `.rs` files.

```yaml
steps:
  rust-review:
    when:
      exists: "**/*.rs"       # run only if a matching file exists; otherwise skip the step
    prompt: "Review the Rust code and fix any issues."
```

- The glob is evaluated relative to the workflow's working directory. Absolute patterns are used as-is.
- Template variables in the pattern are resolved before globbing, so `exists: "{input}/**/*.rs"` works.
- **No match -> the step is skipped** (shown as `skipping: <step> (no files match when.exists)`). One or more matches -> the step runs normally.
- An empty or syntactically invalid glob is rejected at config validation time.
- If some entries cannot be read while scanning (e.g. permission errors), cruise errs on the side of running the step rather than silently skipping it.
- `when.exists` is independent of `skip`: if `skip` already skips the step, the glob is not evaluated at all.

#### Conditional execution (file-changed detection)

When a step has `if: file-changed: <target>`, a snapshot of the working directory is taken **before** the step runs. After the step executes, if any files changed during its execution, the workflow jumps to `<target>`. If no files changed, the workflow continues to the next step normally.

This is designed for loop-back patterns -- for example, re-running tests whenever a review step modifies code:

```yaml
steps:
  test:
    command: cargo test

  review:
    prompt: "Review the code and fix any issues."
    if:
      file-changed: test    # after review, if it modified files, jump back to test
```

> **Note:** The snapshot is taken **before** the step with the `if:` condition runs. If no files change during the step's execution, the workflow proceeds to the next step (or follows the `next:` field if set).

> **Warning:** A top-level cycle that mixes an `if.file-changed` jump back with unconditional sequential edges -- exactly the `test` → `review` → `test` shape above -- is rejected at startup, since it always exceeds the loop-protection ceiling once the conditional edge has exhausted its retries. Confine such retry loops inside a [step group](#step-groups) with `max_retries`.

#### No file changes detection (`if.no-file-changes`)

When a step has `if.no-file-changes` set to `retry` or `failed`, a snapshot of the working directory is taken **before** the step runs. If the step completes without modifying any workspace files, the configured action is taken. Two modes are available:

- **`failed`** -- Abort the workflow with an error and transition the session to the `Failed` state. This is useful for detecting cases where an LLM claims to have implemented something but did not actually modify any files.
- **`retry`** -- Re-execute the current step. This is useful for retrying a step until it produces meaningful file changes.

```yaml
steps:
  implement:
    prompt: "Implement the feature described in {plan}"
    if:
      no-file-changes: failed

  fix:
    prompt: "Fix the issue"
    if:
      no-file-changes: retry
```

**Constraints:**
- The value must be either `retry` or `failed`; any other value (including the removed object form `{ fail: true }` / `{ retry: true }`) is a parse error.
- Cannot be used in `after-pr` steps (rejected at validation time).
- Cannot be used at the group level (`if` in group definitions).
- Can be combined with `if: file-changed` on the same step, but when both are present, `no-file-changes` takes priority for change detection.
- The legacy `fail-if-no-file-changes: true` field is rejected as an unknown field; migrate to `if: { no-file-changes: failed }`.

##### Declaring intentional no-changes

Not every no-change is a failure to route around -- sometimes the plan explicitly says a step should make no changes (e.g. "don't add tests here"), and an agent that reaches that conclusion again on every retry is giving the correct answer, not stalling. Two ways to tell cruise the no-change is deliberate; either one disables **both** actions (`failed` and `retry`) for that attempt:

- **Output marker** -- a line in the step's raw output starting with `NO_CHANGES_INTENTIONAL: <reason>` (leading whitespace on the line is ignored). Works with every backend (`command:` and both `sdk:` modes) since it's plain text matching, no tool support required. The marker must anchor the start of a line -- a mid-line mention (quoted in passing, inside a code block, etc.) does not count.
- **`skip_step` tool** (SDK mode only) -- the agent calls `skip_step(reason)` instead. Schema-validated rather than text-matched. Registered only on prompt steps with an `if.no-file-changes` condition, to keep the exposed tool set minimal on steps that can never call it. Not available in classic `command:` mode -- use the output marker there.

Either path logs the declared reason so the decision stays visible in the run output.

#### Failure handling (`if.fail`)

`if.fail` decides what happens when a step fails. A failure means any of: a non-zero exit code from a command step, a prompt step error (including LLM transport errors), a `timeout`, or a `no-file-changes: failed` trigger.

Two forms are accepted:

- **`fail: <step-name>`** -- Jump to the named step.
- **`fail: { retry: true }`** -- Re-execute the current step.

```yaml
steps:
  flaky_test:
    command: cargo test --flaky
    timeout: 2m
    if:
      fail:
        retry: true        # retry on non-zero exit, timeout, or other failure

  deploy:
    command: ./deploy.sh
    if:
      fail: rollback       # jump to the `rollback` step on failure

  rollback:
    command: ./rollback.sh
```

`if.fail` is subject to the same loop-protection budget as other flow-control jumps (`--max-retries`), so a misconfigured retry loop will not run forever.

A step cycle that mixes a conditional jump (`if.file-changed` / `if.fail` goto) with unconditional sequential edges is rejected at startup: once the conditional edge exhausts its retries, the unconditional edges would always exceed the loop-protection ceiling. Confine such loops inside a group under `groups:` with `max_retries`, as in the example below, so exhausted retries degrade into a graceful skip. A group retry loop without `max_retries` has no such graceful skip and is treated as an unsafe conditional edge.

**Constraints:**
- `if.fail` is rejected at the group level and in `after-pr` steps.
- Can be combined with other `if:` keys (`file-changed`, `no-file-changes`) on the same step.

### Step Groups

Steps can be grouped to coordinate retry loops across multiple steps. A group retries all its member steps together when the `if: file-changed` condition triggers.

Groups can define their steps inline and are invoked from the main `steps` section with `group: <name>`:

```yaml
max_retries: 4

groups:
  review:
    if:
      file-changed: test    # if any step in the group changes files, retry from the group start
    max_retries: 3          # maximum number of group-level retry loops (optional)
    steps:                  # steps defined inside the group
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  test:
    command: cargo test

  review-pass:
    group: review           # invokes the "review" group's steps at this point
```

The same group can be invoked from multiple places in the workflow:

```yaml
steps:
  test-lib:
    command: cargo test --lib
  review-lib:
    group: review

  test-doc:
    command: cargo test --doc
  review-doc:
    group: review           # same group, different call site
```

**Constraints:**
- Steps inside a group definition cannot have nested `group:` references or individual `if:` conditions -- the group-level `if:` applies to the entire group.
- When the group's `if: file-changed` condition triggers, execution jumps back to the **first step of the group** and all group steps re-run.
- A call-site step (e.g. `review-pass: group: review`) cannot have its own `if:` condition.

### Workflow Composition (`workflow_call`)

A step can delegate to another workflow config file by setting `workflow_call` instead of `prompt`, `prompt_file`, `command`, or `option`. The called workflow's steps are inlined into the parent at the call site, with each step ID prefixed by the call-site name (e.g. `shared-review/simplify`).

```yaml
steps:
  build:
    command: cargo build

  shared-review:
    workflow_call: ./workflows/review.yaml    # local relative path

  deploy:
    command: cargo publish
```

The referenced file is a regular cruise config. Its top-level execution settings (`command`, `sdk`, `model`, `env`, etc.) are ignored -- only its `steps` are imported. The parent's settings apply to the expanded steps.

#### Supported sources

| Source | Example |
|--------|---------|
| Local relative path | `workflow_call: ./workflows/review.yaml` |
| GitHub blob URL | `workflow_call: https://github.com/org/repo/blob/main/workflows/review.yaml` |
| GitHub raw URL | `workflow_call: https://raw.githubusercontent.com/org/repo/main/workflows/review.yaml` |

GitHub workflows are fetched via `gh api` at config-load time. Relative paths inside a GitHub-hosted workflow resolve from the remote directory, so nested references work across repositories.

#### Call-site fields

A `workflow_call` step is a pure delegation point. Only `skip`, `when`, and `next` may be set alongside `workflow_call`:

- `skip` and `when` are applied to the **first** expanded step.
- `next` is applied to the **last** expanded step (when it has no explicit `next` of its own).

All other step fields (`prompt`, `prompt_file`, `command`, `model`, `if`, `timeout`, `env`, etc.) are rejected at validation time.

#### Nesting and cycle detection

Workflow calls can be nested: a called workflow may itself contain `workflow_call` steps. Step IDs accumulate prefixes (`outer/inner/step`). Circular references (A calls B, B calls A) are detected and rejected. Groups inside called workflows are not supported.

```yaml
# parent.yaml -> nested/outer.yaml -> inner/leaf.yaml
# Results in step IDs: outer-call/leaf-call/leaf
steps:
  outer-call:
    workflow_call: ./nested/outer.yaml
```

### Variable Reference

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from CLI argument or stdin |
| `{prev.output}` | LLM output from the previous step |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Stderr captured from the previous command step |
| `{prev.success}` | Exit status of the previous command step (`true`/`false`) |
| `{plan}` | Session plan file path (set automatically by `cruise run`) |
| `{plan.language}` | Effective language used for built-in planning prompts (from `CRUISE_LANGUAGE_PLAN`, `languages.plan`, the legacy field, locale inference, or the default) |
| `{pr.number}` | Pull request number, available after a PR has been created |
| `{pr.url}` | Pull request URL, available after a PR has been created |
| `{pr.language}` | Effective language used for PR title/body generation (from `CRUISE_LANGUAGE_PR`, `languages.pr`, the legacy field, locale inference, or the default) |

> **Note:** `{model}` is **not** a template variable -- it is a special placeholder resolved only within the top-level `command` array. It is not available inside `prompt`, `prompt_file`, `instruction`, or `command` step fields.

Literal braces are escaped Rust-`format!`-style: `{{` -> `{` and `}}` -> `}` (e.g. `"{{input}}"` resolves to the literal string `"{input}"`, not a lookup of `input`). An unclosed `{`, a lone `}`, or an empty `{}` is a template syntax error, as is referencing an undefined variable.

## Workspace Mode

When `cruise run` starts a new session, it prompts you to choose a workspace mode:

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| Mode | Description |
|------|-------------|
| **Worktree** (default) | Creates an isolated git worktree at `$XDG_DATA_HOME/cruise/worktrees/<session-id>/` (default: `~/.local/share/cruise/worktrees/<session-id>/`). A new branch `cruise/<session-id>-<sanitized-input>` is checked out. Requires `gh` CLI for PR creation. |
| **Current branch** | Executes directly in the current repository on the active branch. No worktree is created, and no PR is created automatically. |

In non-interactive environments (piped stdin) and with `--all`, worktree mode is used automatically. Sessions created with `--repo` (or the GUI repository picker) are always pinned to Worktree mode — the prompt is skipped and current-branch mode is not available, since a PR is the only way the work leaves the temporary clone.

### Current-branch mode constraints

- For a fresh `cruise run` current-branch session, requires a clean working tree (no uncommitted changes). `cruise exec` and `force_exec` sessions are allowed to start dirty, with a warning; cruise does not stash, commit, or reset existing changes.
- Requires an attached branch (not detached HEAD).
- On resume, the active branch must match the branch recorded at the start of the session.

### Worktree isolation

- The worktree is retained until the PR is closed or merged; run `cruise clean` to delete it.
- Set `cleanup_after_pr: true` in the config (or pass `--cleanup-after-pr` at runtime) to automatically delete the local worktree and branch immediately after the PR is created. Use `--no-cleanup-after-pr` to override the config setting and keep them.

### Copying files into the worktree

Create a `.worktreeinclude` file in the repo root to copy files or directories into the new worktree before the workflow starts:

```
# .worktreeinclude
.env
.cruise/
secrets/config.yaml
```

Each line is a relative path (files or directories). Absolute paths and `..` traversal are ignored for safety.

## Example Config

### Full Development Flow

```yaml
command:
  - claude
  - --model
  - "{model}"
  - -p

model: sonnet
plan_model: opus
max_retries: 4

groups:
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  plan:
    model: opus
    instruction: "What will you do?"
    prompt: |
      I am trying to implement the following features. Create an implementation plan and write it to {plan}.
      ---
      {input}

  approve-plan:
    plan: "{plan}"
    option:
      - selector: Approve
        next: write-tests
      - text-input: Fix
        next: fix-plan
      - text-input: Ask
        next: ask-plan

  fix-plan:
    model: opus
    prompt: |
      The user has requested the following changes to the {plan} implementation plan. Make the modifications:
      {prev.input}
    next: approve-plan

  ask-plan:
    prompt: |
      The user has the following questions about the implementation plan for {plan}. Provide answers:
      {prev.input}
    next: approve-plan

  write-tests:
    prompt: |
      Based on the {plan} implementation schedule, please first create the test code,
      then update the {plan} if necessary.

  implement:
    prompt: |
      Tests have been created according to {plan}. Please implement them to pass.
      If necessary, update {plan}.

  test:
    command:
      - cargo fmt --all
      - cargo clippy --fix --allow-dirty --all-targets --all-features -- -D warnings
      - cargo test

  fix-test-error:
    skip: prev.success            # skip if tests passed
    prompt: |
      The following error occurred. Please correct it:
      ---
      {prev.stderr}
    next: test

  review-pass:
    group: review

cleanup_after_pr: true    # delete local worktree and branch after PR is created

after-pr:
  label:
    command: gh pr edit {pr.number} --add-label automated

  announce:
    command: "echo 'Created PR: {pr.url}'"
```

### Simple Auto-Commit Flow

```yaml
command:
  - claude
  - -p

groups:
  fix-loop:
    if:
      file-changed: test    # if the fix modified files, rerun the tests
    max_retries: 2          # retries exhausted -> continue to commit
    steps:
      apply-fix:
        prompt: |
          The following test errors occurred. Please fix them:
          ---
          {prev.stderr}

steps:
  implement:
    prompt: "{input}"

  test:
    command: cargo test

  fix:
    group: fix-loop

  commit:
    command: "git add -A && git commit -m 'feat: {input}'"
```

The retry loop is confined inside a group so that exhausted retries degrade into a graceful skip instead of a flat step cycle, which is rejected at startup.

## Config Hot-Reload

During `cruise run`, the config file is checked for changes between each step. If the file has been modified (detected via mtime), the updated config is reloaded automatically -- no restart required. This allows you to adjust prompts, add steps, or tweak settings while a session is running.

> **Note:** Hot-reload only applies when the session was started from an external config file (not the built-in default). The current step must still exist in the new config for the reload to take effect.

## Rate Limit Retry

When a rate-limit error (HTTP 429) is detected in a prompt or command step, cruise retries the same model with exponential backoff:

- Initial delay: 2 seconds
- Maximum delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)

The SDK backends additionally accept an optional `retry:` block that widens this into a fallback policy:

```yaml
retry:
  base_delay_ms: 500        # backoff base (default 500); delay is min(base * 2^(attempt-1), 8s) with jitter
  max_delay_ms: 300000      # waiting cap (default 300000). The computed backoff is already capped at 8s,
                            # so this only binds a server `Retry-After` hint (itself clamped to 60s): a
                            # hinted delay above it moves to the next fallback model, or fails the step
                            # when there is none
  model_fallback: true      # allow switching to a fallback model (default true)
  fallback_chains:          # tried in order: exact "provider/model" (or bare "model") key,
                            # then "provider/*", then "default"
    default:
      - anthropic-api/claude-opus-4-6
      - openai-api/gpt-5.5
    "anthropic-api/*":
      - openrouter/*
```

With `retry:` set, HTTP 5xx and network failures become retryable too, and a model that has spent its retry budget (`--rate-limit-retries`) is swapped for the next entry of its chain -- immediately, with a fresh budget, on a fresh session. A `provider/*` chain entry keeps the failing model id and swaps only the provider. `--rate-limit-retries 0` disables retrying, so a rate limit or a 5xx fails the step with no model switch; only a model reference the backend refuses outright still moves to the next chain entry, since nothing was sent and there is nothing to replay. A failed model is skipped for the next 5 minutes of the run (in-memory state owned by the active policy, so a config [hot-reload](#config-hot-reload) clears it). A turn that already streamed visible text is never retried on another model.

Declaring `retry:` at all changes the no-`retry:` behavior, `model_fallback: false` and empty chains included: those only switch model switching off, while 5xx/network classification and the `base_delay_ms`/8s-ceiling backoff schedule stay in force. Omit the block entirely to keep the historical behavior -- rate limits only, same model, 2s doubling to a 60s cap. The `command:` backend always uses the historical behavior and ignores `retry:`.

## Stale Session Detection

When `cruise list`, `cruise tui`, or the desktop GUI loads sessions, any session in the `Running` phase is checked for liveness. If the runner process (identified by PID and start time) is no longer alive, the session is automatically transitioned to the `Suspended` phase. This prevents sessions from being stuck in `Running` indefinitely after a crash or forced termination.

Suspended sessions can be resumed from `cruise list`, `cruise tui`, or reset to Planned. The `run --all` command also picks up Suspended sessions alongside Planned ones.

On resume, cruise restores more than just the current step: while running, each step's pre-execution runtime context -- the `{prev.*}` variables and file-change-tracking snapshots -- is best-effort persisted to `dag.json` in the session directory. `cruise run` loads this file when resuming an interrupted session and restores that context, so `{prev.*}` references and file-change detection behave exactly as they would have without the interruption. A save failure, or a missing/corrupt `dag.json`, falls back to the previous resume behavior (no restored context) with a warning; sessions created before this existed are unaffected.

## Parallel Session Execution

The desktop GUI, TUI, and CLI support running multiple sessions concurrently during `run --all`.

- **GUI/TUI**: the parallelism level is controlled by `run_all_parallelism` in `$XDG_CONFIG_HOME/cruise/config.json` (configurable via `cruise config --set-parallelism <N>`, default: `1`).
- **CLI**: pass `cruise run --all --parallelism <N>` for a one-run override (default: `1`, i.e. sequential). The persisted GUI/TUI setting is neither read nor modified. In an interactive terminal, when `--dry-run` is not set, the CLI also shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output is retained in `sessions/{id}/run.log`. Non-TTY and dry-run invocations keep the normal log output and final summary.

The batch scheduler:
- Seeds from Planned and Suspended sessions.
- Launches up to `N` sessions concurrently.
- Re-scans for newly added eligible Planned or Suspended sessions every 200ms while worker slots are available, so sessions created while a batch is running are picked up automatically.
- Results are returned in scheduling order regardless of completion order.

## New Session Form Persistence

The desktop GUI and TUI persist two pieces of state across sessions:

- **Draft** (`$XDG_STATE_HOME/cruise/new_session_draft.json`): The current contents of the New Session form (task description, config path—including the `__builtin__` sentinel for **Built-in default**—, working directory, repository, skipped steps). Automatically saved on changes and restored when the form is reopened, so unsent input is not lost.
- **History** (`$XDG_STATE_HOME/cruise/history.json`): A log of past New Session selections. Used to pre-populate the step skip selector with the most recent choices for each config file and to recall previous working directory / config combinations.

## GitHub Actions

Mention `@cruise` on a GitHub Issue to drive cruise inside GitHub Actions. The action installs the `jcode` CLI and provisions its credentials, so cruise's default `sdk: jcode` backend works with no configuration; it no longer forces a backend, so a repository config that sets `command:` or `sdk: claude` still wins and has to bring its own CLI. There is no PR mode -- comments on pull requests are ignored.

**Quickstart:**

1. Install the [`cruise-agent` GitHub App](https://github.com/apps/cruise-agent/installations/new) on your repository (optional; falls back to `GITHUB_TOKEN` otherwise).
2. Add an `ANTHROPIC_API_KEY` and/or `OPENAI_API_KEY` repository secret.
3. Copy [`examples/cruise.yml`](examples/cruise.yml) to `.github/workflows/cruise.yml`.

Then open an issue, or comment on one, starting with `@cruise`:

| Mention | Command | What happens |
|---|---|---|
| `@cruise`, `@cruise run <request>` | **run** | Plan (or resolve the existing plan comment), implement in a worktree, and open a **draft** pull request. |
| `@cruise exec <request>` | **exec** | Same planning, but pushes straight to the default branch -- no PR. Advanced/opt-in. |
| `@cruise plan <request>` | **plan** | Post an LLM-generated plan as a tracking comment. Nothing is executed. |
| `@cruise fix <feedback>` | **fix** | Revise the existing plan-tracking comment in place. |

See [`docs/github-actions.md`](docs/github-actions.md) for the full command grammar, a typical plan -> fix -> run -> review walkthrough, the provider table (Anthropic/OpenAI/Kimi/Google/Groq/DeepSeek/xAI/...), inputs/outputs, security notes, and how to point it at your own workflow config or a self-hosted endpoint. See [`examples/`](examples/) for drop-in workflow files and a sample repo config.

## License

MIT
