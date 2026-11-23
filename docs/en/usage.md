**Languages:** [English](../en/usage.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/usage.md) · [简体中文](../zh/usage.md) · [Index](README.md)

<a id="webui"></a>
## Daemon and WebUI

Cruise runs one resident daemon per user and state directory. Session commands are clients of that daemon: `plan`, `--plan`, `draft`, `run`, `run --all`, `exec`, `list`, `tell`, `clean`, and `diff`. They start the daemon on demand when none is running. `cruise webui` also ensures it is running, prints the WebUI login URL, and optionally opens it:

```sh
cruise webui
cruise webui --open
```

Manage the daemon with `cruise daemon`:

```sh
cruise daemon start
cruise daemon status --json
cruise daemon restart
cruise daemon stop --force
cruise daemon serve
```

`serve` runs in the foreground for containers or debugging. `start`, `restart`, and `serve` accept `--host`, `--port`, `--webui-dir`, and `-o`/`--open`. The default address is `127.0.0.1:8484`. If that port is busy and no port was requested, Cruise uses an ephemeral port and warns; set `--port` or `CRUISE_DAEMON_PORT` explicitly on shared hosts. Either option set to `0` requests an ephemeral port. Open the login URL printed by `cruise webui`; it contains a single-use nonce that expires after five minutes, never the registry API token. Successful login sets an HttpOnly, SameSite=Strict `cruise_webui` cookie with a random in-memory session ID, separate from the API token and invalid after the daemon exits. The bare origin returns 401. `-o`/`--open` opens a 0600 temporary HTML redirect file, so the URL is not passed as an opener argument. Loopback binds accept only `localhost`, `127.0.0.1`, and `[::1]` Host names on any port, blocking foreign Host names used in DNS rebinding while allowing SSH tunnels. Non-loopback binds serve plain HTTP; use them only on trusted networks or behind a TLS proxy.

The daemon registry and log are in `$XDG_STATE_HOME/cruise/daemon/` (normally `~/.local/state/cruise/daemon/`): `instance.json` is mode `0600` and contains a random API token, and `daemon.log` contains daemon logs. Keep this directory private. There is no boot-time service installation and no idle auto-exit; the daemon exits when its registry record is removed. The former `cruise webui start|stop|restart|status` commands are removed; use `cruise daemon ...`.
Detached daemons check `daemon.log` every 60 seconds. Above 5 MiB, the current log replaces `daemon.log.1` and the live log is truncated; only one rotated generation is kept.

When upgrading from the v0.3.0 detached WebUI, daemon start, restart, serve, and client autostart refuse while its server is live. Run `cruise daemon stop` first; it also stops the legacy server, cancelling its active work so sessions become `Suspended` and can be resumed with `cruise run <id>`. `cruise daemon status` reports a live legacy server. Dead or stale legacy records are removed when the daemon starts.

The daemon also refuses (HTTP 409, CLI exit 1, state unchanged) to run or plan a session that another cruise process is still executing as its recorded runner, such as a v0.3.0 `cruise run` that was in progress when you upgraded. v0.3.0 background planning workers record no runner and cannot be detected, so let background plans started before the upgrade finish first.

The CLI resolves workflow configuration on the client. Its working directory and workflow-override environment variables such as `CRUISE_MODEL`, `CRUISE_PLAN_MODEL`, and `CRUISE_SDK` are sent with each request and apply only to that request. `PATH`, `GH_TOKEN`, provider credentials, `CRUISE_FORGE`, `CRUISE_SKIP_AGENT_AUTH_CHECK`, `CRUISE_DISABLE_NOTIFICATIONS`, `CRUISE_COMMIT_COAUTHOR_*`, and other process-level variables come from the daemon's environment; restart it with `cruise daemon restart` after changing them. Daemon autostart removes workflow-override variables from the daemon environment; explicit `cruise daemon start` or `serve` preserves them. WebUI sessions use the daemon's environment and default their working directory to the user's home directory (or the data directory when no home is available), independent of the daemon process's neutral working directory. herdr reporting remains client-side.

After `cruise update`, a busy daemon keeps running its old version: CLI clients refuse requests with a hint to run `cruise daemon restart` when idle, and the WebUI stays on the old version until restart. An idle daemon is restarted automatically by the next client request.

Foreground commands stream progress from the daemon. Ctrl+C cancels the operation (`run` becomes `Suspended`; planning is restored), but closing the terminal does not stop daemon work. Reattach with `cruise run <session-id>` or the WebUI. In ordinary sessions, Ask and Option prompts appear in the terminal and WebUI; the first answer wins. If a non-interactive client has a pending prompt, the session waits for an answer in the WebUI or from `cruise run <session-id>`. Manager-owned Ask and Option prompts must be answered on the repository’s Manager page or with `cruise manager answer <GOAL> <PROMPT>`, because the child session page is read-only. `cruise --plan` also runs its planning operation inside the daemon rather than a detached worker process.

