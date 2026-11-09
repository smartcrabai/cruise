# cruise

<p align="center">
  <img src="docs/icon.png" alt="cruise logo" width="160" height="160">
</p>

A CLI tool that orchestrates coding agent workflows defined in a YAML config file.

Cruise wraps CLI coding agents such as `claude -p` and drives them through a declarative workflow: plan → approve → write tests → implement → test → review → optionally open a PR and run after-PR automation. It handles variable passing between steps, conditional branching, and loop control.

> **Note:** Cruise runs on macOS, Linux, and Windows (x86-64, `x86_64-pc-windows-msvc`). Development and testing happen primarily on macOS; Linux and Windows have been verified less extensively.

## Prerequisites

- [`gh` CLI](https://cli.github.com/) -- required for GitHub worktree sessions when automatic PR creation is enabled (the default), PR-backed cleanup, or an opted-in native workflow merge; `glab` for GitLab repositories. A local worktree workflow with `create_pr: false` does not need a forge CLI, but `--repo` still needs `gh`/`glab` to access and clone the repository.
- [`glab` CLI](https://gitlab.com/gitlab-org/cli) -- required instead of `gh` for GitLab repositories (merge request creation, cleanup, issue publishing, and opted-in native synchronous squash merge). The forge is detected from the host (`github.com` / `gitlab.com`); set `CRUISE_FORGE=github|gitlab` for self-managed hosts (and `GITLAB_HOST` for `owner/repo`-style `--repo` locators). `--repo` accepts `owner/repo`, nested `group/sub/project` (with `CRUISE_FORGE=gitlab`), or a full URL. On GitLab, `{pr.number}` / `{pr.url}` refer to the merge request, and the "post an `@cruise run` comment" option when publishing issues is unavailable.
- [`jcode` CLI](https://github.com/1jehuang/jcode) v0.88.0 or newer -- required by the default SDK backend (`sdk: jcode`); sign in with `jcode login <provider>` after installing. For SDK execution, install jcode whenever an effective execution or planning candidate uses jcode, including SDK-qualified array entries under another workflow SDK.
- [`claude` CLI](https://code.claude.com/docs/en/quickstart) -- required whenever an effective execution or planning candidate uses claude, including SDK-qualified array entries. Cruise drives it in-process. Verified against 2.1.250; a `:effort` model suffix maps to `claude --effort`, so a CLI without that flag fails the step with `unknown option '--effort'` (a permanent error, not retried). Authentication is the CLI's own, unrelated to `jcode login`.
- `opencode` CLI with the V2 API, required whenever an effective execution or planning candidate uses opencode, including SDK-qualified array entries. Cruise starts and stops it locally and uses your existing OpenCode provider configuration and authentication. V1 is not supported.
- An OpenSSH client (`ssh`) on the local `PATH` for `cruise ssh`.
- On Windows: [Git for Windows](https://gitforwindows.org/) (`git` on `PATH`). `command:` steps run through `cmd.exe /C`, so use cmd.exe syntax (or invoke `powershell -Command ...` / `bash -c ...` explicitly) rather than POSIX shell syntax. Desktop notifications and notification sounds are unavailable, but configured webhooks are sent. `cruise ssh` needs the Windows OpenSSH client, and herdr reporting is limited to what the herdr environment variables provide.
- A compatible `cruise` version and its usual workflow prerequisites on the SSH destination, including any required `gh`, `jcode`, `claude`, or `opencode` CLI for the selected workflow. The remote host performs the workflow and owns its authentication, configuration, sessions, worktrees, and clones.
- Install a managed provider on that host with, for example, `cruise ssh <host> -- install jcode`.

## Installation

### Windows (PowerShell)

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.ps1 | iex"
```

### macOS and Linux (shell installer)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh | sh
```

Or download the installer to a file and run it only if the download succeeds:

```sh
curl --proto '=https' --tlsv1.2 -fLsS --output cruise-installer.sh \
  https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh \
  && sh ./cruise-installer.sh
```

The shell and PowerShell installers place `cruise` in `$CARGO_HOME/bin` (normally `~/.cargo/bin`; on Windows, `%USERPROFILE%\.cargo\bin` when `CARGO_HOME` is unset). Keep that directory on your `PATH`. Supported managed installs can use `cruise update`; see [Updating Cruise](#updating-cruise) below. To install a pinned release, replace `/releases/latest/` in the installer URL with `/releases/download/v<version>/`, for example `v0.2.15`. Keep Homebrew installations managed with Homebrew.

### Homebrew

```sh
brew install smartcrabai/tap/cruise
```

Upgrade a Homebrew installation with:

```sh
brew upgrade smartcrabai/tap/cruise
```

### Updating Cruise

For a supported managed installation, check the version before and after updating:

```sh
cruise --version; cruise update; cruise --version
```

`cruise update` fetches the latest official stable release. It does nothing when that version is the same as or older than the installed version. Native updates are supported for these release targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`, and `x86_64-pc-windows-msvc`.

| Installation | Update method |
|---|---|
| Shell or PowerShell installer in canonical `$CARGO_HOME/bin` | `cruise update` |
| Source-built installation in canonical `$CARGO_HOME/bin` | `cruise update`; it installs the official runtime, not your local source build or its build-time changes |
| Homebrew | `brew upgrade smartcrabai/tap/cruise` |
| GitHub Action, Docker image, or another external installation | Use that distribution’s original update method |
| Any installation with `CRUISE_RUNTIME` set | Unset the override and run `cruise update` from the managed launcher |

“Managed” is determined by the canonical `$CARGO_HOME/bin` location, not Cargo’s install ledger. The updater replaces only the runtime selection: the fixed `cruise` launcher (`cruise.exe` on Windows) remains in place; each update adds a versioned `cruise-runtime-managed-…` runtime and `.json` sidecar and switches `.cruise-runtime-active`. Older runtimes are retained. Native update does not run Cargo or change its install ledger. If you need a custom source-built runtime, rebuild and install it from source instead.

Published versions without the updater—including v0.2.16—cannot update themselves. An updater-capable build can select v0.2.16, but that published runtime cannot perform a subsequent native update. Bootstrap by installing an updater-enabled release with its platform installer, or rebuilding/installing that source version; use `cruise update` for later updates only when the installed runtime includes the updater.

The downloaded release is checked over HTTPS against its release manifest and SHA-256 checksum. The checksum is distributed by the same release publisher; this is not an independent digital signature.

`cruise update --status` lists recorded transactions for all installations in sorted order. It is read-only: it does not contact the network, acquire the update lock, repair state, or prove which runtime is currently active. The transaction file is `$XDG_STATE_HOME/cruise/installations/<installation-id>/update-transaction.json` (default `~/.local/state/cruise/installations/<installation-id>/update-transaction.json`; on Windows, `%LOCALAPPDATA%\cruise\state\installations\<installation-id>\update-transaction.json` when `XDG_STATE_HOME` is unset). A failed transaction keeps its phase and error. Retry with `cruise update`; it reconciles an interrupted transaction before starting another download. Pre-activation failures leave the old selection in place. If the post-activation launcher probe fails, Cruise restores the previous selection; if recovery cannot prove the selected runtime, leave the record and use the reported error to guide manual recovery rather than deleting transaction files.

If Cruise reports that an update was applied but transaction completion or cleanup warned, the new runtime is already active; it was not rolled back. Inspect `cruise update --status`, then retry `cruise update` to recover or clean the recorded transaction. A status phase is metadata, not proof of activation; check the running version with `cruise --version`.

Updating Cruise does not replace the code in an already running WebUI process. After an update, use `cruise webui status` to compare its version with the current runtime and `cruise webui restart` to load the updated runtime. Restart cancels active WebUI operations, so choose a suitable time.

Versioned runtimes can coexist in `$CARGO_HOME/bin` (on Windows, `%USERPROFILE%\.cargo\bin` by default). To pin one already-installed version, verify that exact versioned sibling with `--version`, ensure its adjacent `.json` sidecar belongs to the current fixed launcher/runtime baseline, then set `.cruise-runtime-active` to that runtime’s exact basename. Do not delete the other runtime files. The baseline is the fixed launcher/runtime size-and-mtime stamp: a reinstall that changes it invalidates the overlay, while a same-size, same-mtime replacement is not detected. Legacy pointers or pinning an older launcher may still require manual retirement. To retire the pointer, first verify the fixed sibling directly; archive and move the pointer only then:

```sh
# macOS / Linux
bin="${CARGO_HOME:-$HOME/.cargo}/bin"
archive="$bin/.cruise-runtime-active.archive"
retired="$bin/.cruise-runtime-active.retired"
test ! -e "$archive" && test ! -e "$retired" &&
  "$bin/cruise-runtime" --version &&
  cp -n "$bin/.cruise-runtime-active" "$archive" &&
  mv -n "$bin/.cruise-runtime-active" "$retired" &&
  "$bin/cruise" --version
```

```powershell
# Windows
$bin = if ($env:CARGO_HOME) { Join-Path $env:CARGO_HOME 'bin' } else { Join-Path $HOME '.cargo\bin' }
$runtime = Join-Path $bin 'cruise-runtime.exe'
$pointer = Join-Path $bin '.cruise-runtime-active'
$launcher = Join-Path $bin 'cruise.exe'
$archive = "$pointer.archive"
$retired = "$pointer.retired"
if ((Test-Path -LiteralPath $archive) -or (Test-Path -LiteralPath $retired)) { throw 'Choose unused archive and retired paths.' }
& $runtime --version
if (-not $? -or $LASTEXITCODE -ne 0) { throw 'The fixed sibling runtime did not pass --version; pointer was not moved.' }
Copy-Item -LiteralPath $pointer -Destination $archive -ErrorAction Stop
Move-Item -LiteralPath $pointer -Destination $retired -ErrorAction Stop
& $launcher --version
```

The first `--version` above must succeed before moving the pointer. These commands only archive and move the pointer file; never delete a runtime or sidecar. Choose unused archive/retired names if those files already exist.

### Managed provider CLIs

Install only the providers your workflows need:

```sh
cruise install jcode
cruise install claude
cruise install opencode
cruise install --status
cruise install jcode --force
```

This build pins jcode **0.99.1**, claude **2.1.250**, and opencode **2.1.3**. Downloads are checked against digests pinned in Cruise and the extracted binary's version is checked before publication. No vendor installer, npm CLI, or shell configuration changes are required. Managed files live under `$XDG_DATA_HOME/cruise/providers/<provider>/` (normally `~/.local/share/cruise/providers/`). Each installation uses a distinct version-prefixed directory, with the active relative path recorded in `install.json`. Previous directories are retained so concurrent readers can continue using their resolved paths. Availability depends on platform. Authentication remains the provider's responsibility.

When jcode is installed only by Cruise, find its active `relative_path` in `providers/jcode/install.json` under the Cruise data directory, then invoke that binary with `login <provider>` or `auth status`. On Windows, the default data directory is `%LOCALAPPDATA%\cruise\data` when `XDG_DATA_HOME` is unset and `LOCALAPPDATA` is set. If `LOCALAPPDATA` is unavailable, the home-directory fallback applies. Quote paths containing spaces.

Resolution uses a nonempty process environment override first, then a valid managed installation matching the current pin, then the existing PATH lookup. The overrides are `CRUISE_JCODE_CLI_PATH`, `CRUISE_CLAUDE_CLI_PATH`, and `CRUISE_OPENCODE_CLI_PATH`. An invalid explicit override is an error, not a fallback. These are process environment variables, not workflow `env:` settings. A missing, stale, or damaged managed installation falls back to PATH. Running workflows, planning, TUI, and WebUI never download or install providers.

`--status` is read-only: it does not access the network, execute provider binaries, or create files. Reinstall with `--force` when needed. Rerun `cruise install <provider>` after a Cruise update changes its pin. `cruise update` updates Cruise itself, not providers. For managed claude and opencode, Cruise disables their auto-updaters only when the respective environment variable is not already set. PATH and override installations retain their existing behavior. The GitHub Action continues to manage its own jcode installation and version input separately.

### WebUI

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
`CRUISE_DISABLE_NOTIFICATIONS=1` to disable all three channels. See [Notifications](#notifications) for settings and WebUI behavior.

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

- **Sessions** -- Browse the global session list. Select a session to view its **Info**, **Graph**, **Plan**, **Log**, or **Diff** detail tab. The [`cruise list` Actions](#cruise-list-actions) matrix describes the list menu, not the full TUI action set. The Info tab shows declared workflow flags and their on/off values. The Graph tab shows its node list plus the selected node's dependency and edge details, hiding flag-disabled steps except in legacy unversioned graph checkpoints, which are displayed without filtering; Markdown is parsed and styled. This includes Ask and Option prompts, Clean, worktree/current-branch selection, Publish as Issue, and PR links.
- **New Session** -- Create a session or draft through a step-by-step dialogue: one question is shown at a time with the answers so far listed above and the remaining questions below. The order is Source (local Directory or GitHub repository), working directory or repository, workflow config, task, images, workflow flags (shown only when the config declares flags), skipped steps (flag-disabled steps are hidden), workspace mode, dirty-tree allowance (current-branch runs only), and finally the launch mode (normal planning, grill planning, input-as-plan, or save as draft). Questions that earlier answers make moot are skipped. For local Directory sessions, the Workflow config question includes Auto-detect and organizes the same prioritized file candidates and built-in workflows as the CLI according to configured categories and Others visibility. GitHub sessions resolve Auto-detect in the cloned repository, so caller-local candidates are omitted, while an arbitrary path is still accepted. `Ctrl-P`, `Ctrl-G`, `Ctrl-U`, and `Ctrl-S` start or draft the session from any question with the current answers. Directory and path answers offer completion, and history is recalled with the arrow keys; draft and selection history are retained as described in [New Session Form Persistence](#new-session-form-persistence).
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

The CLI remains the canonical client for automation, JSON (`cruise list --json`), and CI/non-interactive use. See the [CLI Reference](#cli-reference) for the existing command workflows.

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

### CLI Reference

```
cruise [OPTIONS] [INPUT] [COMMAND]

Commands:
  plan         Create an implementation plan for a task
  draft        Save a task description as a draft without generating a plan
  run          Execute a planned session
  tell         Send an instruction to a running session
  list         List and manage sessions interactively
  clean        Remove sessions with closed/merged PRs
  usage        Summarize locally recorded token usage
  config       Show or update application-level configuration (`config.json` or `config.yaml`)
  exec         Execute a workflow or an instant task in the current directory (no plan, no worktree, no PR)
  ssh          Run a cruise command on a remote host through OpenSSH
  webui        Serve or manage the local browser UI (start, stop, restart, status)
  install      Install a pinned provider CLI or inspect managed provider status
  update       Update a managed native installation; `--status` lists recorded transactions read-only
  workflow     Manage workflows: list/eject built-ins, generate, interactively make or check YAML (`doctor`), install GitHub packages (`add`, `remove`, `update`)

Arguments:
  [INPUT]  Initial input (legacy: positional input without a subcommand uses `plan`)

Options:
      --plan <INPUT>           Create a plan in the background and return immediately (pass `stdin` to read piped stdin explicitly)
      --skip-planning          Use the input directly as the plan, skipping LLM-based plan generation
      --no-force-exec          Ignore `force_exec: true` in the workflow config and plan as usual
      --repo <OWNER/REPO>      Repository (owner/repository, group/sub/project, or full URL; GitHub or GitLab) to clone into a temporary directory for planning and execution
      --image <PATH>           Attach an image file (png/jpg/jpeg/webp/gif) to the planning input; can be repeated
      -f, --flag <NAME[,NAME...]> Enable named workflow flags; NAME=on|off|true|false also works (repeatable)
```

#### `cruise install`

`cruise install <jcode|claude|opencode> [--force]` installs the build's pinned provider version. `cruise install --status` inspects managed installations without network access or side effects. See [Managed provider CLIs](#managed-provider-clis) for resolution and authentication details.

#### `cruise workflow generate`

```
cruise workflow generate <DESCRIPTION> --name <NAME> [--user] [--config <PATH>]
```

Drafts a new workflow YAML from a description. The backend is chosen by the normal config resolution (`--config`, `CRUISE_CONFIG`, local files, user workflows, or the built-in `tdd` workflow). The file is saved to `./.cruise/<NAME>.yaml`, or to the user workflow directory (`~/.config/cruise/workflows/`) with `--user`. `NAME` may contain only letters, digits, `-` and `_`.

- The reply must be raw YAML (no Markdown fence). Each candidate receives doctor static checks with flags off and isolated defaults (reference resolution relative to the destination directory, config validation, retry budget, compile, graph checks). On failure the diagnostic and previous candidate are sent back for up to 3 repairs (4 backend turns in total). If it is still invalid, nothing is written.
- The validated YAML is saved exactly as returned. An existing file with the same name is never overwritten, and the backend is not called in that case.
- The generated workflow is not run and no session is created.
- Validation does not prove that the commands are safe or that the workflow succeeds. The selected agent backend keeps its normal permissions, so it may modify the repository during generation. Review the YAML before running it.

For interactive drafting and revision, use `cruise workflow make` instead.

#### `cruise workflow make`

```
cruise workflow make --name <NAME> [--user] [--config <PATH>] [--from <SELECTOR>] [DESCRIPTION]
```

Create a single workflow YAML through a conversation. Both stdin and stdout must be terminals; use `workflow generate` for non-interactive drafting. An optional description is submitted as the first request. `--config` selects the backend configuration, while `--from` supplies a read-only reference workflow, for example `builtin:tdd` or a file path. Cruise reads the reference without writing it, but an unsandboxed command backend could modify it. The destination is fixed at startup: `./.cruise/<NAME>.yaml`, or the user workflow directory with `--user`. Existing destinations are never overwritten, and the reserved project name `default` cannot be used.

Enter requests to create or revise the draft. The assistant may ask questions without changing it. Use `/show` to inspect the current draft and `/accept` to choose **Write workflow**, **Continue editing**, or **Cancel**. Escape or Ctrl+C at the request prompt cancels; at the approval menu they return to editing. Only choosing Write creates the destination, after re-reading and validating the staged YAML. The workflow is not run and no session is created.

Candidates pass the same `exec` preflight as `workflow generate`, with up to three repairs per request. If all attempts fail, the previous draft remains unchanged. The backend reply can include explanatory text and at most one `<cruise-workflow>` block; replies without a block are conversation only. Validation does not prove command safety, so review the result before running it.

Drafts and `conversation.jsonl` are retained under `$XDG_DATA_HOME/cruise/workflow-make/<id>/` (normally `~/.local/share/cruise/workflow-make/<id>/`) after writing or cancellation. The log includes requests. These directories are not removed by `cruise clean`; remove them manually when no longer needed.

The backend is asked to operate read-only in the project directory. Repository changes detected during a turn cause that turn to fail, including changes made concurrently by the user. Command backends are not sandboxed: change detection is not prevention and excludes some paths. Inspect the repository if a backend may have changed it.

#### `cruise workflow doctor`

```sh
cruise workflow doctor [--json] [--strict] [-f NAME[=on|off]]... [TARGET]...
cruise workflow doctor --all [--json] [--strict]
```

Doctor statically checks workflow YAML without executing it or creating a session, worktree, or project configuration. With no targets it checks `cruise.yaml`, `cruise.yml`, `.cruise.yaml`, `.cruise.yml`, and YAML files in `.cruise/`, excluding `.cruise/default.yaml`. `--all` additionally checks user workflows and all built-ins, and cannot be combined with targets or `-f`.

Targets can be paths, `builtin:<name>`, `__builtin__` (the default built-in), or bare user workflow or built-in names. A bare name matching both a user workflow and a built-in is ambiguous: use an explicit path or `builtin:<name>`. Repeat targets to check several workflows. Repeat `-f` or use comma-separated assignments to select flags for every target, for example `-f review,merge=off`. A flag not declared by a target is an error for that target. Without `-f`, flags are off.

Checks include local prompt and workflow references, configuration, routing targets, retry budgets, flag names, and graph reachability. Resolution uses the same defaults and environment overrides as execution. GitHub workflow references are resolved normally and may require network access. Doctor does not test backend, model, authentication, provider connectivity, command safety, or runtime-dependent template variables.

An unreachable step is a warning under the selected flags, not necessarily a defect: flag-gated paths can legitimately be unreachable. A reachable cycle that cannot reach a normal exit is an error when no normal exit is reachable from the start. If normal exit is possible but another reachable cycle cannot exit, doctor warns. Modified installed packages and deprecated language settings also produce warnings.

Human-readable reports go to stdout. `--json` emits a single object with `schema_version: 1`, `ok`, `exit_code`, `summary` (`checked`, `passed`, `failed`, `errors`, `warnings`) and ordered `targets`. Each target includes its target, kind (`project`, `user`, `builtin`, or `file`), package status, path, status (`ok`, `warning`, `error`, or `unresolved`), step counts, retry limit, declared flags, and ordered diagnostics. Diagnostics contain `level`, `code`, `stage`, nullable `step`, and `message`. Automations should use `code` and `step`, not message wording.

Exit status is `0` when checks pass (warnings are allowed), `1` for errors or warnings with `--strict`, and `2` for unresolved targets or no targets. Status `2` takes precedence over `1`; argument usage errors also exit `2`.

#### `cruise ssh`

```
cruise ssh [OPTIONS] <DESTINATION> [-- <CRUISE_ARGS>...]
```

`cruise ssh` starts one `ssh` client process and delegates the complete command to the remote `cruise`. With no forwarded arguments, a remote `cruise` starts its TUI when a PTY is available. The destination is an OpenSSH `[user@]host` or `~/.ssh/config` alias; use that config for ports, identity files, agents, and ProxyJump.

Options:

```
      --cwd <PATH>                 Change to this remote directory before starting cruise
      --cruise-bin <PATH>          Remote cruise executable or path [default: cruise]
      --tty <auto|always|never>    PTY policy: follow local TTYs, force a PTY, or disable one [default: auto]
```

Everything after `--` is passed to the remote cruise in the same order and is parsed there. In `auto` mode, a local TTY requests a PTY; piped or redirected streams do not, which keeps commands such as `cruise ssh host -- list --json` suitable for scripts. `--tty always` and `--tty never` override that choice.

The remote host is the execution boundary: its sessions, config, jcode authentication, worktrees, and repository clones are stored under its XDG directories and are not shared with local `cruise` commands. `--cwd`, `--config`, `--image`, and other path arguments are interpreted on the remote host. Issue input such as `#12` is resolved using the remote host's `gh` and authentication. Cruise does not upload local files, copy credentials or environment variables, install the remote binary, or synchronize repositories. The WebUI remains a local session client and does not browse remote sessions.

#### `cruise`

```
cruise
```

Running `cruise` with no arguments opens the keyboard-only TUI. It requires a macOS/Linux interactive TTY; see [TUI (Interactive Keyboard Client)](#tui-interactive-keyboard-client) for its screens, workflows, keymap, responsive layout, and log behavior. Use the CLI subcommands for automation, JSON, and CI.

#### `cruise plan`

```
cruise plan [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description

Options:
      --issue <N>                  Use GitHub issue N as the task
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
      --dry-run                    Print the plan step without executing it
      --no-force-exec              Ignore force_exec: true and plan as usual
      --skip-planning              Use the input directly as the plan, skipping LLM-based plan generation
      --grill                      Interview-style planning: the agent asks one question at a time, then writes the plan (requires the SDK backend and a TTY; conflicts with --skip-planning)
      --no-interactive-planning    Disable interactive planning tools for this session; the agent writes plan.md directly (conflicts with --grill)
      --repo <OWNER/REPO>          Repository (owner/repository, group/sub/project, or full URL; GitHub or GitLab) to clone into a temporary directory for planning and execution
      --pr <NUMBER>                Address reviews on an existing GitHub pull request
      --rate-limit-retries <N>     Maximum number of retries per LLM call (SDK fallback policies also use it for retryable 4xx, 5xx, and network failures and fallback switching) [default: 5]
      --image <PATH>               Attach an image file to the planning input; can be repeated
      -f, --flag <NAME[,NAME...]> Enable named workflow flags; NAME=on|off|true|false also works (repeatable)
```

**GitHub issue input.** Use `cruise plan '#12'`, `cruise plan --issue 12 'additional instructions'`, or put `#12` on the first line followed by additional instructions on later lines. The first line must contain only the issue reference. Root positional input, `--plan`, `draft`, and the TUI and WebUI New Session task field also accept this syntax. Only `plan` and `draft` accept `--issue`, not the root command or `exec`. Issue lookup uses `gh` and the current repository origin, or `--repo` when supplied. GitLab issue input is not supported. The issue title, body, labels, and comments become task input, and supported GitHub images are saved as session attachments before user-provided images. The issue number is included in the branch and a closing reference in the PR. Issue input opts out of `force_exec`. Retrieved issue content is untrusted and can contain prompt injection.

`cruise plan` creates an isolated git worktree at `$XDG_DATA_HOME/cruise/worktrees/<session-id>/` before invoking the LLM, so plan-phase edits never touch your working copy. The same worktree is reused by `cruise run` in Worktree mode, or cleaned up automatically when you pick Current-branch mode or cancel planning (`--repo` sessions drop it on approval and re-clone at run). Non-git directories plan in place with a warning, but `cruise run` still needs a git repository.

Successful plan generation sends a best-effort **Plan ready** notification through enabled channels, including a configured webhook. Non-cancellation generation failures send **Failed**; cancellation sends no notification, and notification delivery does not affect the existing cleanup or error path.

With `--skip-planning`, no LLM is called: the (trimmed) input is written straight to `plan.md`. A foreground `cruise plan --skip-planning` run on an interactive TTY still opens the approve-plan menu; automatic approval to `Planned` is used for non-TTY stdin and for the background root `cruise --plan --skip-planning` path. (With `force_exec: true`, the workflow executes directly instead unless issue input or another opt-out is used; pass `--no-force-exec` to opt out explicitly.) Empty or whitespace-only input is rejected. Use this when you've already written the plan yourself and just want cruise to execute it. The WebUI exposes the same behavior via the **"Use input as plan (skip LLM planning)"** checkbox on the New Session form (the submit button remains "Create & plan").

With `--grill`, SDK planning becomes an interview: the agent asks questions one at a time and recommends answers before writing `plan.md`. It requires an SDK backend (including the default `jcode` backend), an interactive terminal, and `interactive_planning: true`; otherwise Cruise reports an error and discards the new session. It conflicts with `--skip-planning` and applies only to initial planning; Fix/Ask turns, replans, drafts, and background planning use the standard prompt. The WebUI exposes the same behavior through the **"Grill me"** toggle on the New Session form (mutually exclusive with "Use input as plan").

With `--no-interactive-planning`, SDK planning uses the direct-to-file flow even when the workflow config has `interactive_planning: true`. The agent writes `plan.md` directly, as in `command` mode. The flag conflicts with `--grill`; it is equivalent to setting `interactive_planning: false` but only affects the current session. The WebUI exposes the same behavior via the **"Non-interactive planning"** checkbox on the New Session form (mutually exclusive with "Grill me").

**Built-in workflow flags.** The six general-purpose built-ins (`tdd`, `refactor`, `add-tests`, `add-formal-specs`, `research`, and `security`) declare `after_pr` and `merge`; every flag is off unless explicitly enabled. `pr-review-fix` declares no flags. `tdd` and `refactor` additionally declare `browser_e2e`, `app_e2e`, and `formal_methods`. For example, `cruise plan -c builtin:tdd -f browser_e2e,app_e2e "task"` enables only the two E2E flags, while `cruise plan -c builtin:tdd -f merge "task"` opts into after-PR automation and the native synchronous squash merge. `-f browser_e2e,app_e2e,merge` combines them. `merge` runs the same after-PR prelude as `after_pr`, once; `after_pr` alone never merges. The implied prelude does not add or rewrite `after_pr` in the session's sparse saved flag map.

- `browser_e2e` enables actual browser exercises, assertions, and failure fixing.
- `app_e2e` enables applicable mobile and desktop native-app exercises using repository/platform-native runners.
- `after_pr` enables after-PR automation without merging.
- `merge` opts into the same after-PR prelude plus a final native synchronous squash merge, with current readiness checks; it does not enable auto-merge, bypass policy, or use a merge queue.
- `formal_methods` enables workflow-level executable Quint, Alloy, and Gherkin specifications and CI; it is not an initial-plan mode.

`research` remains read-only with `create_pr: false`; `merge` never creates a PR or overrides that no-PR policy. If an opted-in after-PR action is unavailable because no PR is created (including `research`, `exec`, current-branch runs, or a no-commit run), Cruise skips it and prints the reason.

With `--repo <owner/repository>`, the session targets a GitHub or GitLab repository instead of the current directory. The repository is cloned via `gh repo clone` (or `glab repo clone` on GitLab) into `$XDG_DATA_HOME/cruise/clones/<session-id>/`, which becomes the session's base directory. Repo sessions always run in Worktree mode and still require `gh`/`glab` to access the repository. With `create_pr: false`, Cruise skips automatic commit, push, PR creation, `after-pr`, and PR-based cleanup while retaining the normal session, artifacts, and worktree/clone. Otherwise, the planning worktree and clone are removed once the plan is approved (only the branch name is kept), then `cruise run` re-clones and recreates them; the clone is removed after PR creation. A successful run with no new branch commits completes without attempting PR creation; existing branch commits remain eligible for a PR. Publication failures in either repo or local worktree sessions enter **PR Failed** and retain the workspace for **Create PR** recovery, rather than rerunning the workflow. A config inside the clone (or the built-in `tdd` when no config file is found) is copied to `sessions/<session-id>/config.yaml` so it stays readable after clone removal, including inlined `prompt_file` contents; an external `-c` path or `CRUISE_CONFIG` file is referenced in place instead. `--repo` also works with background planning (`cruise --plan "task" --repo owner/repository`). The WebUI exposes the same behavior via the **Directory / Repository (GitHub or GitLab)** source toggle on the New Session form, with a repository picker backed by `gh repo list` (or `glab repo list` when `CRUISE_FORGE=gitlab`) (free-form `owner/repository` input is accepted too).

**Fix reviews on an existing GitHub PR.** Run `cruise plan --pr 12` from the repository, or `cruise plan --repo owner/repo --pr 12 "focus on tests"`, approve the generated plan if prompted, then run `cruise run <session-id>`. The optional positional text is an additional instruction; this mode does not read stdin or use an LLM to generate the plan. TUI and WebUI new-session forms also accept a PR number for a GitHub repository. Authenticated `gh` is required. The PR must be open and have a nonempty head branch in the same repository; draft PRs are supported, but fork PRs and GitLab merge requests are not. `--grill` and `--image` cannot be combined with `--pr`.

The default is `builtin:pr-review-fix`, not the project workflow. A custom `-c` workflow must enable PR handling and include an after-PR `github-review` step with `reviewers: all`. The builtin declares no flags, synchronizes the existing head with `git pull --ff-only`, and addresses reviews without creating or merging a PR. The session is pinned to an isolated worktree on the PR head branch, tracking `origin/<head>`; a diverged branch or a branch already checked out elsewhere causes an error rather than a reset. Cleanup removes the worktree but preserves the PR head branch. The review loop commits and pushes fixes, replies or resolves eligible threads, and writes `review-report.json`. Failures leave a retryable session. Publish as Issue and Replan are unavailable for PR sessions.

#### `cruise draft`

```
cruise draft [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description (omit to prompt interactively; reads from stdin when piped)

Options:
      --issue <N>                  Use GitHub issue N as the task
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
```

Saves the input as a `Draft` session without invoking the LLM. The plan can be generated later by choosing **Generate Plan** from `cruise list`. Useful when you have an idea you want to capture immediately but don't want to start (or pay for) planning yet.

#### `cruise run`

```
cruise run [OPTIONS] [SESSION]

Arguments:
  [SESSION]  Session ID to execute (if omitted, picks from pending sessions)

Options:
      --all                        Run all planned or suspended sessions (live dashboard on interactive terminals for non-dry runs)
      --parallelism <N>            Max number of sessions `--all` executes concurrently (no flag default; 1 when omitted, must be >= 1, requires `--all`)
      --auto-requeue-max-attempts <N>  Maximum automatic retries per session (requires `--all`)
      --max-retries <N>            Maximum number of times a budgeted graph transition may be traversed (no flag default; falls back to the workflow config's top-level `max_retries`, else 3)
      --rate-limit-retries <N>     Maximum number of retries per step (SDK fallback policies also use it for retryable 4xx, 5xx, and network failures and fallback switching) [default: 5]
      --dry-run                    Print the workflow flow without executing it
      --cleanup-after-pr           Delete local worktree and branch after PR creation
      --no-cleanup-after-pr        Keep local worktree and branch after PR creation
      -f, --flag <NAME[,NAME...]> Enable named workflow flags; NAME=on|off|true|false also works (repeatable; conflicts with --all)
```

**Workflow flags.** A workflow config can declare `flags:` and attach them to steps with `flag: <name>`. A flagged step runs only when its effective value is on; absent flags are off. Enable one with `--flag NAME`, several with `--flag browser_e2e,app_e2e`, or repeat `--flag`; `-f` is an alias for `--flag`. Explicit `NAME=on|off|true|false` assignments remain supported, so `--flag NAME=off` overrides a saved enabled value. Resuming a session preserves saved flag values unless supplied flags override them. Unknown names are rejected, and `--flag` conflicts with `--all`. A fresh single-session CLI run with declared flags and TTY stdin prompts once to select optional workflow flags, unless any `-f/--flag` was supplied for that invocation (including `NAME=off`). Saved enabled values are preselected; Space toggles choices and Enter confirms, saving selected flags as on and unselected flags as off. With no selection, all declared flags are off. Resumes, dry runs, `run --all`, non-TTY stdin (including CI and pipes), and configs without flags skip this prompt and retain saved values plus explicit CLI overrides. TUI and WebUI Info tabs show all declared flags as `NAME=on|off`, with unspecified flags off. TUI New Session flags start off and Enter proceeds without requiring a selection; WebUI on/off selectors default to off. CLI, TUI, and WebUI skip selectors, and the WebUI and TUI Graph tabs, hide flag-disabled steps, except that legacy unversioned graph checkpoints are displayed without filtering. Approve and Execute now select flags before skips unless `-f/--flag` was supplied; Execute now forwards that selection without prompting again at execution start. The TUI Edit Settings dialog accepts `flag:NAME=on|off` lines. See [Top-level structure](skills/cruise-config/references/top-level.md#flags).

`--all` runs every Planned or Suspended session in sequence by default. Worktree mode is always forced (even if the session was originally started in current-branch mode). After all sessions finish, a summary table is printed showing the outcome and PR link for each session. If a session state file cannot be reloaded for the summary, that session is reported as `Failed` with the state path and error, and the batch still completes. `--all` and `[SESSION]` are mutually exclusive.

Each attempt that reaches a persisted `Completed` or `Failed` state sends a best-effort notification through enabled channels, including a configured webhook. This includes intermediate failures before an automatic retry. A Run All batch does not send an additional aggregate notification. Option prompts and interactive planning questions send **Action required** immediately before waiting for input.

`--parallelism <N>` is an invocation-scoped override that runs up to `N` sessions concurrently during `--all`. It defaults to `1` (sequential) when omitted, must be at least `1`, and is rejected unless `--all` is present. Each concurrent session still runs in its own worktree, failures in one session do not stop the other workers, and Ctrl+C suspends the running sessions and stops scheduling new ones. This flag does not read or modify the persisted WebUI/TUI setting (`cruise config --set-parallelism`).

`--auto-requeue-max-attempts <N>` overrides the persisted auto-requeue limit for this batch only. Unlike parallelism, CLI Run All reads the persisted limit when the flag is omitted. The default is `0` (disabled).

When stderr is an interactive terminal and `--dry-run` is not set, `run --all` shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output remains in that session's `sessions/{id}/run.log`. In non-TTY environments such as CI, or during `--dry-run`, it keeps the normal log output and final summary behavior.

#### `cruise exec`

```
cruise exec [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description bound to {input} (optional if your config doesn't reference {input})

Options:
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
      --instant                    Build a single-agent workflow without a workflow YAML (requires INPUT; conflicts with --config and --flag)
      --sdk <NAME>                 Instant backend: jcode, claude, or opencode (requires --instant)
      --model <MODEL>              Instant model (requires --instant)
      --review                     Add a bounded review loop (requires --instant)
      --review-rounds <N>          Maximum review passes, at least 1 (requires --review; default: 2)
      --verify <COMMAND>           Add a verification command (requires --instant; repeatable)
      --max-retries <N>            Maximum number of times a budgeted graph transition may be traversed (no flag default; falls back to the workflow config's top-level `max_retries`, else 3)
      --rate-limit-retries <N>     Maximum number of retries per step (SDK fallback policies also use it for retryable 4xx, 5xx, and network failures and fallback switching) [default: 5]
      --dry-run                    Print the workflow flow without executing it
  -f, --flag <NAME[,NAME...]>     Enable named workflow flags; NAME=on|off|true|false also works (repeatable; see Workflow flags)
```
Runs the workflow steps directly in the current directory: no plan is generated, no git worktree is created, and no PR is opened automatically. A transient session is recorded while the workflow runs, then automatically removed when it reaches a terminal phase. Sessions paused for input (`Running`) or interrupted with Ctrl+C (`Suspended`) are kept and can be resumed with `cruise run <id>`; exec sessions are excluded from `cruise run` automatic selection and `cruise run --all`. Existing uncommitted changes are allowed; cruise runs on top of them without stashing, committing, or resetting the working tree, and warns that workflow-generated files may be mixed with those changes. An attached branch is still required.

`--instant` explicitly opts into a workflow assembled in memory instead of resolving `CRUISE_CONFIG` or a local workflow file. Without it, `cruise exec` retains normal config resolution: with no config file, an interactive terminal offers built-in workflows in the categorized selector, while non-interactive execution selects `tdd` automatically. Instant requires a nonblank task and includes one implement step. Workflow defaults can prepend command pre-steps. It does not write a workflow into the repository; the existing session retains an inline config snapshot while the session exists. Workflow defaults and environment overrides still apply, and `CRUISE_SDK` / `CRUISE_MODEL` override `--sdk` / `--model`.

```sh
cruise exec --instant --sdk jcode --model my-model --review --review-rounds 2 \
  --verify 'cargo test' --verify 'cargo clippy' "implement the feature"
cruise exec --instant --dry-run "implement the feature"
```

Review fixes confirmed defects and repeats when the review pass changes files, up to the specified number of passes (default 2). A pass with no changes ends the loop; edits in the final allowed pass are not reviewed again. Verification commands run in the order supplied after implementation and any review; a failed command fails the run, not an automatic repair loop. Blank verification commands and commands containing braces are rejected. `--max-retries`, `--rate-limit-retries`, and `--dry-run` also apply; a `--max-retries` budget below the review limit is invalid. Instant declares no workflow flags and cannot be combined with `-c` or `-f`. It requires a git repository with an attached branch. Remote use is available with `cruise ssh <host> -- exec --instant "task"`.

When `force_exec: true` is set in the workflow config, the legacy positional form `cruise "task"` (except the exact inputs `ssh` and `mcp-bridge`, which are reserved for their subcommands), `cruise plan "task"`, and `cruise --plan "task"` use the current-directory execution path without planning, worktree, or PR creation. `--no-force-exec`, `--repo`, `--grill`, issue input, and image attachments opt out; `--skip-planning` and `--no-interactive-planning` do not. Background `--plan` runs foreground because there is no plan worker for direct execution. Both `exec` and these `force_exec` entry points use the same fresh-run flag selector on TTY stdin unless `-f/--flag` is supplied; dry runs and piped input (including `--plan stdin`) do not prompt. Resuming a retained exec session by ID keeps its saved flags without prompting.

#### `cruise --plan`

```
cruise --plan <INPUT|stdin> [--skip-planning] [--no-force-exec] [--repo <OWNER/REPO>] [--image <PATH>]... [--flag <VALUE>]...
```

Creates the session immediately, starts plan generation in a detached worker, and returns the new session ID. While the worker is still running, `cruise list` shows the session as `Planning`. If generation fails, the session remains in `AwaitingApproval` phase internally but `cruise list` shows `Plan Failed`, and approval stays disabled until planning succeeds.

Adding `--skip-planning` skips the background worker entirely: the input is written directly as `plan.md` and the session is created already in `Planned` — no approval step needed (with `force_exec: true` the workflow executes directly instead unless issue input or another opt-out is used; pass `--no-force-exec` to opt out explicitly). The flag also works without `--plan` (e.g. `cruise --skip-planning "task"`), in which case it behaves like `cruise plan --skip-planning "task"`.

`--repo <owner>/<repository>` is accepted here too and behaves as described under [`cruise plan`](#cruise-plan): the repository is cloned into a temporary directory and the session targets the clone. `--grill` is not available on this path — background planning has no interactive user to interview.

With no subcommand, no `--plan`, and no positional input, a non-TTY stdin is read to the end and its trimmed contents become the positional input, so `echo "task" | cruise` takes the legacy positional path and plans like `cruise plan "task"` (`--skip-planning`, `--no-force-exec`, `--repo`, and `--image` still apply). Blank piped input is ignored, and with an interactive stdin the argument-free invocation opens the TUI instead. Pass `--plan stdin` to read piped input explicitly on the background planning path.

#### `cruise tell`

```sh
cruise tell <session-id> "Also cover the timeout case"
printf '%s\n' 'Check the logs' | cruise tell <session-id> -
cruise tell <session-id> --log
cruise ssh devbox --cwd /srv/project -- tell <session-id> "Check the logs"
```

Submit to a live Running session only. Omit the instruction to read non-interactive stdin; `--log` reads instruction history without submitting. Acceptance is not delivery: the currently running agent step cannot see a new instruction. It is appended to the next agent prompt, including all prompt children of a parallel block, but not command or option steps. It cannot change permissions, commit policy, output format, or workflow transitions.

Instructions are trimmed and limited to 8,192 bytes each, with at most 20 pending instructions and 65,536 pending bytes. Delivered or expired items are terminal. Completed runs expire undelivered instructions; failed or suspended runs retain them for a subsequent run, and reset to planned expires them. Delivery is at least once: a crash or failure to record acknowledgement can cause redelivery. The session's XDG data directory stores `instructions.jsonl` and its lock; `--log` shows statuses. Do not put secrets in instructions.

#### `cruise list`

```
cruise list [OPTIONS]

Options:
      --json   Print all sessions as a JSON array to stdout instead of opening the interactive selector
```

With no flags, opens an interactive session browser whose menu depends on each session's phase (see [`cruise list` Actions](#cruise-list-actions)). With `--json`, prints every session as a JSON array (id, phase, input, PR URL, plan-error info, `pending_instructions`, ...) and exits -- useful for scripting or feeding session state to external tooling.

#### `cruise diff`

```
cruise diff <SESSION_ID> [--stat]
```

Review a session branch's patch, or use `--stat` for a summary. The comparison starts at the merge-base of the session branch and the base repository's current HEAD. As HEAD moves, the comparison base may change. When the worktree exists, the diff includes committed and uncommitted tracked changes and untracked files, excluding untracked paths listed in the base repository's `.worktreeinclude` and nested repositories. Tracked changes are included even when listed in `.worktreeinclude`. If the worktree is missing, only committed branch changes are shown. Patch output is limited to 1 MiB and at most 200 untracked files are included; truncation is reported.

The full patch uses `less -R` on a terminal when available, otherwise it is written directly to stdout. `--stat` is written directly to stdout. The list's **View Diff** action returns to the menu afterwards. The TUI and WebUI also have a Diff detail tab; refresh the TUI with `r` or use the WebUI tab's **Refresh** button to reload it.

A session needs a worktree branch for this feature. Current-branch sessions are not supported, and the branch may be removed during cleanup after PR creation (including the default `builtin:tdd` workflow). After cleanup, use **Open PR** to review the changes instead. Diff output is text, not JSON.

#### `cruise config`

```
cruise config [OPTIONS]

Options:
      --set-parallelism <N>
          Set the maximum number of sessions the WebUI and TUI run concurrently in `run --all` mode.

          Must be >= 1. Omit to show the current configuration. The CLI's one-shot `run --all --parallelism` override is independent.
      --set-language <LANG|auto>
          Set the UI language, or clear the saved setting with auto.
      --set-auto-requeue-max-attempts <N>
          Set the automatic retry limit for Run All (0 disables it).
```

Shows or updates application-level settings stored in `config.json` or `config.yaml` under `$XDG_CONFIG_HOME/cruise/` (default: `~/.config/cruise/`) -- this is separate from the per-workflow YAML configs. With no flags, prints the current configuration. `--set-parallelism <N>` sets `run_all_parallelism` (default `1`), which controls how many sessions the **WebUI and TUI** execute in parallel during `run --all`. The CLI does not read this persisted value; use the one-shot `cruise run --all --parallelism <N>` flag instead.

`cruise config --set-language ja` saves the application setting `uiLanguage` (`ui_language` is also accepted when reading). Use `--set-language auto` to clear it and resume automatic selection. The supported explicit tags are `en`, `zh-CN`, `es`, `fr`, `pt-BR`, `ja`, and `ko`. Case and underscores in place of hyphens are accepted, but other tags such as `pt` and `en-US` are not. `cruise config` reports the effective `ui_language` and its source: `env`, `config`, `system`, or `default`.

CLI and TUI UI language precedence is `CRUISE_UI_LANGUAGE` > application `uiLanguage` > system locale > `en`. A blank or `auto` environment value and an unset or `auto` config value leave that layer unspecified. Invalid environment values are warned about in English and ignored; the CLI setter and application-config validation reject invalid values. POSIX locale detection uses the first nonempty value of `LC_ALL`, `LC_MESSAGES`, `LANG`, and `LANGUAGE`, without trying later variables if that value is unsupported. On Windows, the user locale is consulted if environment variables do not determine a locale. On macOS and Linux only environment variables are consulted, so a GUI launch without them defaults to English. System locale matching accepts regional forms, including mapping Portuguese locales to Brazilian Portuguese and Simplified Chinese locales to `zh-CN`.

`cruise config --set-auto-requeue-max-attempts <N>` sets `run_all_auto_requeue_max_attempts` for CLI, TUI and WebUI Run All. The CLI flag `--auto-requeue-max-attempts <N>` overrides it for one batch. The limit is read once when a batch starts.

Set `prevent_sleep: true` in the machine-level `config.yaml` (or `"prevent_sleep": true` in `config.json`) to prevent sleep during runs on macOS; the default is `false`. Cruise shares `/usr/bin/caffeinate -dis -w <pid>` across overlapping enabled runs and stops it when the last one ends. This also covers CLI, TUI, and WebUI runs, but not planning alone. Other operating systems do nothing. Failures to load the setting or start caffeinate only warn and do not fail the run; a later run retries. Changes take effect on the next run. This setting is not supported in project `default.yaml`.

If both files exist, `config.json` wins; otherwise an existing `config.yaml` is used. When neither exists, settings updates create `config.json`. The files are not merged, and an invalid selected file is an error rather than a reason to try the other format. Updates write back to the selected file in its format; YAML comments are not retained.

##### Notifications

Set `notifications` in the application settings file, not the workflow YAML. Each channel has a master switch and optional event-specific switches:

```yaml
notifications:
  desktop: true   # default: true
  sound: true     # default: false
  webhook: true   # default: true, but requires the environment variable below
  events:
    actionRequired: { sound: false }
    planReady: { desktop: false }
    completed: { webhook: false }
    failed: { desktop: true, sound: true, webhook: true }
```

Event keys are `actionRequired`, `planReady`, `completed`, and `failed`; `action_required` and `plan_ready` are also accepted. Each event can specify `desktop`, `sound`, and `webhook`. An omitted event switch defaults to true, but cannot override a disabled channel master switch. Sound is independent of the desktop banner. On macOS and Linux it uses system sound players when available; playback failure is ignored. Windows sends neither desktop banners nor sounds, but supports webhooks.

Set `CRUISE_NOTIFY_WEBHOOK` in the Cruise process environment to a Slack Incoming Webhook URL. Do not put the secret URL in the settings file. An unset, empty, or invalid HTTP(S) URL disables webhook delivery. Cruise POSTs JSON of the form `{"text":"Cruise: <event> -- <detail>"}`, escaping Slack-sensitive `&`, `<`, and `>` characters; detail is limited to 60 characters. Treat the destination as receiving potentially sensitive task details. Requests have a 10-second timeout, do not follow redirects, and are not retried.

Delivery is best effort. A webhook failure is recorded without the URL in `$XDG_STATE_HOME/cruise/notification-webhook-error`; `cruise config` displays the last error classification and Unix timestamp when present, and a successful webhook clears it. Non-default notification settings also appear in `cruise config`, never the webhook URL. CLI delivery is synchronous, while TUI and WebUI delivery is dispatched off the UI path.

WebUI sessions use browser notifications instead of server-side desktop banners, plus server-side sound and webhook delivery. The `desktop` master and event switches also control browser notifications, but not in-page toasts. `CRUISE_DISABLE_NOTIFICATIONS=1` disables desktop, sound, and webhook delivery, but does not suppress WebUI browser notifications. Other values do not activate this global switch.

`model` and `plan_model` in either format provide optional global model defaults. Each accepts a string or an ordered array: the first entry is primary, the rest are SDK fallbacks. Both formats use the same field names and aliases. For example, `config.yaml`:

```yaml
runAllParallelism: 3
model:
  - openai/gpt-6-luna:max
  - openai/gpt-6.1-sol:xhigh
plan_model:
  - openai/gpt-6.1-sol:xhigh
  - openai/gpt-6-luna:max
```

Workflow and `default.yaml` model settings override these defaults; environment overrides still win. If no layer sets `plan_model`, planning uses the effective `model`. SDK arrays use the existing [fallback behavior](#sdk-mode); command mode uses only the primary. `planModel` is accepted as an alias for `plan_model`. Missing parallelism defaults to `1`, so model-only settings are valid. Updating parallelism through the CLI, WebUI, or TUI preserves both model settings.

The same application settings file can organize interactive workflow selectors:

```yaml
workflow_categories:
  Development:
    workflows:
      - builtin:tdd
      - refactor: Refactor an existing codebase
    Backend:
      workflows: [api]
show_others_category: true
others_category_name: Others
```

`workflow_categories` is an ordered tree of category maps. Each category may contain a `workflows` list and nested categories. Entries are names or single-key maps whose value overrides the selector description. `show_others_category` defaults to `true`, and `others_category_name` defaults to `Others`. These settings belong in application `config.yaml` or `config.json`, not workflow files or `default.yaml`. See [config file resolution](#config-file-resolution) for matching and fallback behavior.

Both workflow and application settings support SDK-qualified entries in `model` and `plan_model` arrays:

```yaml
model:
  - openai/gpt-6.1-sol:medium
  - sdk: claude
    model: claude-sonnet-5-5:medium
  - openrouter/z-ai/glm-5.3-flash:high
```

The equivalent JSON is `{"model":["openai/gpt-6.1-sol:medium",{"sdk":"claude","model":"claude-sonnet-5-5:medium"},"openrouter/z-ai/glm-5.3-flash:high"]}`. Strings inherit the workflow's effective SDK, defaulting to `jcode` when neither `sdk` nor `command` is set. Each object overrides only its own entry, so the final string above returns to jcode. Provider prefixes such as `openai/` and `openrouter/` are model references, not SDK names. Existing `opencode` SDK selection is also supported.

Objects require a nonblank `model` and at least one of `sdk` or nonempty `disabled_during`, and are allowed only inside arrays. Unknown keys and explicit null SDKs are rejected. An omitted `sdk` inherits the workflow SDK, not the previous entry's SDK. Objects cannot be combined with `command`, even in a fallback position. Legacy string-only command arrays still use their first model. Environment model overrides replace the entire list. Explicit retry chains remain string lists and take precedence over implicit array tails. Only the owning SDK receives the caller's saved resume ID. Another SDK starts without it, and returning to the original owner may reuse that original ID and its retained storage. Newly emitted IDs from failed attempts are not forwarded to subsequent attempts. Capability checks cover every applicable candidate: computer use requires jcode, remote HTTP/SSE MCP cannot use jcode, and OpenCode does not support legacy SSE.

#### Candidate unavailable windows (`disabled_during`)

Declare unavailable times per candidate in either `model` or `plan_model` arrays. This works in workflows, layered `default.yaml`, and application JSON/YAML settings (including the `planModel` alias). These are user-maintained exclusions, not price-aware routing or provider presets. For example, a user can choose these UTC windows for a DeepSeek candidate:

```yaml
sdk: opencode
model:
  - model: opencode-go/deepseek-v4.1-flash
    disabled_during: ["mon-fri 01:00-04:00", "mon-fri 06:00-10:00"]
  - opencode-go/kimi-k2.6
```

The same windows expressed with a fixed `+09:00` offset are:

```yaml
sdk: opencode
model:
  - model: opencode-go/deepseek-v4.1-flash
    disabled_during: ["mon-fri 10:00-13:00 +09:00", "mon-fri 15:00-19:00 +09:00"]
  - opencode-go/kimi-k2.6
```

Window grammar is `[days ]HH:MM-HH:MM[ offset]`. Days are case-insensitive `mon` through `sun`, comma lists (`sat,sun`), or inclusive, wrapping ranges (`mon,wed-fri`, `fri-mon`). Omitted days mean every day. Times use two-digit hours and minutes: starts are `00:00` through `23:59`, ends are `00:01` through `24:00`. Equal endpoints are invalid; use `00:00-24:00` for all day. Intervals include their start and exclude their end. When the end is earlier than the start, the window crosses midnight and weekdays identify the **start day**: `fri 22:00-02:00` includes Saturday at 01:00, not Sunday at 01:00.

Offsets default to UTC and accept `UTC`, `Z`, or fixed `+HH:MM` / `-HH:MM` values up to an absolute `14:00`. IANA names, daylight-saving transitions, and local system time zones are unsupported. Empty lists, blank windows, and malformed windows fail validation.

Cruise checks windows at each prompt start (including planning, titles, generated workflows, and `if.fail.retry` reruns) and when switching candidates during retries. It never interrupts an already-streaming turn. Any matching window excludes the candidate independently of retry policy or retry budget, even for a one-entry array. Skipping a window consumes no retry budget and records no cooldown. Existing cooldowns still independently exclude candidates. If every candidate is disabled or cooling down, the prompt fails without starting a backend.

Exclusions use SDK plus base-model identity, so effort/variant changes and matching string entries in explicit `retry.fallback_chains` cannot bypass a declared window. Capability validation still covers disabled candidates. A step `model` remains a string: an override inherits the workflow list and its windows only when its resolved SDK and base model both match the primary candidate. Changing only effort preserves inheritance when the SDK also matches. Other overrides do not inherit the workflow list. `subagent_model`, standalone model objects, step-level window fields, and windows on retry-chain entries are unsupported. Environment model overrides replace the whole list, including its windows.

#### `cruise clean`

```
cruise clean
```

Checks each Completed session's PR status via `gh pr view` (or `glab mr view` on GitLab). Sessions whose PR is closed or merged are deleted along with their worktrees (and any leftover `--repo` clone). Terminal exec/current-branch sessions that cannot have a PR, including legacy exec remnants, are deleted without a GitHub status check. Planned current-branch sessions whose `plan.md` is missing or blank and that never started are deleted too. Suspended sessions and other planned sessions remain available for resumption.

> **Note:** A session may lack a PR URL if `gh pr create` failed or was not reached. PR-backed sessions without a PR URL are retained; inspect the session logs or re-run PR creation manually with `gh pr create`.

#### `cruise usage`

`cruise usage [--by session,step,purpose,backend,model] [--session ID] [--format text|json|csv]` summarizes local attempt records. The default grouping is `step,backend,model` and the default format is text. Missing provider values are unknown, not zero; totals include input and output only. Command backends are recorded as unavailable. Costs are provider-reported, not estimates. OpenCode auxiliary title and compaction usage is excluded, so usage may be undercounted. Command backend internal retries are recorded as one call. `cruise ssh` records usage on the remote host, not locally.

## Session Management

Cruise stores session data in `$XDG_DATA_HOME/cruise/sessions/` (default: `~/.local/share/cruise/sessions/`). Each `state.json` has one tagged `config` reference:

```json
{"config":{"kind":"file","path":"/absolute/path/cruise.yaml"}}
```

The other supported kinds are `builtin_snapshot` (with an optional `name` of the built-in catalog entry, e.g. `{"kind":"builtin_snapshot","name":"simple"}`; older sessions without `name` still load), `repo_snapshot` (with a
clone-relative `relative_path`), and `inline_snapshot`. A `file` reference is
live: edits to the file are picked up on the next load or execution reload.
Sessions created with `--skip-planning` (input-as-plan) store `"input": ""` and `"input_as_plan": true`; `plan.md` holds the task text.
Snapshot references read only the session-owned `sessions/<session-id>/config.yaml`
and never rediscover or silently fall back to another config. Built-in, inline,
and temporary-clone workflows are serialized there as resolved YAML so they
remain runnable after their source or clone disappears. Older `state.json`
files containing `config_source` and `config_path` are not migrated or loaded;
create a new session when using this format.

### Runtime File Layout

Cruise follows the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/) and splits its runtime files across three directories:

| Kind | Path |
|------|------|
| User workflow YAML configs (`workflows/*.yaml` / `*.yml`) | `$XDG_CONFIG_HOME/cruise/workflows/` (default: `~/.config/cruise/workflows/`) |
| User workflow defaults | `$XDG_CONFIG_HOME/cruise/default.yaml` (default: `~/.config/cruise/default.yaml`) |
| Project workflow defaults | `./.cruise/default.yaml` |
| Application settings (`config.json` or `config.yaml`) | `$XDG_CONFIG_HOME/cruise/` (default: `~/.config/cruise/`) |
| Sessions, worktrees, and temporary `--repo` clones | `$XDG_DATA_HOME/cruise/` (default: `~/.local/share/cruise/`) |
| State files (`history.json`, `new_session_draft.json`, per-installation `installations/<installation-id>/update-transaction.json`) | `$XDG_STATE_HOME/cruise/` (default: `~/.local/state/cruise/`) |
| WebUI instance registry, lock, and daemon logs | `$XDG_STATE_HOME/cruise/webui/` (`instance.json`, `instance.lock`, `webui.log`, and optionally `webui.log.1`) |
| Usage records | `$XDG_STATE_HOME/cruise/usage.ndjson` (default: `~/.local/state/cruise/usage.ndjson`) |

On Windows, when the `XDG_*` variables are unset the defaults are `%APPDATA%\cruise` (config), `%LOCALAPPDATA%\cruise\data` (data), and `%LOCALAPPDATA%\cruise\state` (state). If those variables are missing, the `~/` defaults above apply, and `~\` is expanded like `~/`.

> **Migrating from `~/.cruise/`?** Earlier versions stored everything under `~/.cruise/`. Move `*.yaml`/`*.yml` into `~/.config/cruise/workflows/`, `config.json` into `~/.config/cruise/`, `sessions/` and `worktrees/` into `~/.local/share/cruise/`, and `history.json`/`new_session_draft.json` into `~/.local/state/cruise/`. Use `git worktree move` (or `git worktree repair`) when relocating worktree directories.
>
> **Workflow configs previously in `~/.config/cruise/*.yaml` / `*.yml`?** Automatic user-config discovery now looks for workflow YAMLs only in the `workflows/` subdirectory. Move them to `~/.config/cruise/workflows/`; cruise prints a warning if it finds legacy workflow YAML files left directly in `~/.config/cruise/`. Application settings `config.yaml` and workflow defaults `default.yaml` stay in that directory and do not trigger the warning.

### Session Lifecycle

1. **`cruise plan "task"`** -- Runs the built-in plan step in an isolated planning worktree to generate an implementation plan, then presents an approve-plan menu when stdin is an interactive TTY; with non-TTY stdin, the plan is auto-approved instead.
2. **`cruise --plan "task"`** -- Creates the session immediately and generates the plan in the background. Review it later from `cruise list`.
3. **`cruise draft "task"`** -- Records the task as a `Draft` session without running the plan step. Use **Generate Plan** from `cruise list` to start planning when you're ready.
4. **Approve-plan menu** -- Choose one of:
   - **Approve** -- Mark the session as ready to run.
   - **Fix** -- Provide feedback; the plan step reruns with your input.
   - **Ask** -- Ask a question; the answer is captured but not displayed; the menu reappears.
   - **Verify** -- Generate Quint and Alloy specifications, run their checks, and display the report and its interpretation without changing the saved plan. Use **Fix** to revise the plan.
   - **Execute now** -- Skip approval and run immediately.
   - **Publish as Issue** -- Publish `plan.md` as a GitHub issue (or GitLab issue) and delete the local session.

   **Verify Spec** in the TUI and **Verify** in the WebUI are also available for a usable plan awaiting approval. It is a menu action, not an `/verify` command in Ask. It requires `quint` on PATH (0.33.0 recommended) and Java 17 or later for Alloy. Alloy uses `CRUISE_ALLOY_JAR` if supplied; otherwise Cruise downloads and checksum-verifies Alloy 6.2.0 into its data directory. Reports and logs are retained under the session's `formal/<run-id>/` directory until the session is deleted, not in the repository. Each Quint parse, typecheck and simulation has a 60-second limit. Model checks use `formal_verify.model_check_timeout_seconds` (default 900 seconds). Alloy commands run sequentially, so N commands can take up to N times that limit, plus setup and other checks. A passed bounded check is not a proof beyond its modeled scope.

   After choosing **Approve** or **Execute now**, declared workflow flags are selected first when no `-f/--flag` was supplied. Then a **step skip selector** lists only steps enabled by those values (grouped steps appear as a parent with children); no selector is shown when there are no candidates. Toggle any steps you want to skip for this run. Only selections from the visible candidates are persisted per config file in `$XDG_STATE_HOME/cruise/history.json` and pre-selected for the next session using the same config. Cancelling either selector returns to the approve-plan menu without saving partial flag changes, approving, or executing the session. Execute now forwards the approval-time flag values as explicit assignments, suppressing another flag prompt at execution start. After Approve, a later `cruise run` still offers the fresh-execution flag selector and does not ask for skips again (see [Workflow flags](#cruise-run)).

5. **`cruise run`** -- Without an ID, picks a pending session (Planned, Running, Failed, or Suspended; prompts when several qualify), reuses (or creates) the git worktree under `$XDG_DATA_HOME/cruise/worktrees/<session-id>/`, and executes the workflow. By default it creates a PR when the run leaves branch commits, then runs configured `after-pr` steps. In built-in workflows, `-f after_pr` enables that prelude without merging; `-f merge` enables the same prelude once plus the final native synchronous squash merge. If a successful run has no new branch commits, PR creation is skipped without error (existing branch commits are still eligible), and opted-in after-PR actions are skipped with a reason. Workflow-level `create_pr: false` skips automatic commit, push, PR creation, `after-pr`, and PR-based cleanup, while preserving the normal session, artifacts, and worktree; opted-in actions are skipped with a reason and merge never overrides the no-PR setting. Explicit `if.no-file-changes` failure/retry behavior is unchanged. Publication failures enter **PR Failed** for both repo and local worktree sessions; use **Create PR** to retry publication without rerunning the workflow.

GitHub built-in `wait-ci` steps pin the local `HEAD`. Readiness requires the PR head and CI rollup commit to match that expected commit, an existing `statusCheckRollup.state` of `SUCCESS`, and every GitHub Actions check suite on the commit with a `workflowRun` to be `COMPLETED`. A completed suite with a failing conclusion fails CI; a suite without `workflowRun` (for example, from a third-party app) does not block. Readiness must hold for two consecutive polls; any non-ready poll resets the streak. A PR-head mismatch stays pending for a 90-second grace period to allow API propagation; if it catches up in time, the wait can succeed, but a persistent mismatch after the grace or overall deadline fails with the existing `head_mismatch` diagnostic. The built-in polls every 5 seconds for up to 29 minutes (1740 seconds) within a 30-minute step timeout; unregistered or pending checks are not success. Existing snapshot sessions retain their saved wait command and timeout until explicitly updated; changing a built-in does not rewrite them.

Native GitHub squash merge applies the same CI readiness gate: the PR head and an existing rollup commit must match the expected local `HEAD`, the rollup state must be `SUCCESS`, and every GitHub Actions check suite on that commit with a `workflowRun` must be `COMPLETED`; a completed suite with a failing conclusion fails CI, while suites without `workflowRun` do not block. Readiness must hold for two consecutive polls; any non-ready poll resets the streak. A PR-head mismatch remains pending for up to 90 seconds for API propagation; a transient mismatch that catches up can succeed, while one persisting past the grace or overall readiness deadline fails. The built-in `wait-ci` retains its existing `head_mismatch` diagnostic. The merge also waits for GitHub's `UNKNOWN` mergeability to resolve, polling every 2 seconds for up to 10 minutes (a shorter step timeout still applies). This runtime check protects existing saved workflow snapshots without rewriting them. Conflicts, malformed responses, and unmet policy or review requirements still reject the merge; the final merge request remains single-shot and SHA-pinned.

A refused final merge records only a fixed reason and available HTTP status and command exit code, after confirming the PR remains open at the expected head. Confirmed merges and other confirmation errors take precedence over refusal diagnostics.

`MERGEABLE/UNSTABLE` is transient only while the pinned commit's CI is pending or not yet registered/current; it is never merge-ready. A current-head CI failure still rejects immediately, and green CI with a remaining `UNSTABLE` state is also rejected.

PR-backed sessions remain in `$XDG_DATA_HOME/cruise/sessions/` until their PR is closed or merged, after which `cruise clean` can remove them. A normal session configured with `create_pr: false` retains its session, artifacts, and worktree without waiting for a PR.

Eligible local worktree sessions without a PR can instead stage a squash merge while retaining the session, or explicitly merge and clean up the worktree, branch, and session.

### Model and Fallback Notices

When a visible status or info channel is available, each prompt execution
reports the Cruise-selected model there; every notice is also written to
the session's `run.log`:

```text
Model: provider/model:free:xhigh
```

When retry policy switches models, Cruise reports the transition and then the
replacement model:

```text
Warning: Fallback: provider/primary -> provider/fallback (503, attempt 1/5)
  Error: API Error: 503 service temporarily unavailable
Model: provider/fallback
```

The parenthesized detail is the reason the model was left behind -- the HTTP
status the provider returned (`503`, `429`, `400`, `404`, ...) when the failure
text names one, otherwise the failure class (`rate limit`, `server error`,
`client error`, `missing model`, `network error`, or `unusable model` for a
reference the backend refused before sending anything).

The original backend error message follows as `Error:`, with embedded newlines
preserved, even when the replacement model succeeds.

The complete model reference is retained, including its provider, path, and
effort suffixes. Claude and external command backends may still run without a
Cruise model and identify the backend in notices. A jcode prompt with no
effective Cruise model stops with a setup error instead of delegating selection
to jcode, so it does not emit `Model: default model (jcode)`. Notices are sent
to foreground CLI stderr when status output is enabled, the TUI's existing
info log, and the session's `run.log`. Detached `cruise --plan` workers and
the CLI's interactive `cruise run --all` dashboard suppress terminal notices,
so those notices must be read from the session's `run.log`. They are not
mixed into stdout or plan/Ask response content. The existing session
lifecycle and log retention rules remain unchanged. In particular, transient
`cruise exec` sessions are still removed after completion.

> **`cruise exec`** is a separate path with a transient lifecycle: it executes in the current directory without planning, worktree creation, or PR creation, and removes its session after terminal completion. Paused or interrupted exec sessions remain resumable by ID. `force_exec: true` enables the same path for direct plan entry points; use `--no-force-exec` to opt out once. See [`cruise exec`](#cruise-exec).

### `cruise list` Actions

The interactive session list shows a menu of actions depending on the session's phase:

| Phase | Available Actions |
|-------|-------------------|
| **Draft** | Generate Plan, Delete, Back |
| **AwaitingInput** | Generate Plan, Delete, Back |
| **AwaitingApproval** | Approve, Publish as Issue, Edit Settings, Delete, Back |
| **Planned** | Run, Publish as Issue, Edit Settings, Replan, Delete, Back |
| **Running** | Resume, Tell*, Reset to Planned, Delete, Back |
| **Suspended** | Resume, Edit Settings, Reset to Planned, Delete, Back |
| **Failed** | Run, Edit Settings, Reset to Planned, Delete, Back |
| **PR Failed** | Create PR, Open PR*, Reset to Planned, Delete, Back |
| **Completed** | Open PR*, Merge PR*, Stage local merge**, Merge locally & cleanup**, Reset to Planned, Delete, Back |

`cruise list` does not offer Verify, matching its existing omission of Ask.

\* Open PR and Merge PR are shown only when the session has a PR URL.
Tell is shown only while the running session's runner is alive.

\** Local merge actions require an eligible local worktree session without a PR and a recorded target branch. Stage is unavailable once a merge intent or commit has been recorded; legacy sessions without a recorded target are ineligible.

**View Diff** is additionally available in every phase when the session has a worktree branch. The branch can still have been deleted, in which case viewing the diff reports an error.

`cruise list` may also show `Planning` while `--plan` is still running, or `Plan Failed` when background planning wrote a durable `plan_error`. These display states are backed by `AwaitingApproval`: `Edit Settings`, `Delete`, and `Back` are always available, while `Approve` and `Publish as Issue` appear only when a non-empty `plan.md` is available and there is no `plan_error`.

- **Generate Plan** -- Generate the plan for a `Draft` or `AwaitingInput` session (transitions it to `AwaitingApproval`).
- **Approve** -- Approve the plan and transition the session to the Planned phase.
- **Publish as Issue** -- Publish `plan.md`, unchanged, as a GitHub issue (or GitLab issue) in the resolved repo, then delete the local session. Prompts whether to also post a follow-up `@cruise run` comment so the `@cruise` GitHub Action picks it up (default: off for `AwaitingApproval`, on for `Planned`). If the issue is created but that comment fails to post, the session is kept so you can retry (the existing issue is reused, not duplicated) or comment manually (the `@cruise run` comment prompt is skipped on GitLab).
- **Run / Resume** -- Execute (or continue) the session.
- **Tell** -- Submit an instruction to the running session's next agent prompt. TUI and WebUI also offer Tell for a live run.
- **Create PR** -- Retry publication for a **PR Failed** session without rerunning the workflow. Cruise commits remaining changes, pushes, and reuses an existing PR for the branch or creates one. Check the displayed error and forge authentication first (for GitHub HTTPS credentials, `gh auth setup-git` can configure Git). Publication failure or cancellation keeps the session in **PR Failed** with its worktree and any existing PR URL. A fatal after-PR error leaves it **Failed**. Success completes the session and runs configured after-PR steps only when a PR was created or reused; a no-commits skip completes without running them. Reset to Planned instead restarts the workflow, while Delete removes the session and its workspace.
- **View Diff** -- Review the session branch's changes and return to the menu. See [`cruise diff`](#cruise-diff) for comparison and cleanup limitations.
- **Replan** -- Provide feedback to re-generate the plan; the session stays in the Planned phase.
- **Open PR** -- Open the session's pull request in the browser via `gh pr view --web`.
- **Merge PR** -- Human-initiated merge of the session's pull request (also available in the TUI and WebUI). Cruise first shows the PR's state, mergeability, review decision, and each CI check. You then choose `Squash` (the default), `Merge`, or `Rebase` and confirm; the confirmation names the chosen method. Cruise runs `gh pr merge <url> --squash|--merge|--rebase` and never adds `--auto`, `--admin`, or `--delete-branch`. If GitHub reports the PR as Closed or Merged afterwards, only that session and its worktree (and any `--repo` clone) are cleaned up, as `cruise clean` would. If the PR is still Open (for example in a merge queue) the session is kept and `cruise clean` can remove it later. A PR that is already Closed or Merged is not merged again: run `cruise clean`. This remains the human-confirmed, GitHub-only route; built-in `-f merge` is a separate native synchronous squash path. See [after-pr](skills/cruise-config/references/after-pr.md) for its checks and constraints.
- **Stage local merge** -- Squash the session branch into the base checkout's index without committing or removing the session. Uncommitted worktree changes are committed to the session branch first.
- **Merge locally & cleanup** -- Squash into one base commit, then remove the worktree, branch, and session. Cleanup failure retains the session for retry without repeating the squash. An initial squash requires the recorded target checked out locally, a clean tracked base and no behind upstream; Cruise does not fetch or pull. A cleanup retry checks that the recorded source commit is available, the branch has not changed, and any retained worktree is the expected checkout with no tracked or untracked changes. Conflicts are reported, rollback is attempted, and rollback failure is reported; AI does not resolve them. These actions exclude `--repo` and current-branch sessions and do not depend on `cleanup_after_pr` or the `merge` flag.
- **Reset to Planned** -- Reset the session back to the Planned phase, clearing the current step and allowing it to be re-run from the beginning.
- **Delete** -- Permanently remove the session.
- **Back** -- Return to the session list.

## Config File Resolution

cruise resolves the workflow config as follows:

1. **`-c/--config` flag** -- highest priority. The specified file must exist or cruise exits with an error. No prompt is shown. `-c __builtin__` selects the built-in `tdd` workflow, equivalent to `-c builtin:tdd`; `-c builtin:<name>` selects a named catalog workflow. The old selectors `builtin:default`, `builtin:simple`, and `builtin:review` are unsupported. The fixed catalog is:

   | Name | Description |
   |------|-------------|
   | `tdd` | Test-first implementation followed by a parallel analyze → consolidate → apply verification group and the PR workflow |
   | `refactor` | Diff-first code, test, and structure improvements with verification |
   | `add-tests` | Audit behavior and user-flow coverage, add meaningful tests and CI, then independently verify and repair gaps |
   | `add-formal-specs` | Add executable Quint, Alloy, and Gherkin specs with CI; fix real spec/source defects |
   | `research` | Bounded, read-only investigation with source-backed evidence, independent review, and a cited report |
   | `security` | Scoped security inventory and parallel audits, triage, safe fixes for confirmed defects, and independent verification |
   | `pr-review-fix` | Address eligible review threads on an existing GitHub PR head branch |

   `refactor`, `add-tests`, `add-formal-specs`, and `security` set `force_exec: true` by default: local planning runs directly in the current directory without an outer plan/session, worktree, or automatic PR. Pass `--no-force-exec` to use the normal plan/session path instead.

   `cruise workflow list` prints the catalog. To customize a built-in, use `cruise workflow eject <name> [--to user|project]`, for example `cruise workflow eject tdd --to project`. Ejection accepts catalog names only and never overwrites an existing file.

   `refactor` starts from the repository's existing staged, unstaged, and untracked changes plus task-branch changes against a trustworthy base/merge-base when available. It limits changes to that diff and necessary callers/dependencies; with no diff, it scopes to the whole repository. Existing work is preserved and unrelated edits are not reset, stashed, or reverted.

   `add-tests` inventories production behaviors and real user journeys, maps stable coverage/gap IDs to source boundaries and test assertions, then adds or completes useful tests and applicable CI. `add-formal-specs` adds executable Quint and Alloy models plus Gherkin scenarios with production-connected step definitions, runs them in CI, and fixes counterexamples or real source/spec defects without weakening requirements.

   Step order alone does not insert earlier reports into a prompt. `add-formal-specs` saves the audit and model outputs as session artifacts (`formal-specs/audit.md` and `formal-specs/model.md`), and its later prompts explicitly embed the required report contents with `{file:...}`.

   `add-tests` passes the original request (`{input}`) and saved plan into implementation via `{file:test-ci-gap-plan.md}`; verification independently consumes `{input}`, `{file:test-ci-gap-plan.md}`, and `{file:test-ci-implementation.md}` to recheck every planned/addressed ID against actual tests, assertions, and commands. It writes `test-ci-gap-plan.md`, `test-ci-implementation.md` (per-ID applied/already-covered/rejected/blocked dispositions with evidence), and `test-ci-verification.md`. The coverage ledger marks behavior `Covered`, `Partial`, or `Missing`, records exclusions and unreviewed areas with reasons, and prioritizes actionable gaps without imposing numerical coverage quotas.
   The built-in `tdd`, `refactor`, and `add-tests` workflows follow repository instructions and first reuse appropriate established tooling for every in-scope real product target—including Windows, Linux, and native targets—preserving its versions, commands, setup, and CI. The recommendations below are only for an appropriate capability that is absent; they are not allowlists, migrations, mandatory installs, runtime dependencies, or new workflow/config keys. Existing tools such as Maestro remain valid. Use stack-supported compatible versions and do not force a new platform test matrix.

   Recommendations for backend and command-line capabilities:

   - **Disposable external dependencies:** [Testcontainers](https://testcontainers.com/) (and its [Rust library](https://rust.testcontainers.org/)) is for provisioning disposable real external databases or dependency services where needed, using a stack-supported library. Pin compatible library and image versions; wait for readiness; use dynamically mapped connection details, real application migrations, isolated seed data, and cleanup limited to test-owned resources. It is not a database assertion framework, and is unnecessary for an in-process/embedded database or a project with no database. If a required Docker-compatible runtime is unavailable, report blocked—not a host production database, mocks, or a pretend pass.
   - **CLI and TUI:** [Bats-core](https://github.com/bats-core/bats-core) can store Bash tests that run the real built CLI executable and meaningfully assert its exit status, stdout, stderr, and side effects. Bats `run` returns success after capturing a command, so explicitly assert the target command's status; `run` alone is not a pass. For an actual interactive TUI, use Bats to orchestrate a real PTY-capable interaction harness (reuse one, such as Expect where appropriate), send keys, assert terminal-screen/state transitions, and inspect rendered terminal/image evidence when applicable. A pipe, noninteractive stdout, or `--help` smoke test is not TUI E2E. If the required PTY/runtime is unavailable, report blocked. Respect supported Bash and host platforms; do not force unsupported native Windows Bash.
   - **Web HTTP APIs:** [Hurl](https://hurl.dev/) is for saved `.hurl` tests run with `hurl --test` against real local application HTTP endpoints, with substantive response and relevant state/effect assertions. Reuse existing setup, isolation, and local test authentication; provision a database dependency with Testcontainers when appropriate. Static schema/parser checks and mocked HTTP are not real endpoint tests; do not use live credentials or external production services.
   - **Backend service contracts:** [Pact](https://docs.pact.io/) ([when Pact is useful](https://docs.pact.io/getting_started/what_is_pact_good_for)) is for HTTP or message contracts only when identified real consumer/provider participants are controllable and supported. Generate contracts through actual consumer client code, verify them against the real provider implementation with explicit provider states, and record both sides. Consumer mocks or pact-file generation alone do not complete verification. Pact is not general backend functional, database, or integration E2E and does not replace Hurl or real-path checks; controlled protocol contract doubles are valid contracts, not mock-only E2E. Do not require a Broker/PactFlow, cloud infrastructure, or secrets.

   Classify the actual product and framework from manifests and source before choosing UI tooling. For browser targets, recommend Playwright CLI (`playwright-cli`) for visual checks and Playwright Test (`*.spec.ts`, `npx playwright test`) for saved E2E only when the corresponding capability is missing. For SwiftUI iOS/macOS, recommend Xcode MCP for actual app interaction and screenshot capture only when suitable visual tooling is missing (Preview alone is insufficient), and XCTest/XCUITest (`xcodebuild test`) only when suitable saved UI E2E tooling is missing; use App Intents Testing additionally only for actual intent integration. For Android, recommend mobile-mcp for visual checks and Espresso for Views, Compose UI Test for Compose, and UI Automator for system/cross-app flows only when the corresponding existing capability is missing.

   For React Native/Expo iOS+Android, recommend agent-device CLI or connected MCP for visual checks and Detox + Jest (`detox build`/`detox test`) for saved E2E only when corresponding suitable tooling is missing. Expo needs its own Detox-integrated native build, pinned Expo/RN/New Architecture/Detox compatibility, and its prebuild/config-plugin workflow (not Expo Go or manual-only generated-file edits); use iOS Simulator and supported Android runtime as appropriate, not blanket physical iOS coverage. Unsupported Detox compatibility is blocked, not a runner switch. For Electron, recommend Playwright Electron API for real-app interaction and Playwright Test with that API for saved E2E only when corresponding suitable tooling is missing; verify its experimental API/version compatibility (renderer-in-browser is not a native-app pass, and OS-dialog stubs are not real OS-dialog coverage).

   Classify Tauri as a native app (WebView plus Rust/IPC), not an ordinary browser or Electron frontend. On a compatible host, when suitable visual tooling is missing, recommend [tauri-plugin-mcp](https://github.com/P3GLEG/tauri-plugin-mcp) for real-app input and screenshots, followed by actual image inspection. When suitable persistent E2E tooling is missing, recommend [tauri-webdriver](https://github.com/Choochmeque/tauri-webdriver) with its required companion [tauri-plugin-webdriver](https://github.com/Choochmeque/tauri-plugin-webdriver) and a real WebDriver client/test runner (reuse an existing one, otherwise use a stack-compatible client such as WebdriverIO or Selenium). Pin compatible versions or Git revisions and confirm host, Tauri, WebView, and client compatibility. Save substantive assertions and an independent command exercising relevant real frontend-to-Rust IPC/native behavior. Both plugins are dev/test only: exclude automation endpoints from production builds, use local/isolated test sessions, and do not enable MCP globally. Account for mocks, synthetic/untrusted events, and dialog stubs; the MCP plugin stubs JavaScript `alert`/`confirm`/`prompt` by default, which is not evidence of real native-dialog behavior. If required host/runtime/plugin capabilities are missing, report blocked rather than substituting a browser-only pass.

   For native targets or capabilities without an applicable listed framework recommendation, reuse appropriate existing tools only; do not introduce new tools. If an established required tool exists but its compatible host, runtime, or server is unavailable, report the precise blocker. If only one of visual interaction/screenshot or saved-E2E capability exists, do reachable work and report the missing capability honestly; do not claim both deliverables. Each applicable selected UI flow requires both real-app interaction with saved screenshots of meaningful states/failures whose pixels are inspected, and deterministic assertion-bearing E2E source stored in the repository with reset/isolated state, exact repeatable build/run commands, and an execution independent of prior exploration sessions and LLM decisions. Keep a reusable shared evidence record of exact commands/results and inspected screenshots rather than rerunning just to duplicate evidence. One actual E2E run plus inspected screenshots suffices; no duplicate run or pixel-diff/golden test is required, and visual-only evidence does not close E2E coverage. If there is no relevant product UI target, report that limitation honestly. Browser/Electron checks on supported hosts and CI remain in scope, and Linux web CI must not be disabled.

   Inspect/load relevant available skills through the executing agent's provided mechanism and use connected MCP servers when appropriate. Skills are optional, missing skills alone are not blockers; do not add skills, guess APIs, edit global MCP/config entries, or force computer-use settings. If a required capability lacks an appropriate existing tool, use an applicable recommendation above; if the needed tool/server/runtime is unavailable, report the precise blocker rather than claiming success.

   `research` runs four bounded read-only phases: plan → investigate → independent review → synthesize. It retains `research/plan.md`, `research/evidence.md`, `research/review.md`, and `research/report.md`; every phase carries `{input}`, and later phases explicitly read earlier reports with `{file:...}`. The final report answers the request with sourced facts, separate inferences, contradictions, uncertainty, review dispositions, and exact blockers. External claims cite a URL and exact locator plus available source version/date and actual retrieval date; repository claims cite paths and line ranges. It does not invent sources or checks. The built-in sets `create_pr: false`, so this durable session makes no workspace/code changes and skips automatic commit, push, PR, `after-pr`, and PR cleanup while retaining session artifacts and the worktree; `merge` never overrides this read-only/no-PR policy, and opted-in unavailable actions are skipped with a reason. Use `cruise plan --skip-planning -c builtin:research "question"`, approve the plan if prompted, then `cruise run <session-id>`—not transient `exec` or `force_exec`.

   `security` inventories scope and coverage, runs parallel code/dependency/delivery audits, triages findings, repairs confirmed safe in-scope defects, and independently verifies each finding. It retains `security/scope.md`, `security/code-findings.md`, `security/dependency-findings.md`, `security/delivery-findings.md`, `security/worklist.md`, `security/repairs.md`, and `security/verification.md`. Each phase receives `{input}`; later phases explicitly consume reports with `{file:...}`, and triage receives the joined audit results as `{prev.output}`. Findings retain stable IDs and evidence for exploitability and accepted/rejected/blocked dispositions, false positives, fixes, regression checks, and independent closure; reports identify exact unrun checks, excluded/unreviewed scope, and limits honestly without claiming whole-system security.

   The research workflow and security inventory/audit/triage phases require SDK permissions for read-only work. When neither `command:` nor `sdk:` remains set after workflow and applicable user/project defaults are resolved, Cruise uses its default `sdk: jcode`; a `command:` backend cannot be confined to read-only mode and is rejected for restricted-permission steps.

   Interactive selectors (CLI, TUI, WebUI) organize built-ins and other candidates into categories. Non-interactive resolution with no config file uses `tdd`.

   `create_pr` is a workflow-only boolean that defaults to `true`. Setting it to `false` skips Cruise's automatic commit, push, PR creation, `after-pr`, and PR-based cleanup, while retaining a normal session, its artifacts, and its worktree; local no-PR worktrees do not need `gh`/`glab`, but `--repo` still needs the forge CLI for repository access. With PR creation enabled, a successful run with no new branch commits skips PR creation without error, while existing branch commits remain eligible; opted-in after-PR actions are skipped with a reason if no PR is created. `merge` never overrides `create_pr: false` or a read-only workflow such as `research`; explicit `if.no-file-changes` failure/retry guards still apply. `create_pr` is rejected in user and project `default.yaml` files; it is not a CLI option.

2. **`CRUISE_CONFIG` environment variable** -- if set, used directly (error if the file does not exist). No prompt is shown.
3. Otherwise, cruise collects every candidate from the following locations and presents them as choices:
   - `./cruise.yaml` -> `./cruise.yml` -> `./.cruise.yaml` -> `./.cruise.yml` (current directory)
   - `./.cruise/*.yaml` / `*.yml` (current directory), sorted by filename; the top-level `./.cruise/default.yaml` is excluded
   - `$XDG_CONFIG_HOME/cruise/workflows/*.yaml` / `*.yml` and members of valid locked bundle directories (default: `~/.config/cruise/workflows/`), sorted by relative path

   When stdin and stdout are both TTYs, candidates and built-in workflows are presented in a categorized interactive selector. In non-interactive contexts (piped stdin, scripts) the highest-priority candidate is taken automatically without a prompt.
4. **No config file found** -- an interactive terminal offers built-in workflows in the selector; non-interactive resolution selects the embedded `tdd` workflow (`builtin/tdd.yaml` in the source tree) automatically. No config file is required.

Interactive selectors use the configured `workflow_categories` tree, or, when it is empty or absent, the defaults: Development (`tdd`, `refactor`), Quality (`add-tests`, `add-formal-specs`), Research (`research`), and Security (`security`). A nonempty user tree replaces those defaults rather than merging with them. Categories and workflow entries each retain their declaration order; CLI submenus display subcategories before workflow entries. A workflow can appear in multiple categories.

A bare entry matches a file's stem (filename without extension) and a built-in name. For example, `refactor` matches both `refactor.yaml` and the built-in, while `builtin:refactor` matches only the built-in. Matching files from different sources all appear. Missing names are ignored, and empty categories are omitted. An entry such as `{refactor: "Custom description"}` supplies the selector description and overrides the workflow's `description:` in WebUI. The CLI picker does not load file workflow descriptions. Conflicting descriptions for the same workflow, including bare and prefixed names for a built-in, are invalid. Picker settings are validated when opening a picker and do not prevent runtime model or parallelism defaults from loading.

Uncategorized local and `CRUISE_CONFIG` candidates remain available at the root (in WebUI, under `Base dir (...)`). Uncategorized installed packages appear in `Packages`, followed by uncategorized user-directory and built-in workflows in `Others` or the configured `others_category_name`. Setting `show_others_category: false` hides those uncategorized user and built-in choices, but not local, environment, or installed package candidates. WebUI also retains its *Auto* and built-in `tdd` choices and the current session workflow when it is otherwise filtered out. Categories affect interactive display only, not explicit selection or non-interactive resolution priority.

### Workflow defaults (`default.yaml`)

`$XDG_CONFIG_HOME/cruise/default.yaml` supplies user defaults. When `XDG_CONFIG_HOME` is unset, the path defaults to `~/.config/cruise/default.yaml` on macOS/Linux. On Windows, it is `%APPDATA%\cruise\default.yaml` when `APPDATA` is set; if `APPDATA` is missing, Cruise falls back to the home-directory path `~/.config/cruise/default.yaml`. `./.cruise/default.yaml` supplies project defaults. These files provide defaults for fields a workflow leaves unset. They are never selectable as workflows (`-c` and `CRUISE_CONFIG` pointing at them is an error). `default` is not a built-in workflow name, so `cruise workflow eject default` is rejected as an unknown built-in; `cruise workflow generate --name default` still refuses to write the reserved `./.cruise/default.yaml`. `<config_dir>/workflows/default.yaml` remains an ordinary workflow.

Precedence, highest first, where applicable: per-invocation CLI flags (including `-c` for workflow selection) > saved session cleanup override > configuration environment overrides > the workflow file > `./.cruise/default.yaml` > `$XDG_CONFIG_HOME/cruise/default.yaml` > application model defaults (`config.json` or `config.yaml`) > built-in defaults. Step-level model overrides still take precedence for ordinary prompts.

When an actual jcode prompt has no effective model, Cruise creates
`$XDG_CONFIG_HOME/cruise/default.yaml` (or the platform's normal user config
path) with `model: null` and stops before starting jcode. Edit that user-level
file to set a real `provider/model[:effort]` reference or bare model. Existing
files are never changed, and project `.cruise/default.yaml` is never generated.
You can instead set `model` in `config.json` or `config.yaml`, the workflow, or `CRUISE_MODEL` for a new session;
planning calls may use `plan_model` / `CRUISE_PLAN_MODEL`. The file is not
created for dry-runs, drafts, listings, command-only execution, or Claude.
After updating settings, retry with the command that initiated the request: use
`cruise run <SESSION>` for a failed run session, or re-run `cruise plan` or
`cruise exec` for those requests. File-backed sessions reload defaults, but saved
snapshot sessions do not reapply later defaults or environment changes; start a
fresh snapshot session to use updated settings.

- `env` and `mcp_servers` merge per key (higher layer wins).
- `command` and `sdk` form one pair: if a layer sets either, the lower layers' pair is ignored.
- Other fields (`model`, `plan_model`, `subagent_model`, `max_retries`, `interactive_planning`, `languages`, `cleanup_after_pr`, `force_exec`, `computer_use`, `permission`, `disallowed_tools`, `retry`, `formal_verify`) are taken from the highest layer that sets them. Restriction lists replace across layers; explicit workflow `disallowed_tools: []` clears inherited defaults.
- Worktree settings `base_branch` and `auto_fetch` are defaults-only keys, with project values overriding user values. `auto_fetch: true` requires a nonempty `base_branch` after merging.
- `description`, `steps`, `groups`, and `after-pr` are workflow-only and are a load error in `default.yaml`. Unknown keys are errors too.
- If defaults name a backend (`command` / `sdk`) of a different kind than the workflow's own, validation fails when they would fill an unset backend-specific `model`, `plan_model`, `subagent_model`, or `retry`, add a missing `mcp_servers` entry, or fill an unset `computer_use`, `permission`, or `disallowed_tools` field, regardless of its value. Set the field in the workflow or remove it from `default.yaml`. The check runs before `CRUISE_SDK` overrides, which are not considered.

`pre_steps:` defines command-only steps (`command`, `env`, `timeout`, `skip`, `when`) inserted at the start of the workflow's `steps` as `pre/<name>`. They do not run in planning or `after-pr`. User steps come first, then project steps; a project step with the same name replaces the user one in place.

```yaml
# ./.cruise/default.yaml
pre_steps:
  mise-trust:
    command: mise trust || true
  nix:
    command: nix develop --command true
```

Upgrade note: the built-in workflow no longer has a `mise-trust` step. Add the `mise-trust` pre step above to keep that behavior.

In GitHub Actions, `XDG_*` is isolated, so only the project `.cruise/default.yaml` takes effect.

## Config File Reference

### Basic Structure

`formal_verify.model_check_timeout_seconds` sets the per-command Quint model-check and Alloy timeout in seconds. It accepts an integer from 1 through 86400 and defaults to 900. It may be set in workflow YAML or layered `default.yaml`:

```yaml
formal_verify:
  model_check_timeout_seconds: 900
```

```yaml
command:                   # LLM invocation command (mutually exclusive with `sdk`)
  - claude
  - --model
  - "{model}"
  - -p

# sdk: jcode              # Use jcode-sdk (the fallback only when neither
                          # `command` nor `sdk` remains set after applicable
                          # `default.yaml` layers; see SDK Mode)
# sdk: claude             # Use claude-agent-sdk to drive the claude CLI

description: |             # one-line summary shown next to the filename in selectors (optional)
  Team-shared review-heavy flow with auto-PR.

model: sonnet             # default model for all prompt steps (optional)
# In SDK mode, arrays use the first model, then fall back in order.
# model: [sonnet, opus, gpt-5.5]
plan_model: opus          # model used for the built-in plan step (optional)
# subagent_model: haiku   # SDK-only: one subagent model, string or {sdk: claude, model: haiku}
languages:                # prompt languages (optional; defaults to English)
  pr: English             # language for auto-generated PR title/body
  plan: English           # language used by built-in planning prompts
# commit_message_template: "{title} [{session_id}]" # host-created commit message
# pr_body_template: "## Summary\n{body}"            # generated PR body wrapper
# force_exec: false       # execute direct plan entry points in place (use --no-force-exec to opt out)
# permission: full        # prompt permission: read-only | edit | full (default full)
# disallowed_tools: [web, subagent, "shell:npm publish"] # SDK prompt deny list
# computer_use: false     # jcode only, macOS: let prompts control the desktop (macos_computer_use)
# Deprecated compatibility fields: pr_language and plan_language

env:                      # environment variables applied to all steps (optional)
  API_KEY: sk-...
  PROJECT: myproject

# MCP servers are available to SDK runs only; omit this block for command mode.
# mcp_servers:
#   local_search:
#     command: npx
#     args: ["-y", "@example/search"]

flags:                    # workflow flags (optional; absent values default off, see Workflow flags)
  # run_review: {}

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
    # flag: run_review     # run only when the declared flag is on (optional)

after-pr:                # optional: steps that run automatically after PR creation
  step_name:
    # step configuration (same format as `steps`)
```

Unknown-key handling differs by level: individual step maps (in `steps`, `after-pr`, a group's `steps`, and parallel children) and the `retry` block reject unknown fields with a parse error, which is what turns a typo or a removed field such as `fail-if-no-file-changes` into an immediate config error. Unknown **top-level** keys are ignored silently, so a misspelled `plan_model` or `max_retries` at the top of the file is accepted and simply has no effect.

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

Instead of spawning an external CLI via `command`, prompt steps can be driven through an SDK backend by setting the top-level `sdk` field. `command` and `sdk` are mutually exclusive; the default `jcode` backend is used only when neither remains set in the resolved workflow after applicable `default.yaml` layers are applied. Three values are accepted: `jcode`, `claude`, and `opencode`.

#### Startup OAuth expiration checks

Before starting agents for `cruise plan`, `cruise run`, or `cruise exec`, and for session planning/execution in the TUI or WebUI, Cruise checks local OAuth credential files for the applicable SDK candidates only. Planning uses the effective `plan_model` (falling back to `model`); execution includes workflow models and step overrides, including `after-pr`, groups, and parallel children. Model arrays, SDK-qualified entries, and effective retry fallback chains are included, even if a fallback is not ultimately used. This is not a global check at TUI/WebUI startup. Command backends, dry-runs, `cruise workflow generate`, and `cruise workflow make` are outside this check.

- **Claude SDK:** reads `$CLAUDE_CONFIG_DIR/.credentials.json`, or `~/.claude/.credentials.json` when unset, using `claudeAiOauth.expiresAt`, `refreshToken`, and optional `refreshTokenExpiresAt`.
- **Jcode SDK:** checks only explicit providers `claude`, `claude-oauth`, and `claude-subprocess` in the source home's `auth.json`, and `openai` and `openai-oauth` in `openai-auth.json`. The source home is the process `JCODE_HOME`, or `~/.jcode` when unset. With multiple accounts, it stops only if every account is conclusively expired.
- **Skipped:** OpenCode, jcode bare models with no provider, API-key providers such as `anthropic-api` / `openai-api`, and all other providers. Missing/unreadable files, invalid JSON, or insufficient expiration data pass without blocking. For Claude, a non-empty process or workflow-level `env` value for `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`, or `CLAUDE_CODE_USE_FOUNDRY` also skips the file check.

Exception: SDK title generation during foreground CLI approval, including `cruise plan --skip-planning`, uses only the inherited process environment and process working directory. Its expiration preflight therefore ignores workflow-level `env`, including `CLAUDE_CONFIG_DIR` and alternative-authentication variables. A workflow-only API key does not bypass this title-generation check; conclusively expired inherited OAuth credentials stop approval.

Cruise stops only when the access token has expired and the refresh token is absent or its recorded expiration is at or before the current time. An expired access token with a refresh token and no known refresh expiration passes without warning, leaving automatic refresh to the backend. The check only reads files: it does not invoke auth-status CLIs, contact providers, refresh tokens, write credentials, or take credential locks. To sign in again, use `claude auth login`, `jcode login claude`, or `jcode login openai` as appropriate.

This is an expiration preflight, not proof that authentication will succeed. It cannot detect server-side revocation of refresh tokens and does not read jcode refresh-failure state or macOS Keychain credentials. Step-level `env` API keys are not considered. To bypass a false positive, set the process environment variable `CRUISE_SKIP_AGENT_AUTH_CHECK=1`; only the exact value `1` opts out, not `true` or whitespace-padded values.

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```

In all SDK backends, `model` / `plan_model` / per-step `model` are **model references** with the same override precedence as command mode (step `model` > top-level `model` / `plan_model`). For jcode and Claude, the optional `:effort` suffix selects a reasoning-effort tier (`low` / `medium` / `high` / `xhigh` / `max`, plus the aliases `minimal` / `min` / `med` and the numeric spellings `1`..`4`); `off` / `none` / `0` / `5` are also consumed but leave the effort unset, and any other `:` suffix (an OpenRouter `:free` variant, say) stays part of the model id. For jcode/Claude, an effort a provider or model does not support is ignored. OpenCode instead validates effort against its catalog and fails explicitly when unsupported (see below).

At workflow level, `model` and `plan_model` may also be arrays. In SDK mode, the first entry is the primary model and later entries form an implicit fallback chain. A model array with fallback entries enables model fallback automatically; an explicit `retry.fallback_chains` entry for the primary model takes precedence over the array tail. A 429, a 5xx, an HTTP 4xx other than 429, 401, 403, 407, and 408, or a network failure switches immediately to a usable fallback when `--rate-limit-retries` is above zero and no visible text was streamed (a 429 is retried on the same model only when no usable fallback is left); such a 4xx is never resent to the same model, since an identical request would fail the same way. 401, 403, and 407 are authentication or permission failures no other model can answer, so they fail the step at once instead of walking the chain. A 408 is a transport timeout classified as a network failure, so it takes the same immediate-switch path as a 5xx: it moves to a usable fallback first, and is only resent to the same model when no usable fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0`. A model skipped after a 429, a 5xx, or a network failure (a 408 included) is cooled down for 30 minutes in the current process, and so is one whose provider reported it as missing (`400 model_not_supported`, `404 model_not_found`), because there the model rather than the request is the defect; only a model left behind by a plain client-error switch is not cooled, since that status describes the request, not the model's health. In command mode, only the first entry is used and the historical same-model retry behavior remains unchanged.

- `"provider/model[:effort]"` (e.g. `provider/model:xhigh`) -- `sdk: jcode` parses the provider and model separately; an empty side (`"/model"`, `"provider/"`) fails when the prompt runs. `sdk: claude` treats everything except the `:effort` suffix as the model name.
- `"model"` (no `/`) for jcode/Claude -- no explicit provider is supplied. Configured OpenCode references require a provider.
- `"provider/model[:effort]"` or `"provider/model#variant"` for `sdk: opencode`. Embedded `/` and unknown colon suffixes such as `:free` or `:2026` remain in the model ID. Recognized trailing effort suffixes are reserved and cannot be combined with `#variant`.
- Unset -- no explicit model is supplied. OpenCode uses its configured default. A jcode prompt fails with a setup
  error if it still has no effective model; Claude and `command:` retain their
  existing no-model behavior.

#### Subagent model

Set `subagent_model` at workflow level or on a prompt step to select the model
for agents it spawns. A step value overrides the workflow value. It accepts a
single nonblank string or `{sdk: claude, model: haiku}` (also `sdk: jcode`), not
an array or fallback chain. Strings inherit the workflow SDK, defaulting to
jcode, not the SDK of a fallback attempt. Only attempts whose SDK matches the
resolved subagent SDK receive the setting; other attempts keep backend defaults.
Validation rejects a setting with no applicable model candidate.

```yaml
sdk: claude
model: sonnet
subagent_model: haiku
steps:
  review:
    prompt: Review the implementation, delegating checks as needed.
    subagent_model: {sdk: claude, model: sonnet}
```

- Claude sets `CLAUDE_CODE_SUBAGENT_MODEL` and
  `CLAUDE_CODE_SUBAGENT_MODEL_FORCE=1`, fixing all subagents including Explore
  and Plan. These injected values override same-named workflow/step `env:` keys.
  Without `subagent_model`, Cruise leaves those environment keys unchanged.
- jcode sets the session's swarm default before the prompt. A worker's explicit
  `model` still takes precedence, so this is a default override, not a force.
- OpenCode subagent models and `command:` backends are unsupported. A mixed-SDK
  workflow may target a Claude/jcode candidate with an explicit SDK object.
  Recognized `:effort` suffixes are rejected, even `:off`; unknown suffixes such
  as `:free` remain part of the model ID. jcode requires a valid nonempty model.
- Applies to workflow prompt steps, including groups, `after-pr`, parallel
  prompt children, and `github-review`, plus planning's plan/fix/ask turns.
  It does not apply to title generation, PR metadata, `workflow generate`, or `workflow make`.
  Non-prompt steps and group/workflow-call/parallel wrappers cannot set it.
- A called workflow's top-level value is ignored in favor of the caller's
  default; the callee's per-step overrides are retained.
- Layered `default.yaml` and `CRUISE_SUBAGENT_MODEL` support it, but application
  `config.json` / `config.yaml` defaults do not.

#### `sdk: jcode` — the jcode SDK (default)

`sdk: jcode` uses the official jcode Rust SDK and requires jcode v0.88.0 or newer. At launch, Cruise validates the SDK handshake and required capabilities (`sessions`, plus `session_tools` when custom tools are registered); it does not probe the CLI version. Upgrade jcode when a required capability is missing. The `model`, `plan_model`, and per-step `model` fields accept `provider/model[:effort]` or a bare model. Every actual jcode prompt must resolve to a model: ordinary prompts use step `model` then workflow/default `model`; planning calls use `plan_model` then `model`. A configured jcode default, `JCODE_MODEL`, or resume session does not replace a missing Cruise model. Missing models create or preserve the user `default.yaml` template and stop that request. The effort suffix is applied through the SDK.

Each SDK run uses a private session home with inherited login records and copied
model catalogs. The runtime's `JCODE_HOME` points to the real session directory;
on Unix, a short symlink alias is used only for SDK socket paths. This satisfies
jcode's account-store safety checks without changing saved-session locations or
resumption.

##### Computer use

Workflow-level `computer_use` defaults to `false`. Set it to `true` to allow prompt turns to use jcode's `macos_computer_use` desktop-control tool on macOS. A prompt step may set `computer_use: true` or `false` to override the workflow default. Built-in plan, fix-plan, and ask-plan turns follow the workflow value; title and PR-description generation always run with computer use off. The tool is macOS-only and the setting has no effect on Linux. Every effective applicable model candidate must use jcode, including SDK-qualified objects and candidates selected by authoritative retry chains, and no `command:` backend may be set. Explicit jcode-only candidates can work under a Claude or OpenCode workflow SDK. Changing only the workflow SDK does not override incompatible SDK-qualified objects.

When computer use is off, Cruise adds `macos_computer_use` to `JCODE_DISABLED_TOOLS`, merging the first available list from workflow `env:`, the Cruise process environment, or `[tools].disabled` in the private copied `config.toml`. This reaches the private runtime's daemon, including swarm workers and subagents. When computer use is on with full permission and no explicit web/subagent denials, Cruise leaves `JCODE_DISABLED_TOOLS` unchanged and never removes a jcode-level disable to force-enable the tool. The user must grant macOS Accessibility and Screen Recording permissions.

### Permission modes

Workflow-level `permission` defaults to `full`. A prompt step may set `permission: read-only | edit | full` to override it. `full` keeps today's behavior. `edit` disables shell execution (Claude `Bash`, jcode `bash`) but keeps direct edit/write/patch tools. `read-only` also disables file mutation tools (Claude `Edit`/`Write`/`NotebookEdit`, jcode `edit`/`write`/`apply_patch`/`replace`). Restricted modes force `macos_computer_use` off even with `computer_use: true`, and Claude runs them with `dontAsk` so no permission prompt can block an unattended run. Jcode's mode-specific tools are merged into the selected `JCODE_DISABLED_TOOLS` list.

- `read-only` and `edit` are rejected with a `command:` backend, because cruise cannot control an arbitrary CLI's tools.
- `read-only` combined with `allow_commit: true` is a configuration error. `allow_commit: true` removes only the HEAD/ref commit guard and implicit Git denials, not explicit restrictions, and never widens the permission mode.
- A group call, `workflow_call`, or `parallel` wrapper cannot set `permission`. Set it on the inner prompt or parallel prompt child. A parallel block with any read-only child must contain only read-only prompt children.
- A `workflow_call` callee's top-level `permission` is ignored (the caller's default applies), while per-step overrides inside the callee are kept.
- A read-only step additionally fails with `ReadOnlyWorkspaceChanged` if the workspace snapshot changes during the step. This is change detection, not an OS sandbox or rollback, and it ignores `.git`, `target`, and `node_modules`.
- Built-in planning, fix/ask, title, PR-description, and workflow-generation turns always run with `full`, without `disallowed_tools` or implicit Git restrictions.

### Tool restrictions

`disallowed_tools` narrows SDK prompt capabilities without granting permissions:

```yaml
disallowed_tools: ["shell:npm publish"]
steps:
  review:
    prompt: Review without publishing or delegating.
    disallowed_tools: [web, subagent, "mcp:github", "shell:gh pr merge"]
```

| Value | Denied capability |
|-------|-------------------|
| `web` | Web fetch/search and backend browser tools where available |
| `subagent` | Subagent or swarm delegation |
| `mcp` | All workflow `mcp_servers` entries |
| `mcp:<name>` | One workflow MCP server; unknown names are errors |
| `shell:<prefix>` | Simple shell commands whose leading words match the prefix |

These are SDK-neutral values, not Claude `Bash(...)` syntax. Use `permission`
for blanket shell/edit restrictions; bare `shell` and `edit` are invalid.
Shell prefixes are trimmed and repeated whitespace is collapsed. They must be
nonempty and use only ASCII letters, digits, spaces, and `._/:=@+%-`.
Wildcards, quotes, commas, parentheses, shell operators, backslashes, newlines,
and control characters are rejected. A final word ending in `=` matches the
start of that word, for example `shell:git -c core.hooksPath=`.

The effective list is the deduplicated **union of workflow defaults, step
restrictions, and implicit Git rules**. A step's `[]` cannot undo workflow
restrictions. Across user/project `default.yaml` and workflow layers, the
highest-precedence explicit workflow-level list replaces lower lists;
workflow `disallowed_tools: []` clears inherited defaults. With `edit` or
`read-only`, shell rules are dropped because shell is already disabled.

Nonempty lists are prompt-only, including parallel prompt children, prompts
inside groups or expanded workflows, `after-pr`, and `github-review` loops.
Command/option steps, parallel wrappers, group and `workflow_call` call sites,
and native `squash-merge` reject them. Called workflows retain step lists but
their top-level list is ignored in favor of the caller's. A `command:` prompt
backend rejects explicit restrictions and does not enforce implicit shell
rules; its existing commit guard remains active.

Unless `allow_commit: true`, SDK prompts implicitly deny these shell prefixes:

```text
git commit                 git merge                  git rebase
git cherry-pick            git revert                 git am
git pull                   git push                   git reset
git switch                 git checkout -b            git checkout -B
git update-ref             git config core.hooksPath  git -c core.hooksPath=
```

This also blocks `git reset HEAD file`; use `git restore --staged` to unstage.
It does not block `git checkout -- file`, `git stash create`, `git branch`
listing, or `git status`/`diff`/`log`. Prefix matching cannot distinguish all
safe and unsafe Git operations, so commit guards remain the final defense.
Unlike the repository-scoped commit guard, shell prefix denials also apply to
matching commands targeting temporary or unrelated repositories.

Backend enforcement and limits:

- **Claude:** native deny rules map `web` to `WebFetch`/`WebSearch`, `subagent`
  to `Agent`, and shell prefixes to `Bash(...)`. Denies remain active with
  `bypassPermissions`. Native Claude matching does **not** normalize Git global
  options, so `git -C path commit` may not match `git commit`.
- **jcode:** merges `webfetch`, `websearch`, `browser`, and/or `swarm` into
  `JCODE_DISABLED_TOOLS`, preserving user entries. Shell rules use Cruise's
  pre-tool gate appended to existing hooks, selected from workflow env,
  process env, then config `[hooks].pre_tool`. `JCODE_HOOKS_DISABLED` in workflow
  or process env disables the gate and produces a warning. Gate setup failure
  also warns. The gate exits `2` to block, including malformed policy/input;
  hook execution failures or other exit codes are fail-open in jcode. Older
  releases may not support the array form needed to combine existing hooks.
- **OpenCode:** native deny rules plus the Cruise plugin/bridge deny web tools,
  `task` delegation, and matching bash input before execution.
- **jcode/OpenCode shell matching:** splits compound commands, removes leading
  assignments and common wrappers (`env`, `command`, `builtin`, `exec`, `nohup`,
  `nice`, `time`, `timeout`, `stdbuf`, `xargs`), examines `sh`/`bash`/`zsh`/`dash`
  `-c` scripts, compares executable basenames, and normalizes Git global options.
  Heredoc body lines can cause false-positive denials. This is not a complete
  shell interpreter or a security boundary.

MCP restrictions filter only workflow-provided entries before backend launch.
User-level backend MCP configuration remains enabled and unaffected, as do
Cruise's own session tools. These controls are defense in depth, not a sandbox;
the prompt warning, reference-transaction hook, and CAS rollback remain.

**Upgrade note:** On macOS, earlier Cruise versions exposed `macos_computer_use` to every jcode prompt implicitly. Set workflow-level `computer_use: true` to retain that behavior.

The GitHub Action's shell bootstrap retains `jcode version --json`, `jcode login`, `jcode provider add`, and observational `jcode auth status`: it has no SDK client, and the SDK exposes neither the CLI version string nor these account/profile setup operations. The Cruise backend itself uses the SDK capability handshake after launch.

Cruise resolves the process `JCODE_HOME` as its source home, falling back to `~/.jcode`. Each Cruise session gets an isolated SDK home that inherits credentials and config; resumed plan turns reuse it. The private runtime receives `JCODE_CHECK_UPDATES=0` without rewriting the copied config. Plan homes persist until their Cruise session is deleted or cleaned, while non-resumable prompt homes are removed after each turn. Unclaimed homes are pruned after 24 hours only when a cross-process lock proves no run is active. Before removing a private home, Cruise stops only a live `jcode serve` process whose command and `JCODE_HOME`/`JCODE_RUNTIME_DIR`/`JCODE_SOCKET` identify that runtime; it never trusts a registry PID alone. The absolute source-home path is saved with resumable plan sessions so deletion still works when Cruise is launched from another directory.
Model catalog caches are copied into that private home; path-bound external credentials are copied as owner-only files instead of linked, inheriting only trust decisions already recorded for the original paths.

`ask_user` keeps its pending question alive across jcode's 120-second SDK callback deadline. After about 110 seconds without an answer, the tool returns a successful continuation notice asking the model to call `ask_user` again with the identical question; it does not prompt the user again, and an answer received between calls is returned by the next call. The wait remains bounded by the workflow step timeout. When a saved plan conversation is absent from its private jcode home, Cruise starts a fresh SDK session because the current plan is already included in the prompt.

`submit_plan` and `update_plan` only write the plan and return; Cruise's existing approval flow happens after the model turn and does not hold an SDK tool callback open.

**Upgrade note:** After upgrading from an MCP-bridge release, remove the stale `mcpServers.cruise` entry from the source `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json` when `JCODE_HOME` is unset). Cruise strips it from each private session copy and leaves the source configuration unchanged. Until the entry is removed, jcode sessions outside Cruise will continue to launch it. Cruise exits with migration instructions on stderr instead of treating it as legacy plan input, so jcode reports the `cruise` MCP server as failed.

The backend exposes Cruise session tools through jcode's session-tools support.

#### `sdk: claude` — the claude CLI in-process

`sdk: claude` drives the `claude` CLI in-process through claude-agent-sdk. Model references are plain `claude --model` names with the optional `:effort` suffix, forwarded as `--effort`. A `claude` CLI without that flag fails the step with `unknown option '--effort'`, which is classified permanent and never retried. Cruise is verified against 2.1.250. Authentication is the claude CLI's own, using its stored credentials or `ANTHROPIC_API_KEY`, unrelated to `jcode login`. The CLI runs with permissions bypassed because Cruise workflows are unattended, so there is no console to answer a permission prompt on. Cruise's workflow tools are exposed through the SDK. `computer_use: true` is rejected at config load because Claude Code's computer use is only available in interactive sessions and Cruise cannot enable it.

#### Tool-less (non-interactive) planning

By default, SDK-mode planning uses session tools to ask questions and submit or revise the plan.

Set `interactive_planning: false` to turn that off. Planning then embeds the target plan-file path in the prompt and asks the agent to write `plan.md` directly — exactly like the `command` backend. The resulting `plan.md` is read back afterward (falling back to the agent's captured output if the file was not written).

```yaml
interactive_planning: false   # tool-less, file-based planning
```

`--grill` requires the interactive planning flow and is rejected when `interactive_planning` is off. The field has no effect in `command` mode, which is always file-based.

#### `sdk: opencode` - the OpenCode V2 client

Cruise includes a Rust client for the workflow-required OpenCode V2 API, not a Python/Node SDK dependency or a provider-specific LLM client. Install the OpenCode CLI explicitly with `cruise install opencode` or separately, and authenticate it. Cruise owns a short-lived loopback `opencode serve --stdio` process and stops it after each attempt. Workflow execution does not connect to an external server, install the CLI, authenticate for you, or fall back to V1.

```yaml
sdk: opencode
model: "provider/model#variant" # Replace with a model in your OpenCode setup
plan_model: "provider/planning-model"
interactive_planning: true
permission: full
steps:
  implement:
    prompt: "{input}"
```

Use `provider/model[:effort]` or native `provider/model#variant` for workflow, planning, and step models, including array candidates and resumed sessions. Common effort names and aliases are case-insensitive: `low` (`min`/`minimal`/`1`), `medium` (`med`/`2`), `high` (`3`), `xhigh` (`4`), and `max`. Cruise validates the requested tier against OpenCode's live model catalog and selects a matching variant, never guesses or downgrades. Unsupported or ambiguous effort, an unlisted model, and tierless suffixes `:off`/`:none`/`:0`/`:5` fail explicitly before sending a prompt or changing a resumed model, without fallback. Common effort and native `#variant` are mutually exclusive. Native variants (including `#none`), suffixless models, and omitted models keep their existing behavior.

Model arrays use the existing failure-driven fallback policy. A genuine OpenCode `provider.no-route` execution failure (including `Model unavailable`) is treated as a missing model and switches immediately to a configured usable fallback with the existing budget and cooldown. Generic unavailable text, other failure kinds, appended stderr, and authentication/permission or invalid-request failures do not qualify for this special handling. Visible output and replay barriers still prevent switching. OpenCode forbids automatic retry or fallback after tool side effects, uncertain prompt acceptance, or unrecoverable event loss, even if no text was displayed. Omitted models are delegated to OpenCode's configured default. Workflow and step `env:` values reach prompt tools along with the inherited OS environment and commit guard. Existing OpenCode global/project configuration and authentication remain in use. Cruise keeps its own database, plugin, and resumable plan conversations in private XDG storage, not the project or the user's OpenCode database. It does not rewrite the user's configuration. Invalid `OPENCODE_CONFIG_CONTENT` is diagnosed rather than discarded.

Cruise planning, questions, title generation, and PR metadata use the existing tools through a native V2 plugin, not an MCP server. `computer_use: true` is explicitly unsupported. `permission: edit` allows direct file edits but denies shell, subagents, and unknown execution tools. `read-only` denies mutation too, while preserving safe Cruise tools and the workspace snapshot check. Unknown MCP and Code Mode execution paths are denied in restricted modes. These are tool restrictions, not an OS sandbox.

Workflow MCP entries support local `stdio` and remote Streamable HTTP (`type: http`). Legacy `type: sse` is rejected with a named-server diagnostic, never silently converted. No OAuth login is started during unattended runs. Workflow entries overlay OpenCode configuration without changing the source files.

The protocol baseline is OpenCode commit `74f8d4860f49c563909e6b039b0cc8b454ebce72` (investigated October 23, 2026). OpenCode v2.1.3 was identified as the first real-CLI verification target, not a blanket compatibility guarantee. Runtime V2 capability checks and fixture tests do not by themselves establish real-model or plugin interoperability.

See [the OpenCode workflow example](skills/cruise-config/examples/opencode-flow.yaml).

#### MCP servers (`mcp_servers`)

The optional top-level `mcp_servers` map makes MCP servers available to every SDK prompt run: workflow prompt steps (including parallel children), `after-pr` steps, planning turns, session-title generation, and PR-description generation. It works with `sdk: jcode`, with the default `jcode` backend only when neither `sdk` nor `command` remains set in the resolved workflow after applicable `default.yaml` layers are applied, and with `sdk: claude` or `sdk: opencode`. Per-step overrides are not supported, but prompt `disallowed_tools: [mcp]` or `["mcp:<name>"]` can exclude workflow entries for a step; user-level MCP configuration is unaffected. A called workflow's top-level `mcp_servers` is ignored; the caller's settings remain in force.

Entries use the Claude Code `mcpServers` server shape. Omit `type` or set `type: stdio` for a local server. Every effective applicable candidate, including SDK-qualified objects and authoritative retry-chain entries, must support the transport: stdio supports all SDKs, HTTP requires Claude or OpenCode, and legacy SSE requires Claude. Changing the workflow SDK does not override SDK-qualified objects. The command backend cannot receive MCP servers:

```yaml
sdk: claude
mcp_servers:
  local_search:
    command: npx
    args: ["-y", "@example/search"]
    env:
      SEARCH_ROOT: ./docs
  remote_docs:
    type: http
    url: https://mcp.example.test/server
    headers:
      Authorization: "Bearer replace-with-your-token"
```

MCP entry values are passed through without Cruise `{variable}` template resolution. Names must be non-empty and contain only ASCII letters, digits, `_` or `-`; `cruise` is reserved. Stdio entries require a non-blank `command` and cannot set `url` or provide non-empty `headers`. `http` and `sse` entries require a URL beginning with `http://` or `https://` and cannot set `command` or provide non-empty `args` or `env`. Unknown entry fields are rejected. A non-empty `mcp_servers` map with the `command` backend is a validation error.

With jcode, Cruise merges the workflow entries into the private session copy of `$JCODE_HOME/mcp.json` (or `~/.jcode/mcp.json` when `JCODE_HOME` is unset). Workflow entries replace same-named entries from that source file, and the source is never modified. jcode then applies its normal config merge order, so same-named servers from `~/.claude.json`, `~/.claude/mcp.json`, or project-local `.jcode/mcp.json`, `.mcp.json`, and `.claude/mcp.json` take precedence over the workflow copy. jcode expands `${VAR}` and `${VAR:-default}` using the runtime environment, including workflow-level `env:` values.

With `sdk: claude`, workflow MCP entries are written to a per-run private JSON file with mode `0600` on Unix (an owner-only ACL on Windows), and the file path is passed via `--mcp-config`; the SDK passes Cruise's in-process tools separately as its own inline `--mcp-config` value. The user's own Claude MCP configuration remains enabled. `${VAR}` expansion for these Claude entries is not guaranteed.

### Prompt Languages

UI display language is independent of prompt languages. Configure it with application `uiLanguage`, `CRUISE_UI_LANGUAGE`, or the TUI language picker; a WebUI browser can override the server default with its own language selector. These settings do not change `languages.pr` or `languages.plan`.

`languages.pr` controls the language used for the auto-generated PR title and body, and `languages.plan` controls the language used by cruise's built-in planning prompts. `CRUISE_LANGUAGE_PR` and `CRUISE_LANGUAGE_PLAN` override the corresponding YAML values; blank environment values are ignored. Otherwise, nested fields take precedence over deprecated `pr_language` / `plan_language`, then the first non-empty of `LC_ALL`, `LC_MESSAGES`, `LANG`, `LANGUAGE` (mapped once; an unsupported value does not fall through), then `English`. The deprecated fields remain supported as fallbacks.

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

### Commit and PR Templates

Optional top-level string keys `commit_message_template` and `pr_body_template` customize the host-created commit and PR body in the worktree PR creation path. Set them in the workflow or in user `<config_dir>/default.yaml` or project `.cruise/default.yaml`. Precedence is workflow > project defaults > user defaults > unset. They are not CLI options or environment overrides.

```yaml
commit_message_template: "{title} [{session_id}]"
pr_body_template: |
  ## Summary
  {body}
```

| Template | Allowed variables |
|----------|-------------------|
| `commit_message_template` | `{title}`, `{issue}`, `{session_id}`, `{branch}` |
| `pr_body_template` | `{title}`, `{body}`, `{issue}`, `{session_id}`, `{branch}` |

`{title}` is the trimmed LLM-generated PR title, or the first non-empty trimmed line of the session input or title if generation yields an empty title. `{body}` is the trimmed LLM-generated PR body, or an empty string if generation fails. `{issue}` currently always expands to an empty string, `{session_id}` is the session ID, and `{branch}` is the worktree branch name. No issue closing text is added automatically by these templates.

Unknown variables and malformed syntax are rejected at workflow load/compile time. Escape literal braces as `{{` and `}}`; expanded values are not expanded again. The rendered body and commit message are trimmed. An empty rendered commit message is an error; the configured Co-authored-by trailer is appended separately. When a PR body template is set, Cruise supplies the title and body explicitly rather than using `--fill`.

When both templates are unset, existing behavior is unchanged: the commit uses the generated PR title or existing input fallback, and an empty PR title uses `--fill`. These settings do not affect current-branch runs, `create_pr: false`, agent-created commits, or squash-merge messages.

### Session Title Generation

After plan approval, cruise generates a concise session title (up to 80 characters) shown in `cruise list` and the WebUI sidebar instead of the raw task input. The behavior depends on the backend:

- **SDK mode (`sdk:` set, or neither `sdk:` nor `command:` remains set in the resolved workflow after applicable `default.yaml` layers are applied -- then the default `jcode` backend)** -- on the foreground `cruise plan` approval path, cruise makes a separate title-generation request using `plan_model` -> `model`. If that jcode call has no effective model, approval fails and is not saved. Other title-generation failures fall back to `plan.md` metadata. Approval from `cruise list`, the WebUI/TUI, and background planning derive the title from `plan.md` instead.
- **Command mode (`command:` set)** -- no LLM is called for title generation. The title is derived automatically from the first heading or first non-empty line in the generated `plan.md`.

For jcode, configure an effective model through the workflow, applicable
defaults, or `CRUISE_MODEL` / `CRUISE_PLAN_MODEL` as appropriate. Claude and
command workflows retain their existing no-model behavior.

### Environment Variables

`CRUISE_JCODE_CLI_PATH`, `CRUISE_CLAUDE_CLI_PATH`, and `CRUISE_OPENCODE_CLI_PATH` select provider executables from the Cruise process environment. They are not workflow configuration keys and cannot be supplied through workflow `env:`; see [Managed provider CLIs](#managed-provider-clis).

Environment variables can be set at workflow and step level. Step-level values override workflow-level values for that step only. Parallel children add a third level: workflow < parallel block < child. Values support template variable substitution; each child resolves its own values against the variables from before the block.

Process-level configuration overrides apply after workflow and defaults resolution. Supported names and value types are:

| Variable | Configuration key | Value |
|---|---|---|
| `CRUISE_MODEL` | `model` | model |
| `CRUISE_PLAN_MODEL` | `plan_model` | model |
| `CRUISE_SUBAGENT_MODEL` | `subagent_model` | model entry |
| `CRUISE_SDK` | `sdk` | text |
| `CRUISE_COMMAND` | `command` | JSON array |
| `CRUISE_MAX_RETRIES` | `max_retries` | integer |
| `CRUISE_INTERACTIVE_PLANNING` | `interactive_planning` | boolean |
| `CRUISE_CLEANUP_AFTER_PR` | `cleanup_after_pr` | boolean |
| `CRUISE_FORCE_EXEC` | `force_exec` | boolean |
| `CRUISE_COMPUTER_USE` | `computer_use` | boolean |
| `CRUISE_PERMISSION` | `permission` | text: `read-only`, `edit`, or `full` |
| `CRUISE_DISALLOWED_TOOLS` | `disallowed_tools` | JSON array |
| `CRUISE_COMMIT_MESSAGE_TEMPLATE` | `commit_message_template` | text |
| `CRUISE_PR_BODY_TEMPLATE` | `pr_body_template` | text |
| `CRUISE_RETRY` | `retry` | JSON object or `null` |
| `CRUISE_RETRY_BASE_DELAY_MS` | `retry.base_delay_ms` | integer |
| `CRUISE_RETRY_MAX_DELAY_MS` | `retry.max_delay_ms` | integer |
| `CRUISE_RETRY_MODEL_FALLBACK` | `retry.model_fallback` | boolean |
| `CRUISE_RETRY_FALLBACK_CHAINS` | `retry.fallback_chains` | JSON object |
| `CRUISE_LANGUAGES` | `languages` | JSON object |
| `CRUISE_LANGUAGE_PR` | `languages.pr` | text |
| `CRUISE_LANGUAGE_PLAN` | `languages.plan` | text |
| `CRUISE_RUN_ALL_PARALLELISM` | app `run_all_parallelism` | integer, at least 1 |

Names normally use `CRUISE_` plus the uppercase key path, replacing dots and hyphens with underscores. The language child names above are the only supported spellings. Values are trimmed and whitespace-only values are ignored. Booleans accept only `true`, `false`, `1`, or `0`; integers are unsigned decimal digits without a sign. Text is literal. Model values are literal single models unless they start with `[` or `{`, in which case they are parsed as JSON and must match the field's normal model shape. In particular, `CRUISE_MODEL` and `CRUISE_PLAN_MODEL` now accept JSON arrays: values beginning with `[` or `{` were previously treated as literal model names. Arrays with fallback entries enable model fallback as in YAML. Other composite values require JSON, not YAML flow syntax. Use JSON `null` for an optional value or `[]` for an empty list where supported, not a blank value. Invalid values produce an error naming the variable and still undergo normal configuration validation.

Parent objects are applied before child overrides, so `CRUISE_RETRY_MAX_DELAY_MS` overrides the corresponding field in `CRUISE_RETRY`, and `CRUISE_LANGUAGE_PR` overrides `CRUISE_LANGUAGES`. Setting `CRUISE_SDK` clears a configured command, while `CRUISE_COMMAND` clears a configured SDK. Setting both nonblank variables is an error. For example:

```sh
export CRUISE_MODEL='["provider/primary", "provider/fallback"]'
export CRUISE_RETRY='{"base_delay_ms":1000}'
export CRUISE_RETRY_MAX_DELAY_MS=5000
```

`CRUISE_RUN_ALL_PARALLELISM` overrides the app setting for TUI/WebUI run-all operations and effective config display, but is not saved to the config file. It does not change CLI `run --all`, which uses `--parallelism`. File-backed sessions reapply environment overrides when reloaded; snapshot sessions retain the resolved config from creation. Language settings fall back to locale inference from `LC_ALL`, `LC_MESSAGES`, `LANG`, then `LANGUAGE` when no explicit language is configured.

The keys `env`, `mcp_servers`, and `pre_steps` are excluded because they contain named entries, commands, or secrets. Workflow structure and identity (`steps`, `groups`, `after-pr`, `inputs`, `returns`, `description`, `flags`), the `create_pr` workflow contract, and deprecated `pr_language` / `plan_language` are also excluded. `formal_verify` is configured through workflow YAML or defaults, not a configuration environment override. Process switches such as `CRUISE_CONFIG`, `CRUISE_FORGE`, `CRUISE_RUNTIME`, `CRUISE_SKIP_AGENT_AUTH_CHECK`, `CRUISE_DISABLE_NOTIFICATIONS`, `CRUISE_DISABLE_HERDR`, `CRUISE_OBSERVABILITY_ENABLED`, and `CRUISE_COMMIT_COAUTHOR_NAME` / `CRUISE_COMMIT_COAUTHOR_EMAIL` are separate from configuration overrides.

`CRUISE_UI_LANGUAGE` instead controls the process UI language, not a workflow field or generated prompt language. See [`cruise config`](#cruise-config) for supported tags and precedence.

`JCODE_OPENAI_SERVICE_TIER` controls jcode SDK's OpenAI service tier. Cruise defaults it to `off` when neither workflow `env:` nor the process environment defines the key. Explicit values are respected.

`JCODE_DISABLED_TOOLS` is jcode's comma- or newline-separated disabled-tool list. When `computer_use` is false, Cruise uses workflow `env:` first, then the process environment, then `[tools].disabled` from the private jcode `config.toml`, and appends `macos_computer_use` without discarding the selected policy. When `computer_use` is true, permission is `full`, and no explicit web/subagent denials apply, Cruise does not alter this variable or the jcode config, so a tool disabled by jcode remains disabled. Restricted `permission` modes append their tools (`bash`, plus `edit`, `write`, `apply_patch`, `replace` for `read-only`) to the selected list.


`CRUISE_COMMIT_COAUTHOR_NAME` and `CRUISE_COMMIT_COAUTHOR_EMAIL` add a `Co-authored-by:` trailer to the commits cruise creates for a PR. Both must be set and non-blank, and a name containing `<`, `>`, or a line break -- or an invalid address -- disables the trailer instead of failing the commit.

`CRUISE_DISABLE_HERDR=1` disables the herdr lifecycle-state reporting described above.

`CRUISE_SKIP_AGENT_AUTH_CHECK=1` bypasses the SDK startup OAuth expiration check described under SDK Mode. It is a process-only runtime switch, not a workflow-config override. Only the exact value `1` skips the check.

`CRUISE_USAGE_RECORDING` is a process-only runtime switch. Set it to `0` or `false` to disable local usage recording; `1`, `true`, or an unset value enables it. Values are trimmed and case-insensitive. Other values enable recording with a warning.


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

Prompt steps can declare `quality_gates` to verify a successful attempt before advancing; see [Quality gates](#quality-gates-quality_gates).

```yaml
steps:
  planning:
    model: claude-opus-4-5        # model to use (optional; overrides top-level model)
    instruction: |                # message displayed before the step runs (optional)
      Before planning, add any constraints or context:
    prompt: |                     # prompt body (use either prompt or prompt_file)
      Create an implementation plan for:
      {input}
    timeout: 10m                  # per-step timeout (optional; see Step Timeout)
    output_file: initial-state.md # save the final response as a session artifact (optional)
    allow_commit: false           # default: guard this prompt from moving Git HEAD in this repository
    computer_use: true            # optional; jcode only; overrides top-level computer_use
    env:                          # environment variables for this step (optional)
      ANTHROPIC_MODEL: claude-opus-4-5
```

When `output_file` is set, cruise saves the completed backend response outside the
workspace at the session's `artifacts/` directory. The file is written atomically,
keeps the exact response text, and is not included in workspace change detection.
Later prompt bodies can read it with `{file:initial-state.md}`. File references
are resolved immediately before a prompt runs, including prompts loaded through
`prompt_file`; the inserted text is not recursively expanded. Artifact names must
be relative and stay within the session directory, and saves and reads are limited
to 1 MiB. Names are rejected when empty, absolute, or containing a `.` or `..`
path component; a subdirectory such as `reports/initial-state.md` is allowed and
its directories are created on write. Symbolic links are refused rather than
followed: a symlink anywhere in the artifact path -- including the session's
`artifacts/` root or its managed parent directories -- fails both reads and
writes, as does a final path that exists but is not a regular file.
See [`examples/file-artifacts.yaml`](examples/file-artifacts.yaml) for a
baseline-and-regression comparison flow. Artifacts belong to the session: deleting
the session removes them. Terminal `cruise exec` sessions are cleaned up after
completion, while interrupted exec sessions remain resumable by ID and `cruise run`
sessions retain artifacts for resume.

An `instruction` is a message shown to the user immediately before a step runs, not a system prompt. If `{input}` is empty, the message is used as a multiline input prompt and the submitted text becomes `{input}`; otherwise it is displayed without being sent to the LLM.

Prompt steps are commit-guarded by default. SDK prompts also receive the implicit Git shell denials described under [Tool restrictions](#tool-restrictions); `allow_commit: true` removes these and the guard, not explicit `disallowed_tools`, and never grants broader permissions. Set `allow_commit: true` only when a prompt intentionally needs to create a commit or otherwise move Git `HEAD`; omitted and `false` values keep the guard enabled. The guard covers classic `command:` mode and all SDK backends (`jcode`, `claude`, and `opencode`), while command and option steps remain unaffected. Ref rejection is scoped to the repository the step runs in, identified by its git common dir: `HEAD` and `refs/heads/*` updates are rejected in every worktree of that repository, including the main checkout and any linked worktree. The repository-scoped commit guard permits ref updates in throwaway repositories created below the step (temporary repositories used by test suites, other clones, nested repositories), but SDK shell prefix denials still block matching Git commands targeting those repositories. Commits in the guarded repository's own checkout or any of its linked worktrees remain rejected. A detected movement fails the step and cruise may restore the original branch reference without resetting the index or worktree.

When `allow_commit` is omitted or `false`, cruise also appends an instruction telling the agent to leave edits uncommitted, not move `HEAD` or local branch refs in the working repository or its linked worktrees, and not bypass the commit guard even if the task requests it. This notice supplements Git enforcement; `allow_commit: true` omits this commit-guard notice, but explicit `disallowed_tools` still adds its own restriction notice.

Set `allow_commit: true` on the actual prompt, not a `group:` or `workflow_call:` invocation. The guard is implemented by injecting `core.hooksPath` and `CRUISE_GUARD_COMMON_DIR` through the step's child environment, which installs a `reference-transaction` hook that rejects ref updates only in the repository named by that variable. Only that rejection is repository-scoped: the injected hooks path applies to every git invocation below the step, in every repository, so a repository's own hooks (`pre-commit`, `commit-msg`, `pre-push`, ...) do not run under a guarded step, not even in an unrelated repository whose ref updates are allowed. The hook identifies the current repository from the `GIT_DIR` that git exports to hooks, following the `commondir` file for linked worktrees, and falls back to `git rev-parse --git-common-dir`. The guard is fail-closed: if the repository of a transaction cannot be resolved the transaction is rejected and the step fails with a commit guard violation, while a missing working directory or a directory outside a Git worktree is still a graceful no-op. It remains a best-effort same-user policy, not a security boundary: other repositories are not post-checked.

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

#### Parallel Steps

Use `parallel:` to run named prompt or command steps concurrently within one
session. The next step starts after every child finishes.

```yaml
steps:
  checks:
    parallel:
      lint:
        command: cargo clippy -- -D warnings
      tests:
        command: cargo test
      review:
        prompt: Review the changes and report findings without editing files.
        timeout: 10m
    timeout: 15m
  summarize:
    prompt: "Summarize the check results: {prev.output}"
```

- All children start together in the session's existing working directory.
  Use independent commands or file edits; cruise does not isolate or merge child
  changes. Concurrent edits to the same files or Git state can conflict.
- Each child receives the same input and `{prev.*}` values from before the
  block. Environment precedence is workflow < parallel block < child. Children
  can use `model`, `env`, `skip`, `when`, and `timeout`; prompt children may also
  set `instruction`, `computer_use`, `permission`, and `disallowed_tools` (the parent cannot), and `output_file` with distinct artifact names within the block. Parallel instructions are noninteractive and require nonempty runtime input. Supply input or use a preceding sequential instruction step, otherwise execution returns an error without opening an input editor. `instruction` is rejected on command children. `prompt_file`
  resolves relative to its config as usual. Command arrays remain sequential per
  child.
  A child prompt that reads an artifact produced by a sibling in the same block
  (`{file:...}` pointing at another child's `output_file`) is rejected at config
  validation time, as are an empty `parallel: {}` block and a child whose
  `command:` array is empty.
- After joining, `{prev.output}` is a JSON object keyed by child name in YAML
  declaration order. Each entry has `output` (prompt text, or `null` for commands),
  `stderr` (including execution errors), `success`, and `skipped`.
  `{prev.success}` is true when every child succeeded or was skipped;
  `{prev.stderr}` combines child errors with their names. Logs include
  `[parent/child]` prefixes. Command children have no interactive stdin.
- A child failure does not cancel siblings. The block counts as one step and
  one failure at most, following normal step failure semantics. Put `if.fail`
  or `next` on the parent; a retry reruns the entire block. Unhandled prompt or
  execution errors stop the workflow after siblings finish. Nonzero command
  exits and timeouts follow the ordinary step failure path.
  Two child errors ignore that aggregation: a commit-guard violation or a
  missing-model error from a jcode prompt is always fatal and stops the workflow
  even when the parent has `if.fail`, while a child timeout is never fatal on its
  own and always reports through `{prev.*}` and the ordinary step failure path.
- A child timeout affects only that child. A parent timeout cancels unfinished
  children and waits for them to stop before taking the failure path. Ctrl+C
  stops all children. Resume restarts the whole interrupted block, including
  children that already finished. Graph display and step selection treat the
  block as one step.
- Parallel blocks also work inside `groups`, `after-pr`, and called workflows.
  Children must be prompt/command steps; child `next`, `if`, `option`,
  `plan`, `group`, `workflow_call`, nested `parallel`, `flag`, and
  `allow_commit: true` are rejected. Configure flow control on the parent (including `flag`) and
  perform commits in a subsequent sequential step.

#### Step Timeout

Prompt, command, and parallel steps may set `timeout:` to abort the step if it runs too long (option steps ignore it; in a command array each command gets the timeout; on a parallel block it limits the whole block). Accepted formats:

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
        next: ~                          # null/omitted next falls through to the
                                         # next declared step; the last step ends the workflow
```

### Post-PR Automation (`after-pr`)

For host-created commit and PR body formatting, see [Commit and PR Templates](#commit-and-pr-templates).

Prompt steps here, including `github-review` prompts, inherit workflow
`disallowed_tools` and may add step restrictions. Native `squash-merge` is not
a prompt and cannot carry a nonempty `disallowed_tools` list.

On GitLab, the built-in `after-pr` steps detect `/-/merge_requests/` in `{pr.url}` and use `glab` (`glab mr update --ready`, `glab ci get --merge-request`, `glab ci trace`) instead of `gh`.

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

Prompt steps in `after-pr` are guarded by the same default, with the same repository scope and the same fail-closed behaviour. The built-in `resolve-conflict` and `fix-ci-error` prompts explicitly set `allow_commit: true` because they are intentionally expected to commit fixes to the PR branch. Other after-PR prompts must opt in individually when they need to advance `HEAD` in the session repository; command and option steps are unaffected.

#### Review loop (`github-review`)

An explicit `after-pr` step can handle review bots such as CodeRabbit. The general-purpose built-ins do not add it; `builtin:pr-review-fix` uses it for an existing PR.

```yaml
after-pr:
  coderabbit:
    github-review:
      bots: ["coderabbitai[bot]"]   # any one of these reviewing the current PR head ends the wait
      max-iterations: 3             # default 3
    prompt: |
      Fix the review comments and commit the changes.
      Reply with the requested JSON describing each thread.
    allow_commit: true
    timeout: 30m                    # required; bounds the whole loop
    output_file: review-report.json # optional iteration report in the session artifacts
```

In bot mode, each iteration waits for a review of the current head commit by a listed bot, then fetches unresolved threads started by those bots (all pages). In all-reviewer mode, it immediately fetches eligible unresolved threads without waiting for a review. Either mode runs the prompt with the head OID and thread data appended. The prompt must answer with JSON only: `{"actions":[{"thread_id":"...","reply":"optional","resolve":true}]}`. Only listed thread IDs are accepted, each at most once, with a non-empty `reply` or `resolve: true`, and only where `viewerCanReply` / `viewerCanResolve` allow it. Cruise itself posts the replies and resolves the threads through `gh api graphql` (requires an authenticated `gh`). Commits made by the prompt are pushed; uncommitted changes fail the step. The loop ends when no eligible unresolved threads remain and fails when eligible threads remain after `max-iterations`, on timeout, or on any API or response error. Unlike other `after-pr` errors, which are warning-only, this `github-review` failure marks the run `Failed`; cancellation and missing-model errors from jcode after-pr prompts also propagate, with missing-model errors failing the run/session. Rerunning resumes safely because replies carry an idempotency marker. The step is valid only directly in `after-pr`.

The default `reviewers: bots` preserves that behavior and requires a nonempty, unique `bots` list. To process existing human and bot feedback instead, use `github-review: { reviewers: all, max-iterations: 3 }` and omit `bots` or specify an empty list. This mode does not wait for a new review. Before passing thread text to the agent, Cruise checks each author's repository collaborator permission: only `write`, `maintain`, or `admin` authors are eligible. A 404 means no permission; other permission lookup failures fail the step. Excluded unresolved thread IDs are recorded in the report and do not block success. The loop succeeds when no eligible unresolved threads remain, fails if an iteration leaves the eligible ID set unchanged, and fails if the iteration limit is reached with eligible threads remaining. The builtin PR review workflow uses this mode with a 60-minute timeout, three iterations, and `allow_commit: true`.

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

When `next` is omitted or set to null (`~`), the workflow falls through to the next step in declaration order and ends only after the final step. An explicit `next` value jumps to the named step.

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

#### Workflow flags

Declare flags in top-level `flags:` and attach one to a step with `flag: <name>`. A flagged step runs only when its flag is `on` and is skipped when it is `off`.

- **Targets:** a regular step, a group call site, a `workflow_call` call site (applied to every expanded step), a `parallel` parent, and steps under `after-pr`. Steps inside a group definition, a called workflow's own steps, and parallel children cannot use `flag:`.
- **Default off:** every declared flag is off when absent.
- **Set values:** `--flag NAME` enables one flag; `--flag NAME1,NAME2` enables a comma-separated selection, and repeated `--flag` options are allowed. Explicit `NAME=on|off|true|false` assignments set or clear individual values. Fresh single-session TTY execution prompts for declared flags unless that invocation supplies `-f/--flag`, with saved enabled values preselected (see [Workflow flags](#cruise-run)); unknown names are rejected.
- **Resume:** saved session values are preserved; supplied `--flag` values override only the named flags.
- **Union with skips:** flag-disabled steps are combined with the user-selected `skipped_steps`; a step is skipped if either applies. CLI, TUI, and WebUI skip selectors and the WebUI and TUI Graph tabs show flagged steps only when enabled, except that legacy unversioned graph checkpoints are displayed without filtering. New sessions drop flag-disabled skip entries so enabling a flag later does not retain a hidden skip.

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
- An empty glob is rejected at config validation time, and so is a syntactically invalid one -- unless the pattern contains `{`, in which case static validation is skipped because the braces may be a template variable. Such a pattern is checked only after substitution, when an invalid glob fails the step at runtime.
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

> Conditional cycles are allowed when a normal exit is possible. Execution preflight rejects a reachable cycle only when no normal exit is reachable from the actual start position, considering configured and user-selected skips. A branch that may enter an exitless cycle produces a warning, not a blanket rejection. Runtime edge budgets still apply.

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
- Parses alongside `if: file-changed` on the same step, but `no-file-changes` disables it: no `file-changed` edge is compiled for that step and no jump to its target ever happens.
- The legacy `fail-if-no-file-changes: true` field is rejected as an unknown field; migrate to `if: { no-file-changes: failed }`.

##### Declaring intentional no-changes

Not every no-change is a failure to route around -- sometimes the plan explicitly says a step should make no changes (e.g. "don't add tests here"), and an agent that reaches that conclusion again on every retry is giving the correct answer, not stalling. Two ways to tell cruise the no-change is deliberate; either one disables **both** actions (`failed` and `retry`) for that attempt:

- **Output marker** -- a line in the step's raw output starting with `NO_CHANGES_INTENTIONAL: <reason>` (leading whitespace on the line is ignored). Works with every backend (`command:` and all three `sdk:` modes) since it's plain text matching, no tool support required. The marker must anchor the start of a line -- a mid-line mention (quoted in passing, inside a code block, etc.) does not count.
- **Structured SDK response** -- SDK-mode agents can declare intentional no-changes through a schema-validated structured response on prompt steps with an `if.no-file-changes` condition. In classic `command:` mode, use the output marker.

Either path logs the declared reason so the decision stays visible in the run output. Parallel children cannot make this declaration for their parent: child output markers remain inside the aggregate JSON, and children do not receive the structured response. A parent `if.no-file-changes` condition still evaluates file changes across the entire block.

#### Failure handling (`if.fail`)

`if.fail` decides what happens when a recoverable step failure occurs: a non-zero exit code from a command step, a recoverable prompt error (including LLM transport errors), a `timeout`, or a `no-file-changes: failed` trigger. A missing-model error from a jcode prompt is not recoverable by `if.fail`; it bypasses the handler and aborts the workflow.

Two forms are meaningful (other mappings such as `{}` or `{ retry: false }` parse and simply fall through):

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

`if.fail` is subject to the same loop-protection budget as other flow-control jumps (`--max-retries`, else top-level `max_retries`, else 3). The budget covers every graph edge except the user-skip fallback and the group-retry-exhausted exit, so plain sequential advances and explicit `next`/option jumps are counted too: traversing any single budgeted edge more times than the ceiling stops the run with loop protection rather than letting a misconfigured loop run forever.

Conditional cycles are allowed when a normal exit is possible. Execution preflight rejects a reachable cycle only when no normal exit is reachable from the actual start position, considering configured and user-selected skips. A branch that may enter an exitless cycle produces a warning, not a blanket rejection. Runtime edge budgets still apply.

**Constraints:**
- `if.fail` is rejected at the group level and in `after-pr` steps.
- Can be combined with other `if:` keys (`file-changed`, `no-file-changes`) on the same step.

#### Quality gates (`quality_gates`)

Use a non-empty list of commands to check a prompt step's work before its transition is applied:

```yaml
max_retries: 2
steps:
  implement:
    prompt: Implement the requested change.
    quality_gates:
      - command: cargo test
      - command: cargo clippy
        timeout: 10m
    if:
      fail: recover
    next: done
  recover:
    prompt: Investigate the failed implementation.
  done:
    command: echo done
```

Each entry requires a nonblank `command` and may specify a positive `timeout` using the usual seconds, `m`, or `h` syntax. Commands run in order after a successful prompt attempt, including the final step, and stop at the first failure. Exit zero without a timeout or signal passes. Skipped steps, failed prompt attempts, and attempts discarded by `if.no-file-changes: retry` do not run gates. A pending `if.file-changed` transition is applied only if the gates pass.

A failed gate retries the same prompt step before applying its normal transition. The next prompt receives a sanitized, bounded excerpt of the command output as untrusted data. When available, a sanitized log is stored under the session's `artifacts/quality-gates/` directory, outside the repository, subject to the artifact size limit. Feedback survives checkpoint/resume and is cleared after the retry is no longer pending. Gate output does not replace `{prev.*}`.

Retries use the existing per-edge budget (`--max-retries`, then `max_retries`, then 3), shared with other self-retries on that step. With no prior self-retries, at most `1 + max_retries` gate attempts run; zero means the first failure exhausts the budget. On exhaustion, `if.fail: <step>` jumps to that step; otherwise cruise stops with `QualityGateExhausted`. The mapping `if.fail: {retry: true}` does not add another gate retry budget.

Gates are allowed on prompt and `prompt_file` steps, including individual steps inside a group. They are rejected on parallel parents and children, `after-pr` steps, group or `workflow_call` call sites, and non-prompt steps. Gate commands execute shell commands, just like ordinary `command` steps; only use trusted workflow configurations.

### Step Groups

Steps can be grouped to coordinate retry loops across multiple steps. A group retries all its member steps together when the `if: file-changed` condition triggers.

Groups can define their steps inline and are invoked from the main `steps` section with `group: <name>`:

```yaml
max_retries: 4

groups:
  review:
    if:
      file-changed: test    # if any step in the group changes files, jump to `test`; sequencing then re-enters the group
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
- When the group's `if: file-changed` condition triggers, execution jumps to the configured target step (`test` above); normal sequencing then re-enters the group and its steps re-run.
- A call-site step (e.g. `review-pass: group: review`) cannot have its own `if:` condition.
- A group call site cannot set `allow_commit: true`; set it on the inner prompt step instead.
- A group call site cannot set `output_file` either; a dedicated error points you at the inner prompt step.
- A group call site cannot set `computer_use`; set it on the inner prompt step instead.
- A step inside a group definition cannot use `workflow_call:`; expand the called workflow at the top level instead.
- A step inside a group definition cannot have a `flag:`; put the flag on the group call site instead.

### Workflow Composition (`workflow_call`)

A step can delegate to another workflow config file by setting `workflow_call` instead of `prompt`, `prompt_file`, `command`, `option`, or `parallel`. The called workflow's steps are inlined into the parent at the call site, with each step ID prefixed by the call-site name (e.g. `shared-review/simplify`).

```yaml
steps:
  build:
    command: cargo build

  shared-review:
    workflow_call: ./workflows/review.yaml    # local relative path

  deploy:
    command: cargo publish
```

The referenced file is a regular cruise config. Its top-level execution settings (`command`, `sdk`, `model`, `env`, `computer_use`, etc.) are ignored -- only its `steps` are imported. The parent's settings apply to the expanded steps, while per-step overrides inside the called workflow are preserved.

#### Required string inputs

A complete callee, `workflows/review.yaml`:

```yaml
inputs: [review_prompt, label]
steps:
  review:
    prompt: "{inputs.review_prompt}"
  announce:
    env:
      REVIEW_LABEL: "{inputs.label}"
    command: 'printf "%s\n" "$REVIEW_LABEL"'
```

Its complete caller, `cruise.yaml`:

```yaml
create_pr: false
steps:
  shared-review:
    workflow_call: ./workflows/review.yaml
    with:
      review_prompt: "Review the changes for {input}, using {plan}."
      label: "review"
```

`inputs` is an optional list of unique names matching `[A-Za-z_][A-Za-z0-9_]*`.
Every declared input is required. `with` is a string-valued mapping: empty strings
are valid, but numbers, booleans, arrays, and objects are not converted to strings.
Missing or extra arguments and undeclared input references fail before execution.
Omitting `with` or setting it to `null` passes no arguments. Even an empty mapping
is allowed only on a `workflow_call` step. A directly loaded root cannot declare
nonempty `inputs`, and `inputs` is forbidden in user/project `default.yaml`.
There are no input defaults, optional inputs, or CLI input arguments.

`{inputs.name}` binds at config-load time in prompt text (including loaded
`prompt_file` contents), instruction, command strings/array elements, env values,
and nested call `with` values, including parallel prompt/command children.
It does not parameterize paths, step names, transitions, flags, or models.
`prompt_file` paths resolve relative to their declaring source before binding.
Each callee has a lexical scope: parents and siblings do not implicitly share
inputs. Forward explicitly, for example `with: {label: "{inputs.label}"}` on a
nested call whose callee declares `label`.

Inserted values are not rescanned in the same binding pass. Runtime tokens such
as `{input}`, `{plan}`, `{prev.*}`, `{pr.*}`, and `{file:...}` remain for normal
execution-time evaluation. Escapes are preserved at load time: `{{inputs.label}}`
is not an input lookup and becomes the literal `{inputs.label}` at runtime.
The existing `{{...}}` and `}}` brace rules still apply. Inputs are template
fragments, not a secret store or an automatic shell-quoting mechanism.

#### Return labels

A called workflow can declare named exits, leaving the caller to choose their
destinations. For example, `workflows/review-gate.yaml`:

```yaml
returns: [approved, needs_fix]
steps:
  review:
    command: "cargo test"
  decide:
    option:
      - selector: Approve
        return: approved
      - selector: Request changes
        return: needs_fix
```

The caller maps every declared label to a step in its own step map:

```yaml
steps:
  gate:
    workflow_call: ./workflows/review-gate.yaml
    on_return: {approved: publish, needs_fix: fix}
  fix:
    prompt: "Fix the review findings in {plan}"
  publish:
    command: "git push"
```

`returns` is a unique list of labels matching `[A-Za-z_][A-Za-z0-9_-]*`.
Every declared label must be used by a callee return and mapped by the caller,
with no extra mappings. `on_return` belongs only on a `workflow_call` and null
means omission. Destinations must exist in the same parent step map and cannot
be workflow call sites. A callee's top-level step may use `return` instead of
`next`; on an option step put `return` on individual options instead. A return
cannot be combined with `next` on the same step or option, or with
`if.file-changed` on the return-bearing step. Returns are not allowed on group
or parallel children, call sites, or after-PR steps. Directly loaded roots cannot
declare nonempty `returns` or use return routes, and `returns` is forbidden in
user/project `default.yaml`.

Routes are converted into explicit `next` edges at load time. Normal completion
without a return retains the call-site `next`, or otherwise continues to the
next parent step. A skipped return-bearing step does not return: it continues
in declaration order, even if the call site has `next`. A call-site `next` is
invalid when the callee's final step has a step-level return, but is allowed
when returns are on options. Nested calls must map labels at each scope; labels
do not propagate outward. Routes may loop to earlier parent steps, subject to
the existing graph validation requiring a normal exit.

#### Supported sources

| Source | Example |
|--------|---------|
| Local relative path | `workflow_call: ./workflows/review.yaml` |
| GitHub blob URL | `workflow_call: https://github.com/org/repo/blob/main/workflows/review.yaml` |
| GitHub raw URL | `workflow_call: https://raw.githubusercontent.com/org/repo/main/workflows/review.yaml` |

GitHub workflows are fetched via `gh api` at config-load time. Relative paths inside a GitHub-hosted workflow resolve from the remote directory, so nested references work across repositories.

Installed built-ins resolve their shared dependencies from embedded resources,
never from a same-named local file. Loading `builtin/*.yaml` as ordinary local
files instead resolves dependencies relative to those files. Inline YAML uses
its supplied base directory for local dependencies, not the embedded registry.
The shared after-PR resource is internal and is not a catalog entry or a new
public `workflow_call` source syntax.

The six general-purpose built-ins call the same six-step after-PR prelude in order:
`pr-ready` → `sync-base` → `resolve-conflict` → `push-after-sync` → `wait-ci` →
`fix-ci-error`. Each root supplies its workflow-specific conflict and CI prompts
through `with`. The `follow-up` call's `flag: [after_pr, merge]` gates all six
steps; the callee has no flags. New sessions use IDs such as
`follow-up/wait-ci`, with internal transitions prefixed to that same call.
Native `squash-merge` stays the final direct root after-PR step, gated only by
`merge`. Both flags remain off by default, and `research` retains
`create_pr: false`.

`cruise workflow eject <name> [--to user|project]` writes a self-contained,
editable workflow with calls and file-backed prompts materialized. It preserves
root field omission and does not bake in host models, environment overrides, or
user/project defaults. Existing files are never overwritten. Installed packages
and new builtin/repository/inline session snapshots likewise retain resolved
content without `inputs`, `with`, `workflow_call`, or `prompt_file` metadata.
Existing snapshots and saved step IDs are not migrated. File-backed sessions
retain their existing live-reload behavior.

#### Call-site fields

A `workflow_call` step is a pure delegation point. Only `with`, `on_return`, `skip`, `when`, `next`, and `flag` may be set alongside `workflow_call`:

- `skip` and `when` are applied to the **first** expanded step.
- `next` is applied to the **last** expanded step (when it has no explicit `next` of its own).
- `flag` is applied to every expanded step. A called workflow's own steps cannot use `flag:`.

All other step fields (`prompt`, `prompt_file`, `command`, `model`, `instruction`, `plan`, `option`, `if`, `timeout`, `env`, `output_file`, `group`, `parallel`, `computer_use`, `permission`, `disallowed_tools`) and `allow_commit: true` are rejected at validation time, and the error names every offending field. An explicit `allow_commit: false` is equivalent to omission.

#### Nesting and cycle detection

Workflow calls can be nested: a called workflow may itself contain `workflow_call` steps. Step IDs accumulate prefixes (`outer/inner/step`). Circular references (A calls B, B calls A) are detected and rejected. Groups and `workflow_call` do not mix in either direction: groups inside called workflows are not supported, and a step inside a group definition cannot itself be a `workflow_call`.

```yaml
# parent.yaml -> nested/outer.yaml -> inner/leaf.yaml
# Results in step IDs: outer-call/leaf-call/leaf
steps:
  outer-call:
    workflow_call: ./nested/outer.yaml
```

### Variable Reference

Commit and PR templates use a separate [allowed vocabulary](#commit-and-pr-templates), which does not include `{plan}`. The `{plan}` below remains a session plan file path.

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from CLI argument or stdin; for `--skip-planning` / input-as-plan sessions (`input_as_plan: true`) it resolves to the current `plan.md` content |
| `{prev.output}` | Previous prompt output, or a parallel block's JSON results keyed by child name |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Previous command/prompt stderr, or a parallel block's combined stderr and execution errors prefixed by child name |
| `{prev.success}` | Previous command success, or parallel block success (`true`/`false`; skipped children count as successful) |
| `{plan}` | Session plan file path (set automatically by `cruise run`) |
| `{file:name}` | UTF-8 contents of a relative session artifact named `name`; prompt fields only, loaded immediately before execution |
| `{plan.language}` | Effective language used for built-in planning prompts (from `CRUISE_LANGUAGE_PLAN`, `languages.plan`, the legacy field, locale inference, or the default) |
| `{pr.number}` | Pull request number, available after a PR has been created |
| `{pr.url}` | Pull request URL, available after a PR has been created |
| `{pr.language}` | Effective language used for PR title/body generation (from `CRUISE_LANGUAGE_PR`, `languages.pr`, the legacy field, locale inference, or the default) |

> **Note:** `{model}` is **not** a template variable -- it is a special placeholder resolved only within the top-level `command` array. It is not available inside `prompt`, `prompt_file`, `instruction`, or `command` step fields.

Literal braces are escaped Rust-`format!`-style: `{{` -> `{` and `}}` -> `}` (e.g. `"{{input}}"` resolves to the literal string `"{input}"`, not a lookup of `input`). An unclosed `{`, a lone `}`, or an empty `{}` is a template syntax error, as is referencing an undefined variable.

## Workspace Mode

Execute now from `cruise plan` or `cruise "task"` selects flags before skip choices in the approval menu, unless `-f/--flag` was supplied, and forwards those values without a second execution-time prompt. Fresh single-session CLI execution also offers the [workflow flag selector](#cruise-run) when the config declares flags and stdin is a TTY, unless that invocation supplies `-f/--flag`. This applies to later `cruise run [id]` after approval or background planning, and Run from `cruise list`. Suppression is per invocation: `cruise plan -f merge "task"` followed by Approve and later `cruise run <id>` without `-f` still prompts, with saved `merge` preselected so Enter preserves it. Resume, dry-run, batch, and non-TTY execution do not ask again.

When `cruise run` starts a new session, it prompts you to choose a workspace mode:

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| Mode | Description |
|------|-------------|
| **Worktree** (default) | Creates an isolated git worktree at `$XDG_DATA_HOME/cruise/worktrees/<session-id>/` (default: `~/.local/share/cruise/worktrees/<session-id>/`). A new branch `cruise/<session-id>-<sanitized-input>` is checked out from HEAD or the configured `base_branch` as described below. Requires `gh` CLI for PR creation (`glab` for GitLab repositories). |
| **Current branch** | Executes directly in the current repository on the active branch. No worktree or PR is created automatically, and no automatic merge runs. If `after_pr` or `merge` is opted in, its unavailable after-PR actions are skipped with a reason. |

With `--all`, worktree mode is always used; a single `cruise run` keeps the workspace mode already recorded on the session, and in non-interactive environments (piped stdin) fresh sessions default to worktree mode. Sessions created with `--repo` (or the WebUI repository picker) are always pinned to Worktree mode — the prompt is skipped and current-branch mode is not available, since a PR is the only way the work leaves the temporary clone.

### Current-branch mode constraints

- For a fresh `cruise run` current-branch session, requires a clean working tree (no uncommitted changes to tracked files; untracked files are ignored). `cruise exec` and `force_exec` sessions are allowed to start dirty, with a warning; cruise does not stash, commit, or reset existing changes.
- Requires an attached branch (not detached HEAD).
- On resume, the active branch must match the branch recorded at the start of the session.

### Worktree isolation

- For PR-backed sessions, the worktree is retained until the PR is closed or merged; run `cruise clean` to delete it. Eligible sessions without a PR can instead use local merge cleanup.
- Set `cleanup_after_pr: true` in the config (or pass `--cleanup-after-pr` at runtime) to automatically delete the local worktree and branch immediately after the PR is created. Use `--no-cleanup-after-pr` to override the config setting and keep them.
- An explicit **Merge locally & cleanup** also removes the eligible local worktree and branch after the squash commit, independently of `cleanup_after_pr`.

New worktrees start at the repository's current HEAD unless `base_branch` is set in user or project `default.yaml`. With a base configured, Cruise prefers the local branch, then `origin/<base>`; with `auto_fetch: true`, it fetches only that branch from `origin` and prefers the updated remote-tracking ref. A failed fetch warns and falls back to local refs; a missing base is an error. The configured base is saved with the session and used as the PR or MR target; without it, the forge CLI chooses its default. Existing worktrees are reused without fetching or resolving the base again.

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

Conditional cycles are allowed when a normal exit is possible. Execution preflight rejects a reachable cycle only when no normal exit is reachable from the actual start position, considering configured and user-selected skips. A branch that may enter an exitless cycle produces a warning, not a blanket rejection. Runtime edge budgets still apply.

## Config Hot-Reload

During `cruise run`, the config file is checked for changes between each step. If the file has been modified (detected via mtime), the updated config is reloaded automatically -- no restart required. This allows you to adjust prompts, add steps, or tweak settings while a session is running.

> **Note:** Hot-reload only applies when the session was started from an external config file, not an embedded built-in workflow. The current step must still exist in the new config for the reload to take effect. Newly declared flags absent from the session's saved values are off.

## Rate Limit Retry

Without an SDK fallback policy, cruise retries the same model with exponential
backoff when command stderr contains rate-limit wording or a standalone `429`
token (not incidental `429` substrings). SDK backends also treat `usage limit`,
`session limit`, and `overloaded` as limits:

- Initial delay: 2 seconds
- Maximum delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)

The SDK backends additionally support a fallback policy through an optional
`retry:` block or a workflow-level model array with fallback entries. An
explicit `retry:` block can configure the policy:

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
      - provider-a/model-a
      - provider-b/model-b
    "provider-a/*":
      - provider-c/*
```

With an SDK retry policy, whether declared by `retry:` or implied by a model array with fallback entries, a 429 switches to the next usable fallback immediately, without a `Retry-After` wait, and is retried on the same model only when none is left; HTTP 5xx, HTTP 4xx other than 429, 401, 403, 407, and 408, and network failures become retryable too, and they switch to the next chain entry immediately when `--rate-limit-retries` is above zero, a usable fallback exists, and no visible text was streamed, with a fresh budget and a fresh session. Such a 4xx is answered by a switch only: the same request is never resent to the same model, and a 400 counts even when its message reads like a permanent error (`invalid request`, `context length`, `max_tokens`), because another model may accept the very same request. A message that instead names the model as absent -- `model_not_supported`, `model_not_found`, `model not found`, `model not supported`, `model is not supported`, `unknown model`, `no such model`, `model does not exist`, `not a valid model`, read from the provider's own error rather than the child-process stderr appended after it -- is classified as a missing model instead: it still only switches, but the reference rather than the request is at fault, so `invalid_request_error: model not found` is a switch rather than a permanent failure and the abandoned model is cooled down. When no usable chain entry is left, the original 4xx surfaces unchanged. The excluded statuses keep their own handling: 401, 403, and 407 are authentication or permission failures that no other model can answer, so they fail the step immediately with the provider's own error rather than hiding it behind a walk through the chain, and a 408 Request Timeout is classified as a network failure, so it follows the 5xx/network path instead: it switches to a usable fallback first and is resent to the same model only when no usable fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0`. Classification reads rate-limit wording first, 5xx and the network markers next, and the missing-model wordings before any bare 4xx-looking number, so a `429` or a `503` keeps its own class even when the surrounding text -- a stack trace or a URL appended from the CLI's stderr, say -- also contains something that looks like a 4xx. A failure with no recognizable status code (`unknown option '--effort'`, say) stays permanent and fails the step immediately; a number is read as an HTTP status only when it is a standalone three-digit number with one of `http`, `status`, `code`, `error`, `returned`, or `upstream` within the preceding 24 bytes, and source locations (`src/lib.rs:404:17`) and URL ports (`https://host:443/path`) are never treated as statuses. A `provider/*` chain entry keeps the failing model id and swaps only the provider. `--rate-limit-retries 0` disables retrying, so a rate limit, a 5xx, or a 4xx fails the step with no model switch; only a model reference the backend refuses outright still moves to the next chain entry, since nothing was sent and there is nothing to replay. A model skipped because of a 429, a 5xx, or a network failure -- a 408 included -- remains skipped for the next 30 minutes in this process (in-memory state, not persisted across processes), as does one the provider reported as missing, so later turns and later steps stop re-selecting a model it does not have; only a model left behind by a 4xx client-error switch escapes the cooldown, because a client error says something about the request, not about the model's health. A turn that already streamed visible text is never retried on another model. For a 429 with no usable fallback, the same-model retry can be bypassed when its computed or server-requested delay exceeds `retry.max_delay_ms`.
Recognized network failures include request-send transport errors; OpenCode setup timeouts before prompt acceptance also follow this retry/fallback path.
OpenCode local V2 API errors before prompt submission that classify as rate-limit, client, or server failures remain behind the retry barrier; only network-class setup and transport failures bypass it for fallback.

Startup exclusions declared by `disabled_during` remain independent of the response-retry budget, including when `--rate-limit-retries 0` is set.

Declaring `retry:` at all changes the no-`retry:` behavior, `model_fallback: false` and empty chains included: those only switch model switching off for scalar model configurations, while the 4xx/5xx/network classification and the `base_delay_ms`/8s-ceiling backoff schedule stay in force. Workflow-level model arrays with fallback entries always enable switching and generate their fallback chains. Omit the block entirely, and use scalar or unset model fields, to keep the historical behavior -- limit errors only, same model, 2s doubling to a 60s cap. The `command:` backend always uses the historical behavior and ignores `retry:`.

## Stale Session Detection

When `cruise list`, the TUI (`cruise`), or the WebUI session detail view loads a session, a session in the `Running` phase is checked for liveness. If the runner process (identified by PID and start time) is no longer alive, the session is automatically transitioned to the `Suspended` phase. This prevents sessions from being stuck in `Running` indefinitely after a crash or forced termination.

Suspended sessions can be resumed from `cruise list` or the TUI, or reset to Planned. The `run --all` command also picks up Suspended sessions alongside Planned ones.

Execution uses a fixed-size directed **Graph**, with one stable node per compiled step (a parallel block remains one unit). Increasing `max_retries` does not expand the graph. The versioned `dag.json` checkpoint is authoritative for the next step, accepted edge traversals, budgeted traversals, group-call retry counts, `{prev.*}` variables and file snapshots. Checkpoints are written atomically before the first step and after accepted transitions, normal completion and loop protection. A persistence failure stops execution before the next side effect. A mid-step interruption retains the pre-step checkpoint, so external command effects are not exactly-once.

Normal resume and prompt/model reloads retain counters. `--max-retries` applies a new ceiling to the saved counts, including when lowered. A rejected transition remains pending and cannot replenish its budget by resuming. Changing the resume position retains counters. Only an explicit restart creates a new execution and archives the previous checkpoint. Removing the current step during hot reload keeps the old workflow rather than silently returning to its start.

The compatibility filename `dag.json` and IPC name `get_session_dag` remain. Unversioned legacy graphs are viewable but cannot resume with guessed counters. Missing, corrupt and unknown-version checkpoints stop resume without deleting data. Explicitly restart the session to run from the beginning. Main and after-pr use separate graphs and counters. This does not add an independently resumable after-pr phase.

## Parallel Session Execution

Run All can automatically retry eligible failures from the failed step within the same worker. Set the non-negative limit in WebUI settings, with `e` in the TUI Run All view, or with `cruise config --set-auto-requeue-max-attempts <N>`. It defaults to `0`, disabling retries. Unclassified command errors and eligible provider errors and no-file-changes errors may retry, while cancellation, configuration, authentication and authorization errors do not. An attempt's normal failure notification still appears. Completed runs with failed steps are not reclassified as failures.

The persisted per-session count is a lifetime budget across batches. Manual Run or Retry does not reset it, but Reset to Planned does. Failed sessions are not picked up at batch start, single-session runs and exec sessions are not automatically retried, and lowering the limit does not erase the count. Counts appear in session details, CLI list JSON and batch summaries.

The WebUI, TUI, and CLI support running multiple sessions concurrently during `run --all`.

- **WebUI/TUI**: the parallelism level is controlled by `run_all_parallelism` in the selected application settings file (`config.json` or `config.yaml` under `$XDG_CONFIG_HOME/cruise/`, configurable via `cruise config --set-parallelism <N>`, default: `1`).
- **CLI**: pass `cruise run --all --parallelism <N>` for a one-run override (default: `1`, i.e. sequential). The persisted WebUI/TUI setting is neither read nor modified. In an interactive terminal, when `--dry-run` is not set, the CLI also shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output is retained in `sessions/{id}/run.log`. Non-TTY and dry-run invocations keep the normal log output and final summary.

The batch scheduler:
- Seeds from Planned and Suspended sessions.
- Launches up to `N` sessions concurrently.
- Re-scans for newly added eligible Planned or Suspended sessions every 200ms while worker slots are available, so sessions created while a batch is running are picked up automatically.
- Results are returned in scheduling order regardless of completion order.

## New Session Form Persistence

The WebUI and TUI persist two pieces of state across sessions:

- **Draft** (`$XDG_STATE_HOME/cruise/new_session_draft.json`): The current contents of the New Session form (task description, config path—including the `__builtin__` sentinel for the built-in `tdd` workflow—, working directory, repository, skipped steps). The TUI saves changes automatically. The WebUI saves them when **Save draft** is clicked. The draft is restored when the form is reopened, so unsent input is preserved after that save.
- **History** (`$XDG_STATE_HOME/cruise/history.json`): A log of past New Session selections. Used to pre-populate the step skip selector with the most recent choices for each config file and to recall previous working directory / config combinations.

## GitHub Actions

Mention `@cruise` on a GitHub Issue to drive cruise inside GitHub Actions. The action installs the `jcode` CLI and provisions its credentials, so cruise's default `sdk: jcode` backend works with no configuration; it no longer forces a backend, so a repository config that sets `command:`, `sdk: claude`, or `sdk: opencode` still wins and has to bring its own CLI. There is no PR mode -- comments on pull requests are ignored.

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

## OpenTelemetry

Inspired by [TAKT's observability support](https://github.com/nrslib/takt/blob/main/docs/observability.md), Cruise can export workflow traces, execution metrics, and bounded failed-command stderr logs over OTLP HTTP/protobuf. Telemetry is off by default; no workflow YAML changes are needed.

```sh
CRUISE_OBSERVABILITY_ENABLED=true \
OTEL_SERVICE_NAME=cruise \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
cruise exec --config cruise.yaml
```

- `CRUISE_OBSERVABILITY_ENABLED=true` opts in (case-insensitive). `OTEL_SDK_DISABLED=true` overrides it. Disabled telemetry creates no exporter or network client.
- Without an endpoint override, enabled telemetry uses `http://localhost:4318`. The base endpoint gets `/v1/traces`, `/v1/metrics`, and `/v1/logs` appended. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`, and `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` override each URL verbatim, including its signal path.
- `OTEL_EXPORTER_OTLP_HEADERS` supplies comma-separated, URL-encoded `key=value` authentication headers; use an environment/secret manager, not workflow YAML. `OTEL_EXPORTER_OTLP_TIMEOUT` sets the request timeout in milliseconds (SDK default: 10,000). Signal-specific `OTEL_EXPORTER_OTLP_TRACES_*` / `OTEL_EXPORTER_OTLP_METRICS_*` / `OTEL_EXPORTER_OTLP_LOGS_*` values take precedence.
- Only HTTP/protobuf is supported; gRPC and HTTP/JSON protocol selections fall back to HTTP/protobuf. Optional compression: `OTEL_EXPORTER_OTLP_COMPRESSION=gzip`.
- Resources default to `service.name=cruise` and the package's `service.version`. `OTEL_RESOURCE_ATTRIBUTES` adds or overrides resource attributes; `OTEL_SERVICE_NAME` takes precedence for the service name.

Once a session ID is allocated, `cruise.execution` spans preparation through finalization, including repository cloning before session persistence. CLI plan/exec/run, background planning workers, and the shared Application API used by TUI/WebUI are covered; parallel sessions have separate execution roots. Root attributes include `cruise.execution.kind` and `cruise.session.id`; `cruise.workflow.kind` and `cruise.workflow.name` are set when configuration is available. Workflow names are built-in names or final file names, never full paths. A preparation failure can leave a trace with an allocated ID but no persisted session.

| Span | Scope |
|---|---|
| `cruise.execution` | A session operation, correlated by session ID |
| `cruise.phase` | Actual `prepare`, `workspace`, `planning`, `workflow`, `pull_request`, or `finalize` work |
| `cruise.workflow` | Engine execution under the workflow phase |
| `cruise.step` | One step; parallel children are siblings under their parallel step |
| `cruise.agent` | A prompt backend call, including its retry lifecycle |
| `cruise.attempt` | One backend or shell attempt, with a 1-based `cruise.retry.attempt` and the selected backend/model |

Phases are emitted only when their work runs: skipped planning or disabled/unneeded PR creation does not produce a successful placeholder phase. `cruise.retry` span events record backoff/fallback actions, fixed retry reasons, attempt numbers, and model switches. SDK fallback and the independent command/prompt rate-limit retry loops are instrumented without changing retry policy.

GitHub native squash-merge refusals for unconfirmed mergeability include the observed `mergeable` and `mergeStateStatus` values in the saved session failure, even when telemetry is disabled. When enabled, telemetry also records these values as `cruise.github.mergeable` and `cruise.github.merge_state_status` attributes on the `squash_merge` step span.

The same span includes the final GitHub readiness observation:

| Attribute | Value |
|---|---|
| `cruise.github.waiting_for` | `CI registration`, `current-head CI registration`, `current-head CI checks`, or `GitHub mergeability`; `none` when no pending reason was observed or readiness resolved |
| `cruise.github.ci_state` | Observed `statusCheckRollup.state`; `unobserved` before a snapshot, or `unregistered` when absent |
| `cruise.github.ci_head_matches` | Boolean comparing the observed CI rollup commit to the expected HEAD; `false` also covers a missing or not-yet-observed CI commit |

The latest observation is retained locally and written once when the merge operation returns or its future is dropped, including outer step timeouts and cancellation. This avoids duplicate polling attributes exhausting the SDK's span attribute limit. A ready or already-merged result clears a previous wait reason; initial `none` / `unobserved` / `false` values do not prove that CI passed. Commit IDs are not exported.

When a GitHub squash merge command completes unsuccessfully and the pull request is confirmed still open at the expected head, the saved failure includes a fixed refusal reason and any available HTTP status and command exit code. The same span records `cruise.github.merge_refusal_reason` and, when available, numeric `cruise.github.merge_http_status` and `cruise.github.merge_exit_code`. Reasons are `merge_not_performed`, `head_changed`, `base_changed`, `squash_disabled`, `merge_in_progress`, or `unclassified`. The last means the response did not match a recognized canonical message, not that a cause was inferred from the status. Raw provider messages and response headers are not saved or exported. These attributes are not metric labels.

Skipped steps, including parallel children, do not emit step spans or operation observations. Step outcomes include postcondition failures such as `if.no-file-changes`; a later recovery can succeed without erasing the failed step. Cancelling the approval menu is recorded as cancelled while preserving the successful CLI exit; interrupting active planning or title generation still returns an error after draining that work. Choosing Execute now finishes planning before starting a separate execution root.

Outcomes include `success`, `failed`, `error`, `cancelled`, `timeout`, and `paused`. Non-success operations have error status and a payload-free `error.type`, such as `command_exit`, `rate_limit`, `timeout`, or `cancelled`; the span status description equals `error.type`. Existing backend retry classifications distinguish network, provider, and model failures. Recovered retries leave failed attempts visible without marking the final successful execution as failed. An Esc-paused exec stays resumable and exits successfully at the CLI, while its root outcome remains `paused`.

Failed shell attempts include numeric `cruise.command.exit_code` or, on Unix when terminated by a signal, `cruise.command.signal`. The terminal failed result also annotates its step span; a recovered rate-limit attempt does not annotate the successful step.

Completed unsuccessful shell results also record `cruise.command.stderr_present` (whether captured stderr is nonempty, including whitespace) and `cruise.ci.diagnostic_status` on those spans:

| Diagnostic status | Meaning |
|---|---|
| `valid` | A bounded, allowlisted `CRUISE_CI_DIAGNOSTIC_V2` or `CRUISE_CI_DIAGNOSTIC_V4` marker was accepted |
| `invalid` | A V2 or V4 marker candidate was present, but none passed validation |
| `unsupported` | Only unsupported numeric marker versions were recognized |
| `missing` | No CI marker candidate was recognized in captured stderr |

The last valid V2 or V4 marker wins even if later lines are malformed or unsupported; otherwise `invalid` takes precedence over `unsupported`. Candidates start with the marker token, allowing leading ASCII whitespace for detection only. Both versions are limited to 16,384 bytes and contain allowlisted scalar fields; V2 accepts no trailing fields, while V4 requires a strictly validated JSON detail payload. Unknown versions, unknown JSON fields, and malformed values are never exported; invalid optional references are omitted while retaining valid check names/states and marking details incomplete. `missing` can also describe an unrelated command or an old workflow without diagnostic markers: these statuses describe captured output, not the underlying CI failure. Spawn errors, timeouts, and cancellation without a completed shell result do not add these fields. Both attributes are trace-only, not metric labels.

Jcode failures add fixed diagnostics to the affected `cruise.attempt` span:

| Attribute | Meaning |
|---|---|
| `cruise.agent.failure_stage` | The bounded stage where the failure was detected, such as SDK launch, API event handling, or turn-stop handling |
| `cruise.agent.failure_reason` | A fixed allowlisted reason for typed SDK/API/turn failures; unknown cases remain `unknown` rather than inferring HTTP status, authentication, or provider-internal causes |

These attributes are trace-only, not metric labels. Raw SDK error messages, stop messages, prompts, and output are not exported.

The built-in `follow-up/wait-ci` command additionally reports bounded, allowlisted diagnostics:

| Attribute | Meaning |
|---|---|
| `cruise.ci.provider` | `github` or `gitlab` |
| `cruise.ci.failure_reason` | Fixed cause: `local_head_error`, `repository_query_error`, `repository_invalid`, `api_error`, `graphql_error`, `pr_head_missing`, `head_mismatch`, `rollup_head_missing`, `malformed_rollup`, `invalid_ci_state`, `invalid_check_count`, `ci_failed`, `unknown_state`, `clock_error`, or `timeout` |
| `cruise.ci.state` | Last observed allowlisted CI state, a registration state, or `unknown` / `unobserved`; never arbitrary provider text |
| `cruise.ci.head_matches` | Whether the observed GitHub PR head matches local HEAD; omitted when unobserved. A matching PR head does not prove the rollup is current |
| `cruise.ci.check_count` | Observed numeric check count when available |
| `cruise.ci.http_status` | Numeric HTTP status from 100 through 599 when observed; omitted when unknown |
| `cruise.ci.failed_checks` | For `ci_failed`, a compact JSON array of failed check/job descriptors containing `name`, `state`, and available `run_id`, `job_id`, and `url` references |
| `cruise.ci.failed_checks_truncated` | For `ci_failed`, whether details are incomplete, invalid references/entries were omitted, or names/references were normalized/shortened |

Failure details include at most 10 checks, with names limited to 128 Unicode characters after removing controls and trimming whitespace. GitHub collects completed failing CheckRun conclusions and failing StatusContext states, fetching and paginating detail responses only after a failed rollup. CheckRun references include the workflow run ID when available; an Actions job ID is extracted only when the details URL's repository and run ID match the observed repository/workflow run. StatusContext entries do not invent run/job IDs. GitLab requires the detail response to match the observed failed pipeline before using its pipeline ID, failed job IDs, URLs, or local trace fetching. Reference IDs are positive decimal strings of at most 20 digits. URLs are HTTP(S), at most 1,024 bytes, with userinfo, query strings, and fragments removed. Missing or invalid details preserve `ci_failed` and set the truncation flag; the producer drops optional references if needed to keep the complete marker bounded. These descriptors are trace-only, never metric labels. Full CI job logs remain on the provider; telemetry records references, not copies of those logs.

The built-in `follow-up/sync-base` command exports `cruise.sync_base.failure_reason` with one of `base_ref_query_error`, `base_ref_invalid`, `fetch_failed`, `merge_conflict`, or `merge_failed`. CI and sync-base span diagnostics come from strict bounded stderr markers; malformed marker fields and raw provider responses are not attached to spans. All CI/sync-base machine-marker lines, including invalid or unsupported ones, are omitted from stderr logs. Saved workflow snapshots and previously ejected workflows retain their old CI command. V2 diagnostics remain readable without replacing those commands; for `ci_failed`, unavailable check descriptors are exported as an empty array with `cruise.ci.failed_checks_truncated=true`. V3 remains unsupported. Create a new snapshot or update an ejected workflow to obtain V4 check details; existing exported spans are not changed.

Errors classified as `loop_protection` annotate each result-based enclosing span with `cruise.loop.from`, `cruise.loop.to`, `cruise.loop.max_retries`, `cruise.loop.traversals` (accepted budgeted traversals of that edge, before the refused transition), and `cruise.loop.skipped_step_count`. Step IDs follow the existing step-name privacy policy; full edge tables, skipped-step lists, and exception messages are not exported. The retry budget and workflow behavior are unchanged.

Metrics are `cruise.operation.count` and `cruise.operation.duration` (seconds), labeled only by `cruise.operation` and `cruise.outcome`, plus `cruise.retry.count`, labeled only by `cruise.retry.action` and `cruise.retry.reason`. Session/workflow/model identifiers are trace-only, not metric labels. Token usage and cost are not exported to OpenTelemetry; they are recorded locally as described under `cruise usage`.

Prompt/input text, agent output, command bodies, working paths, and exception messages are not automatically attached to spans or metrics. Session IDs, workflow file names, step/model names, CI check/job names and references, and explicitly supplied resource attributes are exported; keep secrets out of those values.

Each completed unsuccessful shell attempt can additionally emit one ERROR OTLP log named `cruise.command.stderr`, correlated to that attempt's native `trace_id` and `span_id`. The final step does not duplicate it, successful-command stderr is not exported, and empty/whitespace-only sanitized output emits no record. Spawn errors, timeouts, and cancellation without a completed shell result do not produce stderr logs.

Log bodies redact recognized credential headers and assignments, auth schemes, known token formats, private-key blocks, and URL userinfo/query/fragment values; ANSI/control sequences are normalized before matching. Redaction happens before a UTF-8-safe head/tail excerpt capped at 8,192 bytes. `cruise.command.stderr_truncated` records whether sanitized output exceeded that cap, not whether credentials were redacted.

Redaction is pattern-based, not a guarantee that arbitrary secrets or personal data are removed. Captured stderr can still contain sensitive paths, messages, or business data. Before enabling telemetry for real workloads, restrict backend log access to authorized operators and configure short retention. Cruise does not provision backend RBAC or retention policies; these remain deployment responsibilities and are not changed by enabling logs.

Providers flush on normal CLI completion, including workflow errors. Collector/export failures do not change the workflow result, though shutdown can wait for export timeouts. Invalid exporter initialization settings stop startup with a redacted diagnostic. Forced termination cannot guarantee a flush.

Backend acceptance is not a persistence guarantee. Local E2E investigation found that GreptimeDB retained all histogram observations when SDK exports were posted separately, but lost earlier histogram points when the same exports were combined into one OTLP request with multiple `ResourceMetrics`. The SDK payloads contained all observations and the gateway returned HTTP 200. This downstream ingestion issue is not worked around by duplicating application exports.

## Development build

To test the launcher locally, build both binaries. The `cruise` launcher starts the adjacent `cruise-runtime`, so both are needed:

```sh
cargo build --bin cruise --bin cruise-runtime
```

`cargo test --all-features --test workflow_defaults_cli` also checks CLI startup with a 1 MiB main-stack budget on Unix; Windows CI exercises the native main-thread limit.

## Support

If you find cruise useful, consider [sponsoring smartcrabai](https://github.com/sponsors/smartcrabai)
to support its development and maintenance.

## License

MIT
