# Using Cruise

**Languages:** [English](../en/usage.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/usage.md) · [简体中文](../zh/usage.md) · [Index](README.md)

## WebUI

Start the server-rendered browser UI with:

```sh
cruise webui
```

This runs in the foreground. To manage a background instance instead:

```sh
cruise webui start --port 8484
cruise webui status
cruise webui status --json
cruise webui restart --port 8484
cruise webui stop
```

`start` and `restart` accept the same server options shown below; put them after the subcommand. `stop --force` permits forced termination if graceful shutdown cannot complete. Status reports a stopped instance with a nonzero exit status. Stopping the WebUI is not a request to delete sessions. The lifecycle commands manage the local instance, not a system service or an automatic startup watcher.

Foreground and background servers share one registry per state directory, even if `XDG_DATA_HOME` differs. Use the same `XDG_STATE_HOME` to manage the instance you started. A repeated `start` reports the running instance; specifying a different host or port requires `restart`. `status --json` reports fields including `running`, `stale`, `mode`, `origin`, `pid`, `version`, `started_at`, `log_path`, and `outdated`. Status does not remove a stale record; `stop` removes a stale record without signalling a reused PID.

`restart` reuses the previous host and port unless overridden. It starts in the invoking command's current working directory, not necessarily the original server's directory. Pass `--webui-dir` again if using development assets, and run from the intended directory for relative paths. Stop and restart request cancellation of active WebUI operations; running sessions are suspended so they can be resumed, rather than deleted. Forced termination may prevent graceful cleanup.

The registry, lock and background log live under `$XDG_STATE_HOME/cruise/webui/` (normally `~/.local/state/cruise/webui/`). The instance record contains a control credential and should be kept private. The background log is `webui.log`, rotated to `webui.log.1` on a later start when larger than 5 MiB. On Windows the default state directory is `%LOCALAPPDATA%\cruise\state` when `XDG_STATE_HOME` is unset. Detached start, status, stop and restart are available on Windows as well as Unix; on Windows a failed graceful control request may require `stop --force`.

The default URL is `http://127.0.0.1:8484/`. Use these flags to customize it:

- `--host <ADDR>` -- interface to bind (default `127.0.0.1`; the WebUI has no authentication, so do not bind it to an untrusted network). The Merge PR route is served by the same unauthenticated server, and its on-screen confirmation is not access control: never expose the WebUI to a network you do not trust.
- `--port <PORT>` -- TCP port to listen on (default `8484`).
- `-o`, `--open` -- open the WebUI in the default browser after the server starts (by default the browser is not opened; visit the printed URL yourself).
- `--webui-dir <DIR>` -- serve `templates/` and `static/` from disk for development. For example, `cruise webui --webui-dir webui`; templates are re-read on every request, so edit and reload without restarting.
- `Ctrl-C` -- stops the server. Active operations are cancelled (sessions become Suspended), the server exits within about 10 seconds, and a second `Ctrl-C` quits immediately.

The Graph tab loads Mermaid from jsDelivr and therefore needs network access; all other WebUI tabs work offline.

Choose the display language in the sidebar footer on desktop or the header on mobile. The selection reloads the page and is saved in the browser's `cruise_ui_language` cookie for approximately one year, without changing application configuration. A valid cookie takes precedence over the server's UI language; without one, the server's process default applies.

TSX component imports are relative to the file containing the import. For example, `templates/components/session-row.tsx` imports its sibling badge as `./phase-badge`, not `./components/phase-badge`.

## Usage

