**Languages:** [English](../en/usage.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/usage.md) · [简体中文](../zh/usage.md) · [Index](README.md)

<a id="webui"></a>
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

`start` and `restart` accept the same server options shown below; put them after the subcommand. `stop --force` permits forced termination if graceful shutdown cannot complete. `status` reports a stopped instance with a nonzero exit status. Stopping the WebUI is not a request to delete sessions. Lifecycle commands manage the local instance.

`restart` reuses the previous host and port unless overridden. Stopping and restarting request cancellation of active WebUI operations. Running sessions are not deleted; they are suspended so they can be resumed later.

The WebUI instance record (`$XDG_STATE_HOME/cruise/webui/instance.json`, normally under `~/.local/state/cruise/webui/`) contains control credentials, so keep it private.

The default URL is `http://127.0.0.1:8484/`. Use these flags to customize it:

- `--host <ADDR>` -- interface to bind (default `127.0.0.1`; the WebUI has no authentication, so do not bind it to an untrusted network).
- `--port <PORT>` -- TCP port to listen on (default `8484`).
- `-o`, `--open` -- open the WebUI in the default browser after the server starts (by default the browser is not opened; visit the printed URL yourself).
- `--webui-dir <DIR>` -- serve `templates/` and `static/` from disk for development. For example, `cruise webui --webui-dir webui`.
- `Ctrl-C` -- stop the server. Active operations are canceled (sessions become `Suspended`).

The Graph tab loads Mermaid from jsDelivr and therefore needs network access; all other WebUI tabs work offline.

Choose the display language in the sidebar footer on desktop or the header on mobile. The selection reloads the page.

<a id="usage"></a>
# Usage

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