All session mutations pass through the daemon, so separate CLI processes and the WebUI can safely act on sessions concurrently. Claims prevent duplicate work on the same session. Local-only commands are `config`, `usage`, `update`, `install`, `workflow *`, `docs`, `ssh`, and every `--dry-run`; they do not connect to the local daemon.

The browser needs network access to load Mermaid from jsDelivr for the Graph tab. New Manager conversation turns and proposal generation require the daemon to reach its configured SDK provider. Choose the display language in the sidebar footer on desktop or the header on mobile. The selection reloads the page.

Select **Manager** in the sidebar to open an existing Manager record, review proposals, and control goals from the browser. See [Experimental Manager records](sessions.md#experimental-manager-records).

<a id="usage"></a>
# Usage

```sh
# Create a session (plan -> approve)
cruise plan "implement the feature"

# Create a session and generate the plan in the background
cruise --plan "implement the feature"

# Interview-style planning: answer one question at a time, then the plan is written (SDK backend + TTY + `interactive_planning: true`)
cruise plan --grill "implement the feature"

# Enable applicable UI E2E for the built-in test-first workflow
cruise plan -c builtin:tdd -f e2e "task"

# Read the common UI E2E policy and available framework IDs
cruise docs e2e
# Read React Native / Expo or Tauri policy
cruise docs e2e react-native
cruise docs e2e tauri

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

# Open the interactive session selector (same as `cruise list` on a TTY)
cruise

# List and manage sessions with the CLI selector
cruise list

# Review a session's changes, or just their summary
cruise diff <SESSION_ID>
cruise diff <SESSION_ID> --stat

# Remove eligible closed/merged PR and no-PR sessions (keeps failed exec sessions)
cruise clean

# With no forwarded args, run `cruise list` on the remote project
cruise ssh devbox --cwd /srv/project

# Run remote commands and forward stdin or root options unchanged
cruise ssh devbox --cwd /srv/project -- plan "implement the feature"
echo "implement the feature" | cruise ssh devbox --cwd /srv/project -- plan
cruise ssh devbox -- list --json | jq .
cruise ssh devbox -- run
cruise ssh devbox -- --plan "implement the feature"
cruise ssh devbox -- --plan "implement the feature" --repo owner/repository

# The exact task text "docs" is reserved; use cruise plan "docs"
# Legacy: positional input without a subcommand is treated as `cruise plan`
cruise "implement the feature"
```

All notifications for workflow events **Action required**, **Plan ready**, **Completed**, and **Failed** are sent by the daemon, whichever client (CLI or WebUI) started the work. Sound and Slack webhook delivery always come from the daemon. The desktop banner comes from a connected WebUI browser tab as a browser notification when one is open; when no WebUI browser is connected, the daemon shows a native desktop banner (macOS/Linux desktop session). Sound is off by default. Delivery failures do not change workflow results or exit status. Set `CRUISE_DISABLE_NOTIFICATIONS=1` in the daemon's environment and restart it with `cruise daemon restart` to disable daemon desktop, sound, and webhook delivery; this does not disable WebUI browser notifications. See [Notifications](cli-reference.md#notifications) for settings and WebUI behavior.

<a id="herdr-integration"></a>
## herdr integration

Herdr status reporting happens client-side for CLI `cruise plan`, `cruise run`, and `cruise exec`, including Run/Resume from `cruise list`. Only `cruise list` browsing and action-selection menus are excluded; other CLI prompts, such as `cruise run` workspace selection, report `blocked`. WebUI daemon sessions do not report workflow status.

Herdr Core reports over OSC 7501 when `HC_TERMINAL_ID` is nonempty and a TTY is available; on Unix, Cruise must also be in the foreground process group. It needs no `hc` executable, RPC, or config ([Core details](https://github.com/herdrdev/herdr-core#status-comes-from-the-program)).

Classic Herdr requires `HERDR_ENV=1`, `HERDR_PANE_ID`, and `HERDR_BIN_PATH`; when fully configured, it takes precedence over Core.

Both report `working` and `blocked` with a reason. On exit, Core sends `clear`; classic reports `idle` and releases its pane. Reporting is best-effort and does not affect workflow results or exit status. Set `CRUISE_DISABLE_HERDR=1` to disable both.

From a herdr pane, use `cruise tell <session-id> "instruction"` to instruct a running session.

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