```sh
# Create a session (plan -> approve)
cruise plan "implement the feature"

# Create a session and generate the plan in the background
cruise --plan "implement the feature"

# Interview-style planning: answer one question at a time, then the plan is written (SDK backend + TTY + `interactive_planning: true`)
cruise plan --grill "implement the feature"

# Enable only browser and app E2E for the built-in test-first workflow
cruise plan -c builtin:tdd -f browser_e2e,app_e2e "task"

# Plan against a GitHub repository instead of a local directory (temporary clone)
cruise plan --repo owner/repository "implement the feature"

# Background planning from stdin
echo "implement the feature" | cruise --plan stdin

# Save the task as a draft (no plan yet); generate the plan later from `cruise list`
cruise draft "implement the feature"

# Execute (or resume) a pending session -- Planned, Running, Failed, or Suspended
cruise run

# Execute a config directly in the current directory (no plan, no worktree, no PR)
cruise exec "do this"

# Run one agent without writing a workflow YAML
cruise exec --instant "do this"

# Open the full interactive keyboard client
cruise

# List and manage sessions with the CLI selector
cruise list

# Review a session's changes, or just their summary
cruise diff <SESSION_ID>
cruise diff <SESSION_ID> --stat

# Remove closed/merged PR sessions and terminal no-PR sessions
cruise clean

# Open the TUI in a remote project
cruise ssh devbox --cwd /srv/project

# Run remote commands and forward stdin or root options unchanged
cruise ssh devbox --cwd /srv/project -- plan "implement the feature"
echo "implement the feature" | cruise ssh devbox --cwd /srv/project -- plan
cruise ssh devbox -- list --json | jq .
cruise ssh devbox -- run
cruise ssh devbox -- --plan "implement the feature"
cruise ssh devbox -- --plan "implement the feature" --repo owner/repository

# Legacy: positional input without a subcommand is treated as `cruise plan`
cruise "implement the feature"
```

The `ssh` subcommand reserves the exact legacy input `ssh`; to plan a task whose
entire text is `ssh`, use `cruise plan "ssh"`.
The hidden `mcp-bridge` entry point likewise reserves the exact input
`mcp-bridge`; use `cruise plan "mcp-bridge"` to plan that task.
The `tell` subcommand is also reserved; use `cruise plan "tell"` to plan that text.