The CLI and TUI make a best-effort attempt to send desktop, sound, and Slack webhook notifications for workflow events **Action required**, **Plan ready**, **Completed**, and **Failed**. Desktop notifications require a macOS or Linux desktop session; sound is off by default. Delivery failures do not change workflow results or exit status. Set `CRUISE_DISABLE_NOTIFICATIONS=1` to disable all three channels. See [Notifications](cli-reference.md#notifications) for settings and WebUI behavior.

<a id="herdr-integration"></a>
## herdr integration

Inside a [herdr](https://herdr.dev) pane (`HERDR_ENV=1` with `HERDR_PANE_ID` and `HERDR_BIN_PATH` set), the TUI, `cruise plan`, `cruise run`, and `cruise exec` report their lifecycle state. Reporting is best-effort and does not affect workflow success, failure, or exit status. Set `CRUISE_DISABLE_HERDR=1` to disable it.
From a herdr pane, use `cruise tell <session-id> "instruction"` to instruct a running session.

<a id="tui-interactive-keyboard-client"></a>
## TUI (Interactive Keyboard Client)

Running `cruise` with no arguments opens Cruise's official interactive keyboard client: the third client beside the CLI and WebUI. It preserves existing CLI behavior while bringing the WebUI's session-management workflows to a terminal.

```sh
# Open the interactive client
cruise
```

Typical flow: run `cruise`, press `n`, choose the Source, working directory or repository, and workflow config, then type the task. Press `Ctrl-P` for normal planning, `Ctrl-G` for grill planning, or `Ctrl-U` to use the input directly as the plan, or press `Tab` to answer the remaining questions one at a time and choose the launch mode at the end. After Task, the remaining questions are Images, declared workflow flags (if any), skipped steps (flag-disabled steps are hidden), Workspace (Directory sessions only), Dirty tree when applicable, and Launch. Press `1` to inspect or act on sessions, then `3` to run the planned queue in **Run All**. In Run All, press `p` to edit shared parallelism, enter a positive integer, then press `Enter` to save or `Esc` to cancel.

The TUI requires an interactive TTY on macOS, Linux, or Windows. It is not an automation interface: use the CLI for automation, JSON output, and CI. GitHub workflows require the external [`gh` CLI](https://cli.github.com/); the TUI does not bundle or replace it. Current-branch-only work does not require `gh` (`glab` for GitLab repositories).

<a id="screens-and-workflows"></a>
### Screens and workflows

The TUI has three views:

- **Sessions** -- Browse the global session list. Select a session to view its **Info**, **Graph**, **Plan**, **Log**, or **Diff** detail tab. The [`cruise list` Actions](sessions.md#cruise-list-actions) matrix describes the list menu, not the full TUI action set. It also supports Ask and Option prompts, Clean, worktree/current-branch selection, Publish as Issue, and PR links.
- **New Session** -- Create a session or draft through a step-by-step dialogue: one question is shown at a time, with completed answers above and remaining questions below. The order is Source, working directory or repository, workflow config, task, images, workflow flags (if declared by the config), skipped steps, workspace mode, dirty-tree allowance (current-branch runs only), and finally launch mode. Questions made moot by earlier answers are skipped.
- **Run All** -- Run Planned or Suspended sessions with live parallelism, in-app status, and bell feedback.

PR and Issue URLs are shown as text; a successful Publish as Issue also opens the new issue URL automatically. Other Markdown links remain textual. CLI-only `config` and `exec` operations are available through their CLI commands, not TUI screens.

A selected session opens on its **Plan** tab while planning is active or its phase is **Awaiting Input**, **Awaiting Approval**, or **Planned**, and on **Info** otherwise. `ask_user` pauses its session without opening a modal: the session is marked **Awaiting Input**, and its question appears only in that session's **Plan** tab. Press `o` or choose **Answer Prompt** on that session to open the Plan tab and focus the detail pane. Press `Enter` to start editing, `Enter` again to submit, and `Esc` to leave editing with the draft intact. Canceling a run leaves the session `Suspended`; canceling planning restores the previous state and plan. In New Session, the task must be non-blank or include at least one image attachment, and a GitHub source requires a repository.

For interactive planning questions, line breaks are rendered as adjacent lines in the selected session's Plan tab. The answer field is inline, session-scoped, and not a modal. `Option` requests retain the modal queue and its existing `o`/Run All behavior.

The Sessions sidebar shows a presentation status, which can differ from the persisted session **Phase** while planning, running, or waiting for a user action. A filled `●` marks **Awaiting Input** and **Awaiting Approval** in light blue, **Running** and **Planning** in yellow, **Planned** in blue, **Completed** in green, **Failed** and **Plan Failed** in red, and **Suspended** in purple. A draft uses a green `◯`. The status text remains visible when `NO_COLOR` is set.

<a id="keyboard-map"></a>
### Keyboard map

The TUI is keyboard-only. Keys are fixed and cannot be configured:

| Key | Action |
|-----|--------|
| `1` / `2` / `3` | Switch to Sessions / New Session / Run All |
| `n` | Open New Session at its first question, Source; choose the location and config before typing the task |
| `r` | Refresh |
| `L` | Choose the UI language or `auto` and save it in application configuration |
| `?` | Show help |
| `q` / `Ctrl-C` | Quit; if work is active, confirm and then cancel it |
| `Ctrl-P` | Create the New Session from the current answers and start normal planning |
| `Ctrl-G` | Create the New Session from the current answers and start grill planning |
| `Ctrl-U` | Create the New Session from the current answers using the input directly as the plan |
| `Ctrl-S` | Save the New Session answers as a draft |
| `Tab` / `Shift-Tab` | Next / previous question (Tab completes a path first when one matches, including arbitrary workflow config paths); in Sessions, toggle focus between the sidebar and detail pane |
| Arrow keys / `j` / `k` / `PgUp` / `PgDn` / `Home` / `End` | Navigate lists and choices. In the dialogue, `Up` / `Down` select workflow config candidates, recall recent directories or `gh` repositories, or move through the skipped-step list. In text questions, `Left` / `Right` / `Home` / `End` edit text, and `j` / `k` are entered as text; `j` / `k` navigate non-text choices. In Sessions, with the sidebar focused they switch sessions (`Right` focuses the detail pane); with the detail pane focused, `Up` / `Down` / `PgUp` / `PgDn` / `Home` / `End` scroll Info, Plan, and Log or move the Graph node selection, and `Left` / `Right` switch detail tabs |
| `[` / `]` | Move between detail tabs |
| `a` | Open the action palette |
| `p` | On Run All, edit shared parallelism; `Enter` saves and `Esc` cancels |
| `e` | On Run All, edit the automatic requeue limit; `Enter` saves and `Esc` cancels |
| `o` | In Sessions, open the selected session's Ask in its Plan tab; otherwise handle the Option queue or open a dedicated PR/Issue URL |
| `f` | Follow the log |
| `c` | On Sessions, confirm Clean for reclaimable and closed-PR sessions |
| `Ctrl-R` | In the multiline Edit Settings dialog, toggle between save only and save and regenerate |
| `Enter` | In a Plan Ask, start editing or submit the answer; otherwise accept the New Session answer and move to the next question, start or save the session as a draft, or insert a task/image newline |
| `Ctrl-Enter` | Advance the current New Session question (required in the task and image editors, where `Enter` inserts a newline); in multiline Edit Settings, save (and regenerate if selected) |
| `Space` | Toggle the current choice or the highlighted skipped step |
| `Esc` | Leave Plan Ask editing without discarding the draft; otherwise go back one question or, at the first question, return to Sessions |

<a id="workflow-packages"></a>
## Workflow packages

`cruise workflow add <owner/repo[/path.yaml][@ref]>` installs from GitHub into `$XDG_CONFIG_HOME/cruise/workflows/` (default `~/.config/cruise/workflows/`). If no path is specified and the repository root contains `cruise-package.yaml`, it selects a bundle; otherwise the single-workflow default is `cruise.yaml`. An explicit YAML path selects a single workflow. The ref defaults to the repository's default branch. Use `--name` to override the package name (`[A-Za-z0-9_-]+`).

A bundle manifest explicitly lists the workflows to install, for example:

```yaml
description: Review and implementation workflows
workflows:
  - name: review
    path: workflows/review.yaml
  - name: implement
    path: workflows/implement.yaml
```

Names must be unique ignoring case and contain only letters, digits, `_`, or `-`. Paths must be unique repository-root-relative `.yaml` or `.yml` paths. Only listed workflows are installed, not every YAML in the repository.

Single packages retain `workflows/<name>.yaml` and `<name>.cruise-package.json`. Bundles use `workflows/<name>/<workflow>.yaml`. Bundle members appear as `<name>/<workflow>.yaml` in the normal config selector. Existing single packages are not migrated to bundles.

`add` and `update` preview the source, commit, workflows, and commands, warn about untrusted commands, and ask `[y/N]`. Without a TTY, pass `--yes` (the preview is still printed). Installing never runs the workflow. `update <name>` re-resolves the recorded ref without changing package kind, `remove <name>` removes the package, and `list` shows built-in and installed packages. Bundle members are staged as a unit. `update` and `remove` refuse edited YAMLs and bundles containing extra files, protecting user changes.

Use `cruise workflow doctor <name>` to check an installed package, including whether its YAML has been modified.