The CLI and TUI support best-effort desktop, sound, and Slack webhook notifications for
**Action required**, **Plan ready**, **Completed**, and **Failed** workflow events.
Desktop notifications require a macOS or Linux desktop session; sound is off by default.
Delivery failures never change workflow results or exit status. Set
`CRUISE_DISABLE_NOTIFICATIONS=1` to disable all three channels. See [Notifications](cli-reference.md#notifications) for settings and WebUI behavior.

### herdr integration

Inside a [herdr](https://herdr.dev) pane (`HERDR_ENV=1` with `HERDR_PANE_ID` and
`HERDR_BIN_PATH` set), the TUI, `cruise plan`, `cruise run`, and `cruise exec`
report their lifecycle state through `"$HERDR_BIN_PATH" pane report-agent` as
source `custom:cruise` / agent `cruise`: `working` while planning or executing
steps, `blocked` while waiting for an answer, an option selection, or plan
review, and `idle` otherwise. Authority is released on exit. Reporting is
best-effort and never changes workflow success, failure, or exit status. Set
`CRUISE_DISABLE_HERDR=1` to disable it.
From a herdr pane, use `cruise tell <session-id> "instruction"` to instruct a running session.

### TUI (Interactive Keyboard Client)

Running `cruise` with no arguments opens cruise's official interactive keyboard client: the third client beside the CLI and WebUI. It preserves the existing CLI behavior while bringing the WebUI's session-management workflows to a terminal.

```sh
# Open the interactive client
cruise
```

Typical flow: run `cruise`, press `n`, choose the Source, working directory or repository, and workflow config, then type the task. Press `Ctrl-P` for normal planning, `Ctrl-G` for grill planning, or `Ctrl-U` to use the input directly as the plan, or press `Tab` to answer the remaining questions one at a time and pick the launch mode at the end. After Task, the remaining questions are Images, declared workflow flags (if any), Skipped steps (flag-disabled steps are hidden), Workspace (Directory sessions only), Dirty tree when applicable, and Launch. Press `1` to inspect or act on sessions, then `3` to run the planned queue in **Run All**. On Run All, press `p` to edit the shared parallelism setting, type a positive integer, and press `Enter` to save or `Esc` to cancel.

The TUI requires an interactive TTY on macOS, Linux, or Windows. It is an interactive client, not an automation interface: use the CLI for automation, JSON output, and CI. GitHub-backed workflows use the external [`gh` CLI](https://cli.github.com/); the TUI does not bundle or replace it. Current-branch-only work does not require `gh` (`glab` for GitLab repositories).

#### Screens and workflows

The TUI has three views:

- **Sessions** -- Browse the global session list. Select a session to view its **Info**, **Graph**, **Plan**, **Log**, or **Diff** detail tab. The [`cruise list` Actions](sessions.md#cruise-list-actions) matrix describes the list menu, not the full TUI action set. The Info tab shows declared workflow flags and their on/off values. The Graph tab shows its node list plus the selected node's dependency and edge details, hiding flag-disabled steps except in legacy unversioned graph checkpoints, which are displayed without filtering; Markdown is parsed and styled. This includes Ask and Option prompts, Clean, worktree/current-branch selection, Publish as Issue, and PR links.
- **New Session** -- Create a session or draft through a step-by-step dialogue: one question is shown at a time with the answers so far listed above and the remaining questions below. The order is Source (local Directory or GitHub repository), working directory or repository, workflow config, task, images, workflow flags (shown only when the config declares flags), skipped steps (flag-disabled steps are hidden), workspace mode, dirty-tree allowance (current-branch runs only), and finally the launch mode (normal planning, grill planning, input-as-plan, or save as draft). Questions that earlier answers make moot are skipped. For local Directory sessions, the Workflow config question includes Auto-detect and organizes the same prioritized file candidates and built-in workflows as the CLI according to configured categories and Others visibility. GitHub sessions resolve Auto-detect in the cloned repository, so caller-local candidates are omitted, while an arbitrary path is still accepted. `Ctrl-P`, `Ctrl-G`, `Ctrl-U`, and `Ctrl-S` start or draft the session from any question with the current answers. Directory and path answers offer completion, and history is recalled with the arrow keys; draft and selection history are retained as described in [New Session Form Persistence](sessions.md#new-session-form-persistence).
- **Run All** -- Run Planned or Suspended sessions with live parallelism, in-app status, and bell feedback. Press `p` to edit the shared WebUI/TUI parallelism setting. Changes are saved to the app configuration and can be made while a batch is active; a new limit applies at the next scheduling point, without cancelling workers that are already running. Distinct sessions may run concurrently in one TUI process; duplicate work for one session is rejected.

PR and Issue URLs are shown as text; a successful Publish as Issue also opens the new issue URL automatically. The dedicated PR/Issue URL action opens them with `open` on macOS, `explorer.exe` on Windows, or `xdg-open` on Linux; other Markdown links remain textual. CLI-only `config` and `exec` operations remain available through their CLI commands rather than TUI screens.

The New Session dialogue autosaves its answers 500 ms after a change. A selected session opens on its **Plan** tab while planning is active or its phase is **Awaiting Input**, **Awaiting Approval**, or **Planned**, and on **Info** otherwise. Other screen state is ephemeral, except the shared Run All parallelism setting, which is persisted in the app configuration, and manually selected detail tabs, which are retained per session for the duration of the TUI process, including refreshes and planning updates, and they override that default. `ask_user` pauses its session without opening a modal: the session is marked **Awaiting Input**, and its question is shown only in that session's **Plan** tab. Press `o` or choose **Answer Prompt** on that session to open the Plan tab and focus the detail pane, press `Enter` to edit, `Enter` again to submit, and `Esc` to leave editing while keeping the draft. Execution-time Options remain queued in the existing modal; a single-run Option opens automatically, while Run All shows a queue badge. Delete, Discard, Reset to Planned, Publish as Issue, Stage Local Merge, Merge Locally & Cleanup, Merge PR (a preview modal where you pick Squash, Merge, or Rebase and press Enter), Clean, Run All / Cancel Run All, and quitting while work is active ask for confirmation; Cancel, Approve, Run, Resume, Retry, Generate Plan, and Open PR execute immediately. Cancelling a run moves its session to `Suspended`; cancelling planning restores the prior state and plan. In New Session, a non-blank task or an image attachment is required and the GitHub source requires a repository; a blank working directory means `.` and a blank workflow config means auto-detect. Terminal state is restored on normal exit, panic, SIGTERM, and SIGHUP. Session errors stay in the app and session state; only terminal/root/event-loop failures exit the TUI.

For interactive planning questions, line breaks are rendered as adjacent lines in the selected session's Plan tab. The answer field is inline, session-scoped, and not a modal. `Option` requests retain the modal queue and its existing `o`/Run All behavior.

The Sessions sidebar shows a presentation status, which can differ from the persisted session **Phase** while planning, running, or waiting for a user action. A filled `●` marks **Awaiting Input** and **Awaiting Approval** in light blue, **Running** and **Planning** in yellow, **Planned** in blue, **Completed** in green, **Failed** and **Plan Failed** in red, and **Suspended** in purple. A draft uses a green `◯`. The status text remains visible when `NO_COLOR` is set.

#### Keyboard map

The TUI is keyboard-only. Keys are fixed and cannot be configured:

| Key | Action |
|-----|--------|
| `1` / `2` / `3` | Switch to Sessions / New Session / Run All |
| `n` | Open New Session at its first question, Source; choose the location and config before typing the task |
| `r` | Refresh |
| `L` | Choose the UI language or `auto` and save it in application configuration |
| `?` | Show help |
| `q` / `Ctrl-C` | Quit; an active quit confirms and then cancels work |
| `Ctrl-P` | Create the New Session from the current answers and start normal planning |
| `Ctrl-G` | Create the New Session from the current answers and start grill planning |
| `Ctrl-U` | Create the New Session from the current answers using the input directly as the plan |
| `Ctrl-S` | Save the New Session answers as a draft |
| `Tab` / `Shift-Tab` | Next / previous question (Tab completes a path first when one matches, including arbitrary workflow config paths); in Sessions, toggle focus between the sidebar and the detail pane |
| Arrow keys / `j` / `k` / `PgUp` / `PgDn` / `Home` / `End` | Navigate lists and choices. In the dialogue, `Up` / `Down` select workflow config candidates, recall recent directories or `gh` repositories, or move through the skipped-step list. In text questions, `Left` / `Right` / `Home` / `End` edit text, and `j` / `k` are entered as text; `j` / `k` navigate non-text choices. In Sessions, with the sidebar focused they switch sessions (`Right` focuses the detail pane); with the detail pane focused, `Up` / `Down` / `PgUp` / `PgDn` / `Home` / `End` scroll Info, Plan, and Log or move the Graph node selection, and `Left` / `Right` switch detail tabs |
| `[` / `]` | Move between detail tabs |
| `a` | Open the action palette |
| `p` | On Run All, edit the shared parallelism setting; `Enter` saves and `Esc` cancels |
| `e` | On Run All, edit the automatic requeue limit; `Enter` saves and `Esc` cancels |
| `o` | In Sessions, open the selected session's Ask in its Plan tab; otherwise handle the Option queue or open a dedicated PR/Issue URL |
| `f` | Follow the log |
| `c` | On Sessions, ask for confirmation to Clean reclaimable and closed-PR sessions |
| `Ctrl-R` | In the multiline Edit Settings dialog, toggle between save only and save and regenerate |
| `Enter` | In a Plan Ask, start editing or submit the answer; otherwise accept the New Session answer and move to the next question, start or draft the session, or insert a task/image newline |
| `Ctrl-Enter` | Advance the current New Session question (required in the task and image editors, where `Enter` inserts a newline); in multiline Edit Settings, save (and regenerate if selected) |
| `Space` | Toggle the current choice or the highlighted skipped step |
| `Esc` | Leave Plan Ask editing without discarding the draft; otherwise back one question or, at the first question, return to Sessions |

#### Layout, logs, and process behavior

- At **120 or more columns**, the layout uses a fixed **34-column sidebar** and a detail pane.
- At **80--119 columns**, Sessions and Run All stack their two panes vertically (sidebar/summary above detail/log).
- Below **80x24**, the TUI shows a resize notice.
- In Sessions, the focused pane (sidebar or detail) has a thick border.
- `NO_COLOR` is honored; labels and statuses are never conveyed by color alone.
- Idle updates are event-driven. External state is polled every 3 seconds, and active work uses a 100 ms spinner.
- The in-memory session log is bounded to the latest 10,000 lines; the Run All view is bounded to the latest 2,000 lines. Complete per-session run output remains at `$XDG_DATA_HOME/cruise/sessions/<session-id>/run.log` (by default `~/.local/share/cruise/sessions/<session-id>/run.log`).
- The TUI cannot hand its stdin to an interactive child command: child processes never share TUI stdin, so such commands cannot prompt through the TUI.
- Concurrent mutation of the same session by separate cruise processes is unsupported. Within one TUI process, distinct sessions can run concurrently, and Run All uses the configured run-all parallelism.
- The TUI delivers configured desktop, sound, and webhook notifications asynchronously for action-required prompts, ready plans, completed runs, and failures, without blocking rendering or changing workflow state. Set `CRUISE_DISABLE_NOTIFICATIONS=1` to suppress all three channels.

The CLI remains the canonical client for automation, JSON (`cruise list --json`), and CI/non-interactive use. See the [CLI Reference](cli-reference.md#cli-reference) for the existing command workflows.

### Workflow packages

`cruise workflow add <owner/repo[/path.yaml][@ref]>` installs from GitHub into `$XDG_CONFIG_HOME/cruise/workflows/` (default `~/.config/cruise/workflows/`). With no explicit path, a root `cruise-package.yaml` selects a bundle; otherwise the single workflow defaults to `cruise.yaml`. An explicit YAML path selects a single workflow, except that paths ending in the reserved basename `cruise-package.yaml` or `cruise-package.yml` are rejected. The ref defaults to the repository's default branch and is pinned to a commit. Use `--name` to override the package name (`[A-Za-z0-9_-]+`).

A bundle manifest explicitly lists the workflows to install, for example:

```yaml
description: Review and implementation workflows
workflows:
  - name: review
    path: workflows/review.yaml
  - name: implement
    path: workflows/implement.yaml
```

Names must be unique ignoring case and contain only letters, digits, `_`, or `-`. Paths are unique repository-root-relative `.yaml` or `.yml` paths without empty, `.` or `..` components or backslashes, and must not end in `cruise-package.yaml` or `cruise-package.yml`. Unknown manifest keys are rejected. Only listed workflows are installed, not every YAML in the repository. Nested `workflow_call` steps and `prompt_file` contents are inlined at installation, leaving self-contained YAMLs.

Single packages retain `workflows/<name>.yaml` and `<name>.cruise-package.json`. Bundles use `workflows/<name>/<workflow>.yaml` and `.cruise-package-lock.json`, which records the source, ref, commit, and each installed YAML's SHA-256. Bundle members appear as `<name>/<workflow>.yaml` in the normal config selector. Existing single packages are not migrated to bundles.

`add` and `update` preview the source, commit, workflows and commands, warn about untrusted commands, and ask `[y/N]`. Without a TTY, pass `--yes` (the preview is still printed). Installing never runs the workflow. `update <name>` re-resolves the recorded ref without changing package kind, `remove <name>` removes the package, and `list` shows built-ins and installed packages. Bundle changes are staged as a unit. `update` and `remove` refuse edited YAMLs and bundles containing extra files, protecting user changes.

Use `cruise workflow doctor <name>` to check an installed package, including whether its YAML has been modified.
