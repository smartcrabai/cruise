---
name: cruise-cli
description: Use when running, operating, or troubleshooting Cruise's CLI and resident daemon — the YAML-driven coding-agent workflow orchestrator with the jcode SDK as its primary, default backend. Covers command selection (plan / --plan / draft / run / exec / list / manager / clean / config / ssh / daemon / webui / update), GitHub issue input, planning variants, workflow catalog and flags, bounded run-all execution, session lifecycle, the experimental Goal-driven `cruise manager` (proposals, approval, goals, child sessions, prompts), workspace modes, config resolution, daemon environment and runtime file layout. Trigger whenever the user asks how to operate Cruise sessions, Manager goals, its CLI, daemon, or WebUI.
---

cruise is a YAML-driven coding-agent workflow orchestrator with the jcode SDK as its primary and only SDK backend (`sdk: jcode`); custom CLI commands remain available through `command:`. Workflows follow **plan → approve → run (write tests → implement → test → review) → usually open a PR → after-pr automation when a PR is created**. This skill is the operator's manual — how to *drive* cruise. For writing the workflow YAML itself, see the **cruise-config** skill.

## Forges

GitHub (`gh`) and GitLab (`glab`) are supported. The forge is detected from the repo host (`github.com`, `gitlab.com`); set `CRUISE_FORGE=github|gitlab` for other hosts. `--repo` accepts `owner/repo`, `group/sub/project` (with `CRUISE_FORGE=gitlab`, host from `GITLAB_HOST`), or a full URL. On GitLab the PR is a merge request and the `@cruise run` issue-trigger toggle is unavailable.

Prompt steps accept `permission: read-only | edit | full` (default `full`; see the **cruise-config** skill). A `read-only` step that changes the workspace fails with "read-only step changed the workspace"; this is snapshot-diff detection, not a sandbox. The command backend rejects restricted modes at validation. The built-in `research` workflow and the inventory/audit/triage phases of `security` require these SDK read-only permissions; security's repair and verification phases use full permission. If neither `command:` nor `sdk:` remains set after workflow and applicable defaults are resolved, Cruise falls back to `sdk: jcode`.

Prompt `disallowed_tools` uses SDK-neutral `web`, `subagent`, `mcp`,
`mcp:<name>`, and `shell:<prefix>` denies. Step lists add to workflow defaults,
not replace them; workflow `[]` can clear inherited `default.yaml` lists.
SDK prompts without `allow_commit: true` also deny common Git ref-changing
commands, including all `git reset` forms (use `git restore --staged` to unstage).
`allow_commit: true` removes the guard and implicit Git denies only, never
explicit restrictions or permission presets. Command backends reject explicit
lists and keep only commit guards. Built-in planning/fix/ask/title/PR-description
turns stay unrestricted. These controls are not a sandbox: jcode hooks can be
disabled by `JCODE_HOOKS_DISABLED` or fail open, Claude native matching does not
normalize Git global options, and user-level MCP configuration is unaffected.
See the **cruise-config** skill for the full implicit list and backend limits.

## Generating workflow YAML

`cruise workflow generate "description" --name <name> [--user] [--config <path>]` drafts a new workflow with the backend selected by normal config resolution. It saves `./.cruise/<name>.yaml` (or the user workflows dir with `--user`), never overwrites an existing file, and does not run the result or create a session. Output must be raw YAML; each candidate passes doctor static checks with flags off and isolated defaults, with up to 3 repairs, and nothing is written on failure. Validation does not prove command safety, and the backend keeps its normal repository permissions, so review the YAML before running it.

`cruise workflow doctor [--json] [--strict] [-f NAME[=on|off]]... [TARGET]...` checks workflows without running them or creating sessions. Targets are paths, `builtin:<name>`, `__builtin__`, or unambiguous user/builtin names. No targets checks project-local workflows; `--all` checks those plus user workflows and built-ins and conflicts with targets and flags. Flags default off and apply to every target. Unreachable steps under the selected flags and potentially exitless cycles are warnings; a definite lack of normal exit is an error. JSON reports use schema version 1. Exit 0 allows warnings, exit 1 indicates errors or warnings under `--strict`, and exit 2 indicates unresolved targets, no targets, or invalid arguments. Remote workflow references may require network access; this is not a backend connectivity or command safety check.

For interactive creation or revision use `cruise workflow make --name <name> [--user] [--config <path>] [--from <selector>] [description]`. Both stdin and stdout must be terminals. `--from` provides a reference that Cruise reads without writing it. Submit requests, inspect with `/show`, and use `/accept` to write, continue editing, or cancel. Only an approved write creates the destination, without overwriting. Drafts pass the same preflight and up to 3 repairs, and are retained with the conversation log under the XDG data directory in `cruise/workflow-make/`, outside `cruise clean`. The workflow is not run and no session is created. The backend is asked to operate read-only, but command backends are not sandboxed; detected project changes, including concurrent user edits, fail the turn. Review the YAML before running it.

## Mental model

Work flows through **sessions**, each with a phase. The normal path is:

```
plan/draft  →  [AwaitingInput while an interactive question is pending]  →  AwaitingApproval  →  (approve)  →  Planned  →  run  →  Running  →  Completed  →  clean
```

- A **session** is a unit of work (one task → one plan → one run → usually one PR).
- **AwaitingInput** means SDK planning has persisted an unanswered question. Answering it resumes planning; `cruise list` offers **Generate Plan**, which regenerates the plan and moves the session to `AwaitingApproval`.
- `cruise plan`/`--plan`/`draft` *create* sessions; `cruise run` *executes* them; `cruise list` *manages* them; `cruise clean` *garbage-collects* them.
- `cruise exec` runs in the current directory without a plan, worktree, or PR. Success removes the session. Failure retains its original ID, checkpoint, logs, and artifacts until a retry succeeds or you choose **Delete** in `cruise list`. Paused or interrupted sessions remain resumable by ID. `force_exec: true` enables the same path for direct plan entry points. `--no-force-exec` opts out once.
- The WebUI offers **Exec** for Directory sessions and replaces **Create & plan** with **Execute** for `force_exec`. Use **Create draft** to plan instead. Suspended exec sessions can be resumed in the sidebar, but failed exec recovery is CLI-only.
- `cruise exec --instant "task"` explicitly builds an in-memory single-agent workflow without a workflow YAML. Ordinary `exec` still resolves configuration: with no config file, interactive terminals offer built-in workflows in the categorized selector, while non-interactive execution selects `tdd` automatically, not instant.
- Bare `cruise` opens `cruise list` on an interactive terminal; piped stdin still follows the positional planning path. Use `cruise list` to manage sessions, or the WebUI.

## Existing PR review fixes

`cruise plan --pr 12` creates a deterministic plan without reading stdin or invoking an LLM. Optional positional input adds instructions; `--repo owner/repo` targets another repository. Approve the plan, then `cruise run <session-id>`. The WebUI GitHub repository form also accepts a PR number. Authenticated `gh` is required; open same-repository PRs, including drafts, are supported, but forks, closed or merged PRs, and GitLab MRs are rejected. `--grill` and `--image` conflict with `--pr`.

The default `builtin:pr-review-fix` has no flags and runs in an isolated worktree on the existing PR head, tracking `origin/<head>`. It fast-forwards safely, rejects divergence or a head checked out elsewhere, and never resets the branch. The review loop processes eligible unresolved threads, pushes committed fixes, and replies or resolves; it does not create or merge a PR. Cleanup preserves the head branch. Failures are retryable, and Publish as Issue and Replan are unavailable. A custom workflow must have `create_pr: true` and an after-PR `github-review` step with `reviewers: all`. See the [review configuration reference](../cruise-config/references/after-pr.md#github-review) for permissions, progress rules, and the report.

## Experimental Manager (repository goals)

`cruise manager` turns a repository-level goal into a reviewed proposal of several tasks, then (after explicit approval) runs each task as a Manager-owned child session, integrates the results serially, verifies the combined SHA, and opens **one** PR/MR that a human merges. Manager never merges. Use `cruise plan` for one task → one session; use Manager only when the user wants Cruise to decompose and coordinate a goal. It is experimental.

Run it inside a local Git checkout (`cruise manager` requires one); every subcommand is a daemon client. Records live in `$XDG_DATA_HOME/cruise/manager/`, keyed by the Git common directory, so linked worktrees share one record and the WebUI **Manager** page shows the same conversation, proposal, and goals.

| Subcommand | Purpose |
|---|---|
| `cruise manager` / `status [--json]` | Show transcript, pending proposal (with revision), goals, and pending child prompts. Use `--json` to read Goal, task, and prompt IDs in scripts. |
| `say [TEXT] [--no-wait]` | Discuss the goal. Without TEXT, reads a terminal prompt or piped stdin. Waits for the reply and exits non-zero if the turn does not complete; `--no-wait` returns the turn ID. Ctrl-C stops waiting only. |
| `propose [--no-wait]` | Generate a validated proposal preview from the conversation. The target branch must resolve to a local `origin` tracking ref (fetch first). A preview authorizes nothing. |
| `approve [--revision N] [--yes]` | Print the proposal (tasks, checks, contract) and approve it. Terminal: type `approve` unless `--yes`. No terminal: **both** `--yes` and `--revision N` are required, and N must match the current preview. Creates one active Goal; the daemon schedules children asynchronously. |
| `pause <GOAL>` / `resume <GOAL>` | Pause or restore Goal-scoped scheduling. |
| `resume <GOAL> <TASK>` | Continue the task's latest failed or suspended child in its existing worktree. |
| `retry <GOAL> <TASK>` | Reserve a fresh attempt for an eligible failed/suspended child, or a completed child of a Goal blocked by a safely established integration conflict. For a Goal blocked by `interrupted Manager reservation no longer matches Goal tip`, reactivate it instead when that task's reservation has no published session; the reservation then starts once from the current tip. |
| `retry <GOAL>` | Re-verify a blocked Goal whose tasks are all integrated, keeping prior evidence. Discards any unapproved corrective preview into the Goal's audit history so it can no longer be approved. Does not retry an uncertain publication. |
| `retry <GOAL>` (legacy GitHub title rejection) | Reconcile an older `CreateIntent` only when it records GitHub's definite `Title is too long` rejection and a successful no-PR lookup. Requires the same verified tip and approved contract, fresh authenticated GitHub observation, and unchanged approved remote refs before a new create. Adopts a matching PR instead of creating another. |
| `answer <GOAL> <PROMPT> [TEXT]` | Answer a child's Ask prompt (TEXT or a terminal prompt; blank is rejected). |
| `answer <GOAL> <PROMPT> --choice LABEL [--text TEXT]` | Answer a child's Option prompt outside a terminal with the exact label; `--text` only for text-input choices. On a terminal, Option prompts use the menu and `--choice` is rejected. |
| `corrective <GOAL> <TASK> [--instructions TEXT]` | After a definite failed check on a blocked Goal, save a corrective task preview (instructions from the flag, terminal, or piped stdin). Authorizes nothing. |
| `corrective <GOAL> <TASK> --resume [--instructions TEXT]` | Preview resuming the task's **failed** attempt in its own session and worktree with instructions appended to its plan. Bound to the attempt, failed step, worktree content, Goal tip, and contract: a change before launch refuses it, and it must be previewed again. |
| `corrective <GOAL> <TASK> --evidence --criterion ID… --path FILE…` | Preview resolving a failed task the Goal tip already satisfies. After approval the daemon runs **every** approved check of those criteria at that exact tip; all green resolves the task without a commit, anything else leaves it failed and is never rerun automatically. |
| `corrective <GOAL> --amend FILE` | Preview a narrow wording amendment of criteria or task instructions. FILE is a JSON array such as `[{"criterion":"C1","replace":"old","with":"new"}]` (`"task"` for instructions, optional `"old_sha256"`). The proposal is never rewritten and checks cannot be amended. |
| `corrective <GOAL> --retarget` | Preview a fast-forward of the same GitHub target ref after a definite legacy title rejection. Each task's latest attempt must be integrated; older finalized results superseded by that integration stay in the audit history. Requires the verified remote head and fresh authenticated PR absence across all states. Approval integrates the pinned base once and reruns every approved check before guarded publication. |
| `approve-corrective <GOAL> [--revision N] [--yes]` | Approve that exact preview with the same confirmation and revision rules as `approve`. A changed preview, integrated tip, or contract needs fresh review. |

Typical scripted flow (approval requires the user's explicit authorization — never self-approve on their behalf):

```sh
cruise manager say "Goal: migrate config loading to XDG paths; keep the CLI stable"
cruise manager propose
cruise manager status --json | jq '.proposal | {id, revision}'
cruise manager approve --yes --revision <N>     # only after the user approved this revision
cruise manager status --json | jq '.goals[] | {id, status, reason}'
```

Constraints and gotchas:

- Conversation and children resolve `builtin:tdd` with user/project `default.yaml` and the **daemon** environment; Manager requests do not forward client `CRUISE_*` overrides. Conversation needs `sdk: jcode` (not `command:`) and a planning model. Each proposal freezes the child workflow with automatic PR creation, after-PR steps, `after_pr`/`merge` flags, post-PR cleanup, and `force_exec` removed; project workflow YAML does not customize children. See the **cruise-config** skill.
- Execution requires Git 2.38+ and an authenticated GitHub or GitLab `origin` (fetch and push URLs must match).
- Manager-owned child sessions are read-only elsewhere: `cruise list` offers only **View Diff** (when available) and **Back**, and their WebUI pages are read-only with **Open in Manager**. Answer their Ask/Option prompts with `cruise manager answer` or the Manager page, not `cruise run`.
- A failed check blocks the Goal (`status` shows `reason`). Choose `retry <GOAL>` for a flaky check, or `corrective` + `approve-corrective` to add a fix task. If verification fails again after a retry, create a fresh corrective preview rather than approving a discarded one. Failed children name the exact `resume`/`retry` command in the Goal reason.
- `corrective --resume`, `--evidence`, and `--amend` act on an active Goal and are separate approval units: show the user each saved preview (`status --json`, `.goals[].execution.corrective_preview`) and approve only the revision they approved. `resume`/`retry` never deliver an unapproved correction. Evidence results are in `.goals[].execution.evidence_runs`; an `unresolved` outcome keeps the task failed.
- New conversation turns and proposals need the daemon to reach the SDK provider. The WebUI **Manager** page opens only repositories with an existing record and an accessible main checkout on the daemon host.
- Manager sends only the ID, objective, and status of completed Goals to the provider when discussing or drafting; CLI status and the WebUI still show full records.

## Which command do I want?

| Goal | Command |
|------|---------|
| Plan a task, then approve it interactively (foreground) | `cruise plan "task"` |
| Address reviews on an existing GitHub PR | `cruise plan --pr 12` or `cruise plan --repo owner/repo --pr 12 "additional instruction"`, then approve and run the session |
| Plan in the background, review later, return immediately | `cruise --plan "task"` |
| I already wrote the plan myself — skip the LLM planning | `cruise plan --skip-planning "<plan text>"` (or `cruise --plan "…" --skip-planning`) |
| Interview me one question at a time, then write the plan | `cruise plan --grill "task"` (SDK backend (the default) + TTY) |
| Add executable Quint, Alloy, and Gherkin specifications with CI | `cruise exec -c builtin:add-formal-specs "task"` |
| Read the embedded common E2E policy or one framework policy | `cruise docs e2e` / `cruise docs e2e react-native` / `cruise docs e2e jetpack-compose` / `cruise docs e2e flutter` / `cruise docs e2e tauri` / `cruise docs e2e slint` / `cruise docs e2e fyne` / `cruise docs e2e browser-extension` |
| Verify a plan without changing it | Choose **Verify** in the interactive plan approval menu or WebUI for a usable plan in either **Awaiting Approval** or **Planned**; `cruise list` and Ask text do not provide this action |
| Run `refactor`, `add-tests`, `add-formal-specs`, or `security` locally by default (`force_exec: true`) | For example, `cruise plan -c builtin:refactor "task"` runs directly without an outer plan/session, worktree, or automatic PR; add `--no-force-exec` for the normal plan/session path, which declares no after-PR flags or steps and still opens a PR when eligible |
| Research with a durable, read-only report | `cruise plan --skip-planning -c builtin:research "question"`; approve if prompted, then `cruise run <session-id>` |
| Audit and safely repair confirmed security defects in a durable session | `cruise plan --no-force-exec -c builtin:security "audit request"`; approve if prompted, then `cruise run <session-id>` |
| Disable SDK planning tools and have the agent write `plan.md` directly | `cruise plan --no-interactive-planning "task"` |
| Attach planning images | `cruise plan --image screenshot.png "task"` (repeat `--image` as needed) |
| Plan from a GitHub issue | `cruise plan --issue 12 "additional instructions"` or `cruise plan '#12'` |
| Target a GitHub repo instead of a local directory | `cruise plan --repo owner/repo "task"` (also with `--plan`) |
| Just capture an idea now, plan later | `cruise draft "task"` |
| Execute or resume a non-exec session (Planned, Running, Failed, or Suspended; prompts when multiple) | `cruise run` |
| Execute a specific session | `cruise run <session-id>` |
| Run a config right here, no plan/worktree/PR | `cruise exec "task"` (or `cruise "task"` with `force_exec: true`; use `cruise plan "ssh"` or `cruise plan "mcp-bridge"` when the task text is exactly `ssh` or `mcp-bridge`, respectively) |
| Run one agent without writing workflow YAML | `cruise exec --instant "task"` |
| Execute every Planned or Suspended session back-to-back (live dashboard on TTY for non-dry runs) | `cruise run --all` |
| Run all planned or suspended sessions with bounded concurrency (one run) | `cruise run --all --parallelism 4` |
| Retry eligible Run All failures once (one run) | `cruise run --all --auto-requeue-max-attempts 1` |
| Set the shared Run All retry limit | `cruise config --set-auto-requeue-max-attempts 1` |
| Set how often after-PR reruns GitHub jobs no runner acquired, per PR head (default 2, 0 disables) | `cruise config --set-ci-unacquired-rerun-max-attempts 2` |
| Browse, approve, resume, or delete sessions interactively | `cruise list` |
| Have Cruise split a repository goal into child sessions and publish one PR/MR (experimental) | `cruise manager say "goal"`, `cruise manager propose`, then `cruise manager approve` (see [Experimental Manager](#experimental-manager-repository-goals)) |
| Inspect Manager goals, proposals, or pending child prompts | `cruise manager status [--json]` |
| Answer a prompt from a Manager-owned child session | `cruise manager answer <GOAL> <PROMPT> [TEXT]` or `--choice LABEL [--text TEXT]` |
| Review a session branch's changes or summary | `cruise diff <session-id>` / `cruise diff <session-id> --stat` |
| Dump session state for scripts | `cruise list --json` |
| Automate, emit JSON, or run in CI | Existing CLI commands, especially `cruise list --json` and `cruise run` |
| Recover a failed exec, including instant or forced exec | `cruise run <session-id>` or **Resume** in `cruise list` |
| Delete eligible closed/merged PR and no-PR sessions (keeps failed exec sessions) | `cruise clean` |
| Summarize locally recorded token usage | `cruise usage [--by step,backend,model] [--session ID] [--format text\|json\|csv]` |
| Run cruise on another machine | `cruise ssh <host> [--cwd <remote-path>] [-- <cruise-args>]` |
| Serve the local browser UI (add `-o`/`--open` to launch the browser) | `cruise webui` |
| Manage the resident daemon | `cruise daemon start`, `stop [--force]`, `restart`, `status [--json]`, or `serve` |
| Update Cruise itself | `cruise update` |
| Install/update the jcode CLI or inspect managed status | `cruise install jcode [--force]` / `cruise install --status` |
| Inspect in-progress update transactions across installations (read-only; no network access or changes) | `cruise update --status` |
| Install / update / remove / list a GitHub workflow package, including root-manifest bundles (confirms first; `--yes` required without a TTY) | `cruise workflow add owner/repo[/path.yaml][@ref]` / `update <name>` / `remove <name>` / `list` |
| Draft a new workflow YAML from a description | `cruise workflow generate "description" --name <name> [--user] [--config <path>]` |
| Interactively create or revise workflow YAML | `cruise workflow make --name <name> [--user] [--config <path>] [--from <selector>] [description]` |
| Statically check workflows or a package | `cruise workflow doctor [--json] [--strict] [-f NAME] [TARGET]...` |
| Show / change app-level settings (e.g. WebUI parallelism; displays `prevent_sleep`) | `cruise config` |
| Sign the default `jcode` backend in to a provider / inspect what's configured | `jcode login <provider>` / `jcode auth status` (cruise has no login command of its own) |
| See what *would* run without executing | add `--dry-run` to `plan` / `run` / `exec` |

### Managed provider CLIs

`cruise install jcode` checks upstream release metadata on every install invocation and selects the latest stable CLI. A healthy managed installation at that version is reused; an installation at another version is updated. `--force` downloads the selected latest version again. Installation requires network access to release metadata and downloads; upstream changes may affect compatibility. Downloads use publisher-supplied checksums, not independent signatures. `cruise install --status` is read-only and offline: it reports the recorded installed version without claiming it is latest. Managed-runtime validation uses the version recorded for that installation, not a Cruise-build pin. Provider authentication remains separate.

Resolution prefers the nonempty `CRUISE_JCODE_CLI_PATH` process environment override, then a valid managed installation, then PATH. An invalid override fails instead of falling back. Workflow `env:` cannot set the explicit provider override, but its `PATH` controls PATH fallback for the child. Running workflows never installs providers. `cruise update` updates Cruise, not providers.

For managed-only jcode authentication, read `relative_path` from `providers/jcode/install.json` under the Cruise data directory, then invoke that binary with `login <provider>` or `auth status`. The data directory is `$XDG_DATA_HOME/cruise`, or `~/.local/share/cruise` on Unix when unset. On Windows without `XDG_DATA_HOME`, it is `%LOCALAPPDATA%\cruise\data` when `LOCALAPPDATA` is available, otherwise the home-directory fallback applies. Quote paths containing spaces.

### Instant execution

`cruise exec --instant --review --review-rounds 2 --verify 'cargo test' --verify 'cargo clippy' "task"` runs implement, optional review, then verification commands in order. The task must be nonblank. Review repeats only when its pass changes files, up to the positive pass limit (default 2); changes in the final pass are not reviewed again. A failed verification command fails the run rather than triggering a repair loop. Blank verification commands and commands containing braces are rejected.

Use `--sdk jcode` and `--model <MODEL>` to select the SDK backend and model. The only supported SDK is `jcode`; `CRUISE_SDK` accepts no other value and takes precedence over these options. `CRUISE_MODEL` also takes precedence over `--model`. Instant bypasses workflow config discovery, including `CRUISE_CONFIG`, but uses the existing inline session snapshot, not a generated repository file. `--instant` conflicts with `-c/--config` and `-f/--flag`; the other instant options require it, and `--review-rounds` requires `--review`. Existing `--dry-run`, `--max-retries`, and `--rate-limit-retries` work; the retry budget cannot be lower than the review limit. Like other exec runs, it requires a git repository on an attached branch, allows existing changes, and retains paused/interrupted sessions for explicit resume. Remote example: `cruise ssh <host> -- exec --instant "task"`.

### Updating Cruise

For a supported managed install, check the current version, update, then check again:

```sh
cruise --version; cruise update; cruise --version
```

`cruise update` fetches the latest official stable release and is a no-op when that release is the same as or older than the installed version. Native updates support `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`, and `x86_64-pc-windows-msvc`.

| Installation | Update method |
|---|---|
| Shell or PowerShell installer at `~/.local/bin` (on Windows, `%USERPROFILE%\.local\bin`), or an existing installer install at canonical `$CARGO_HOME/bin` | `cruise update` |
| Source-built install at canonical `$CARGO_HOME/bin` | `cruise update` installs the official runtime; local source changes/build-time features are not preserved |
| Homebrew | `brew upgrade smartcrabai/tap/cruise` |
| GitHub Action, Docker, or another external install | Use its original distribution method |
| `CRUISE_RUNTIME` override set | Unset it; run update from the managed launcher |

Managed means the executable resolves into the installer default `~/.local/bin` (on Windows, `%USERPROFILE%\.local\bin`) or canonical `$CARGO_HOME/bin`, not that Cargo’s install ledger claims it. Updating adds a versioned `cruise-runtime-managed-…` runtime and `.json` sidecar, then switches `.cruise-runtime-active`; the fixed `cruise` (`cruise.exe` on Windows) launcher and older runtimes remain. The updater does not run Cargo or modify its install ledger. Homebrew, Action, Docker, and other external installs are not changed by this command. Published versions without the updater—including v0.2.16—cannot update themselves. An updater-capable build can select v0.2.16, but that published runtime cannot perform another native update. Bootstrap by installing an updater-enabled release with its platform installer, or rebuilding/installing that source version; use native updates only while the installed runtime includes the updater.

The updater downloads over HTTPS and checks the release manifest’s SHA-256 against the payload. The checksum comes from the same release publisher; it is not an independent signature.

`cruise update --status` prints recorded transactions for installations in sorted order and is read-only: no network, lock, or repair. Its transaction file is `$XDG_STATE_HOME/cruise/installations/<installation-id>/update-transaction.json` (default `~/.local/state/cruise/installations/<installation-id>/update-transaction.json`; on Windows, `%LOCALAPPDATA%\cruise\state\installations\<installation-id>\update-transaction.json` when `XDG_STATE_HOME` is unset). Status reports the recorded phase/error, not proof of the active runtime. A failed transaction keeps its error; rerun `cruise update` to reconcile an interruption. Failures before activation leave the previous selection, and a failed post-activation launcher probe restores it. If completion/cleanup warns after activation, the update is already applied; inspect status and retry rather than treating it as rolled back.

Older versioned runtimes coexist beside the fixed launcher in its installation directory: `~/.local/bin` (Windows `%USERPROFILE%\.local\bin`) for the installer default, or canonical `$CARGO_HOME/bin` (Windows default `%USERPROFILE%\.cargo\bin`) for legacy Cargo/source-built installs. To pin an installed version, run that exact sibling with `--version`, confirm its `.json` sidecar matches the current fixed launcher/runtime baseline, and set `.cruise-runtime-active` to its exact basename; do not delete runtime files. The baseline is the fixed launcher/runtime size-and-mtime stamp: a reinstall that changes it invalidates the overlay, while a same-size, same-mtime replacement is not detected. Legacy pointers or pinning an older launcher may still require manual retirement. Before retiring the pointer, confirm the fixed sibling itself runs:

```sh
# macOS / Linux
# For legacy Cargo/source-built installs, set this to their actual bin directory instead.
bin="$HOME/.local/bin"
archive="$bin/.cruise-runtime-active.archive"
retired="$bin/.cruise-runtime-active.retired"
test ! -e "$archive" && test ! -e "$retired" &&
  "$bin/cruise-runtime" --version &&
  cp -n "$bin/.cruise-runtime-active" "$archive" &&
  mv -n "$bin/.cruise-runtime-active" "$retired"
```

```powershell
# Windows
# For legacy Cargo/source-built installs, set $bin to their actual installation bin.
$bin = Join-Path $HOME '.local\bin'
$runtime = Join-Path $bin 'cruise-runtime.exe'
$pointer = Join-Path $bin '.cruise-runtime-active'
$archive = "$pointer.archive"
$retired = "$pointer.retired"
if ((Test-Path -LiteralPath $archive) -or (Test-Path -LiteralPath $retired)) { throw 'Choose unused archive and retired paths.' }
& $runtime --version
if (-not $? -or $LASTEXITCODE -ne 0) { throw 'The fixed sibling runtime did not pass --version; pointer was not moved.' }
Copy-Item -LiteralPath $pointer -Destination $archive -ErrorAction Stop
Move-Item -LiteralPath $pointer -Destination $retired -ErrorAction Stop
```

Only move the pointer after the fixed sibling’s `--version` succeeds. Archive it first; never delete the runtime or sidecar. Use unused archive/retired names if needed.

### Remote execution with `cruise ssh`

`cruise ssh <host> [-- <args>]` delegates the entire command to a `cruise` installed on the SSH host. Without forwarded arguments, the remote command runs `cruise list` (the interactive selector when a PTY is available). `--cwd` and `--cruise-bin` are remote directory and executable paths, and all other arguments after `--` are parsed only by the remote cruise. Use the system OpenSSH configuration for aliases, ports, identity files, ssh-agent, and ProxyJump.

The remote host owns its XDG session/config/state directories, jcode authentication, worktrees, and repository clones. Local and remote session lists are separate. No local config, image, repository, credential, or environment variable is uploaded implicitly. In particular, `--config`, `--image`, and `--cwd` paths must exist or be meaningful on the remote host. The WebUI does not act as a remote session browser.

> **Legacy shortcut:** `cruise "task"` with positional input and no subcommand is treated as `cruise plan "task"`. The `ssh` and `docs` subcommands reserve those exact legacy inputs; use `cruise plan "ssh"` or `cruise plan "docs"` for those tasks. The hidden `mcp-bridge` entry point likewise reserves the exact input `mcp-bridge`; use `cruise plan "mcp-bridge"` for that task. Piping (`echo "task" | cruise`) feeds the task on stdin.

## The session lifecycle, step by step

1. **Create a session.**
   - `cruise plan "task"` — runs the built-in plan step in an isolated *planning worktree*; in a non-git directory, planning runs in the base directory without a worktree, and `cruise run` then requires a git repository. It then drops you into the **approve-plan menu** (below). Foreground.
   - `cruise --plan "task"` creates the session, starts planning asynchronously inside the daemon, and returns the session ID immediately. `cruise list` shows `Planning`, then `AwaitingApproval` (or `Plan Failed`).
   - `cruise draft "task"` — records the task as a `Draft` with no planning at all. Use `cruise draft --issue 12` to capture a GitHub issue. Plan it later via **Generate Plan** in `cruise list`.

2. **Approve the plan.** The approve-plan menu offers:
   - **Approve** → session becomes `Planned`, ready to run.
   - **Fix** → give feedback; the plan step reruns with your input.
   - **Ask** → ask a question; the answer is captured internally, then the menu reappears without displaying it.
   - **Execute now** → skip approval and run immediately.
   - **Publish as Issue** → publish `plan.md` as a GitHub issue (or GitLab issue) and delete the local session (see [Publish as Issue](#cruise-list--phase--available-actions)).

   After **Approve** or **Execute now**, declared workflow flags are selected first unless `-f/--flag` was supplied, then the step-skip selector offers only flag-enabled steps. Cancelling either selector returns to the action menu without saving partial changes, approving, or executing. Execute now forwards these flag values without a second execution-time prompt.

3. **Run.** `cruise run` without an ID selects a non-exec `Planned`, `Running`, `Failed`, or `Suspended` session (prompting when several qualify), uses saved workflow flags plus any explicit `-f/--flag` overrides without a flag selector, prompts for a **workspace mode** when applicable, reuses/creates the worktree, and executes or resumes workflow steps. By default Cruise opens a PR when branch commits are available, then runs configured `after-pr` steps. `-f after_pr` enables that prelude only; `-f merge` enables the same prelude once and adds the final native synchronous squash merge. Both flags are default-off, and the implied prelude does not rewrite `after_pr` in the session's sparse saved flag map. A successful run with no new branch commits skips PR creation without error; any opted-in after-PR action is skipped with a reason when no PR is created. Workflow-level `create_pr: false` skips automatic commit, push, PR creation, `after-pr`, and PR-based cleanup while retaining the normal session, artifacts, and worktree; it never allows `merge` to override the no-PR policy. Configured `if.no-file-changes` failure/retry behavior is unchanged. Publication failures in both `--repo` and local worktree sessions enter `PR Failed`; use **Create PR** to retry publication without rerunning the workflow. A paused step leaves the session `Running`. Ctrl+C normally leaves a run `Suspended`, but canceling recovery of a failed exec before it enters `Running` leaves it `Failed`. Exec sessions are excluded from automatic selection and `run --all`. Failed exec sessions are also excluded from WebUI inventory. Recover a failed exec explicitly with `cruise run <id>` or **Resume** in `cruise list`. Use `cruise run <id> --dry-run` to preview the retry point. Unhandled nonzero step exits do not stop later steps, but unresolved failures give the attempt a nonzero exit. Resume starts at the first unresolved failure, or at the durable checkpoint for a hard error with no earlier unresolved failure. If setup failed before workflow execution started and no checkpoint exists, recovery starts at the beginning. A missing or unusable checkpoint blocks recovery for a workflow that already started. For `cruise run`, the graph retry budget comes from an explicit `--max-retries`, the original explicit exec value when resuming an exec, the workflow's top-level `max_retries`, or `3`, in that order. That step and later steps may run again, without rolling back files or external effects. Failed exec rows add `"exec": true` in `cruise list --json`; ordinary rows are unchanged.
   Before a fresh GitHub or GitLab workflow run, Cruise checks the configured `base_branch` or forge default branch: failing CI or a status retrieval failure stops the run, while pending or absent checks warn and continue; CLI dry runs, `exec`, workflow and after-PR resumes, and origins not resolvable to a supported forge are skipped, and `check_base_ci: false` in `default.yaml` opts out.
   On macOS, setting `prevent_sleep: true` in the application config prevents sleep during runs; it defaults to false and has no effect on other operating systems.
   `create_pr` is a workflow-only boolean that defaults to `true`; it is not a CLI option and is rejected in user/project `default.yaml`. Setting it to `false` skips Cruise's automatic commit, push, PR creation, `after-pr`, and PR-based cleanup while preserving the normal session, artifacts, and worktree. If `after_pr` or `merge` was opted in, unavailable after-PR actions are skipped with a reason.
   `commit_message_template` and `pr_body_template` are workflow or user/project `default.yaml` settings, not CLI options; see [template configuration](../cruise-config/references/top-level.md#commit_message_template-and-pr_body_template).

4. **Clean up.** `cruise clean` checks each PR-backed `Completed` session via `gh pr view` (or `glab mr view` on GitLab) and deletes the session + worktree (and any leftover `--repo` clone) once the PR is merged or closed. A normal session with `create_pr: false` retains its session, artifacts, and worktree without waiting for a PR. Before any completion cleanup, a worktree or `--repo` clone session copies untracked/ignored `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, `.md`, `.txt`, and `.log` evidence (5 MiB per file, 25 MiB and 100 files per session, images first; `node_modules` and similar dependency/cache directories skipped) into `<data_dir>/sessions/<id>/artifacts/evidence/`. `artifacts/evidence.md` lists saved and skipped files; its path appears in the run log, the WebUI **Info** tab, and the `cruise list` session view. Evidence is not added to the PR.

### `--skip-planning`

`--skip-planning` skips the planning call: the trimmed input (plus stored attachment paths, when present) is written to `plan.md`; empty/whitespace input is rejected. Foreground TTY use still opens the approval menu. Foreground non-TTY use auto-approves to `Planned`; background `cruise --plan … --skip-planning` also creates a `Planned` session immediately. SDK-mode foreground approval makes a separate title-generation request; background skip-planning and command mode derive the title from `plan.md`. When the resolved local workflow has `force_exec: true`, `--skip-planning` does not opt out: force-exec runs before this flag is handled; `--no-force-exec`, `--grill`, `--image`, and issue input opt out. Such sessions store an empty `input` with `input_as_plan: true` in `state.json`; `plan.md` holds the task text and `{input}` resolves to the current `plan.md` content.

### `--grill` (interview planning)

`cruise plan --grill "task"` turns planning into an interview: the SDK agent asks one question at a time, recommends an answer, and then writes `plan.md`. It requires an SDK backend, an interactive terminal, and `interactive_planning: true`; a non-TTY invocation errors before session creation, while an SDK/tool-less mismatch errors and deletes the new session. It conflicts with `--skip-planning` and affects only the initial plan; Fix/Ask, replan, drafts, and background `--plan` use the standard prompt. The WebUI equivalent is the **"Grill me"** toggle.

### `--no-interactive-planning`

`cruise plan --no-interactive-planning "task"` disables interactive SDK planning for this invocation and asks the agent to write `plan.md` directly. It conflicts with `--grill`; the WebUI equivalent is **Non-interactive planning**. When the resolved local workflow has `force_exec: true`, `--no-interactive-planning` does not opt out: force-exec runs before this flag is handled; only `--no-force-exec`, `--grill`, `--image`, and issue input opt out.

### Built-in workflow flags

`builtin:tdd` supports `cruise run <id> -f advisor` for one read-only second opinion after implementation, off by default and jcode-only. On success, the normal model evaluates fresh advice once and applies only confirmed in-scope defects. It does not request another review.

`tdd` and `research` declare `after_pr` and `merge`, both off when absent. `research` is `create_pr: false`, so its flags are skipped with a reason. `refactor`, `add-tests`, `add-formal-specs`, and `security` declare neither flag and have no after-PR steps, even with `--no-force-exec`. Only `tdd` and `refactor` additionally declare the opt-in `e2e` flag and `formal_methods`; there are no separate browser/app flags. `after_pr` runs after-PR automation without merging; `merge` runs the same prelude and then the native synchronous squash merge once.

`e2e` enables applicable web, mobile, and desktop UI testing with only tools already in the repository. E2E prompts consult the embedded common policy with `cruise docs e2e` and the applicable framework policy with `cruise docs e2e <framework>`; the canonical sources are in `docs/e2e/`. Output is read-only, offline, and tied to the running Cruise version. For native Android Jetpack Compose UI, use the `jetpack-compose` policy; non-Compose Android follows the common policy as an unlisted target. Other unlisted native targets are not unsupported: use suitable existing repository tools only and report missing capabilities without introducing defaults. Capture actual app screenshots and inspect them at pixel level; save deterministic assertion-bearing E2E tests. TDD runs `e2e-baseline` → `e2e` before `verify-review-pass`; refactor runs `prepare-e2e-baseline` → `prepare-e2e` → the existing `verify-refactor-pass` review → `e2e-final-baseline` → `e2e-final-check`, all gated by `e2e`. A host guard gate retries an E2E step until it undoes changes to dependency manifests, lockfiles, toolchain pins, or CI configuration and any newly introduced E2E framework. The TDD `e2e` and refactor `e2e-final-check` reports list missing capabilities with their exact preparation under `## Blockers` and end with one `E2E_STATUS: passed|blocked|not-applicable` line; `blocked` stops the session with the blockers as its failure reason. Prepare and resume, or resume with `-f e2e=off` to continue without UI E2E. Mark out-of-scope targets not applicable; complete reachable work and record exact tools, versions, setup, and results.

`formal_methods` enables workflow-level executable Quint, Alloy, and Gherkin specifications plus CI. The six general-purpose built-ins are `tdd`, `refactor`, `add-tests`, `add-formal-specs`, `research`, `security`; `pr-review-fix` declares no flags. `add-tests` inventories consumer-visible behaviors and real journeys with stable `COV`/`GAP` IDs, existing test/assertion evidence, `Covered` / `Partial` / `Missing` status, prioritized actionable gaps, and explicit exclusions/limits without numerical coverage quotas; it writes `test-ci-gap-plan.md`, `test-ci-implementation.md`, and `test-ci-verification.md`, with independent per-ID closure. `research` performs bounded read-only plan → investigate → review → synthesize phases and retains `research/plan.md`, `research/evidence.md`, `research/review.md`, and `research/report.md`; `security` inventories scope, performs parallel code/dependency/delivery audits, triages, repairs confirmed safe defects, and independently verifies each finding, retaining `security/scope.md`, `security/code-findings.md`, `security/dependency-findings.md`, `security/delivery-findings.md`, `security/worklist.md`, `security/repairs.md`, and `security/verification.md`. Research/security reports distinguish evidence from inference, preserve finding IDs and review dispositions, cite sources, and state blocked or unrun work honestly.

On a confirmed current-head GitHub conflict, `follow-up/wait-ci` exits promptly after validating the commit and rollup unless CI reports a failure. Failed CI retains its diagnostic and repair path, including waiting for running Actions suites. A stale PR head still observes the propagation grace period. On GitHub, `follow-up/wait-ci` treats CI as ready only when the PR head and an existing rollup commit match the pinned local `HEAD`, the rollup state is `SUCCESS`, and every GitHub Actions check suite on that commit with a `workflowRun` is `COMPLETED`. A completed suite with a failing conclusion fails the wait; suites without `workflowRun` (such as third-party app checks) do not block. Readiness must hold for two consecutive polls, and any non-ready poll resets the streak. A PR-head mismatch stays pending for up to 90 seconds for API propagation; a persistent mismatch after the grace period or overall deadline fails with the existing `head_mismatch` diagnostic. Native GitHub merge uses the same CI readiness gate, with 2-second polls for up to 10 minutes while waiting for `UNKNOWN` mergeability; the built-in wait uses 5-second polls for up to 55 minutes. When `wait-ci` hits its deadline with current-head checks still queued or running, each native merge poll that still sees them pending restarts the 10-minute budget, for up to 60 minutes after the merge step starts; CI failures still stop it. GitLab's provider-specific readiness behavior is unchanged. See the [after-PR config reference](../cruise-config/references/after-pr.md#opt-in-native-synchronous-squash-merge) for details.

The after-PR CI wait allows up to 60 minutes. If checks are still running at
its deadline with no failed rollup, it reports pending without invoking
repair, and native GitHub merge keeps waiting for them as described above.
If the GitHub rollup is already failing, it keeps polling attached
Actions suites until they finish or the deadline; the deadline preserves the
failure and invokes repair with available diagnostics.

The shared prompt receives wait-ci stderr, enumerates every non-success check
on the current head, reads canceled/timed-out job logs, and reproduces the
exact CI commands. Empty or truncated diagnostics fall back to provider
queries. It fixes all in-scope failures in one pass and runs matching
formatters, linters, and tests before commit. Unrelated/flaky jobs are rerun
once or reported clearly; no-change repairs do not loop.

Before that prompt, `follow-up/rerun-unacquired-ci` handles GitHub failures
made only of Actions jobs that no runner acquired (API-verified: conclusion
`cancelled`, no runner, no steps, failure annotation `The job was not acquired
by Runner ...`). It runs `gh run rerun --failed` and returns to `wait-ci`, up
to `ci_unacquired_rerun_max_attempts` times per PR head (default `2`), counted
in the session so resuming does not recount. Mixed failures, GitLab, or a
spent limit go to the repair prompt as before; at the limit the run stops
unmerged with a message naming the setting.

At merge time, GitHub's compare API checks whether the PR includes the current base. Only confirmed behind/diverged ancestry or `CONFLICTING`/`DIRTY` mergeability routes the built-in `squash-merge` through `follow-up/sync-base` → `sync-base` → `resolve-conflict` → `push-after-sync` → `wait-ci` → `fix-ci-error`. Only native `ResyncRequired` routing from `squash-merge` to its `if.fail` target starts a fresh after-PR pass: it resets `budgeted_traversals` for every after-PR edge except `squash-merge` → target and clears group retry counts, preserving edge traversal history. The resync edge remains capped at `max_retries`; generic `if.fail` retains shared budgets. Worst case: up to `max_retries` resyncs, each pass allowing up to `max_retries - 1` CI-fix iterations; zero retry budget permits no resync. Other readiness failures still fail. This ancestry check and recovery are GitHub-only; GitLab behavior is unchanged.

### `--image`

`cruise plan --image screenshot.png "task"` attaches a PNG, JPG/JPEG, WebP, or GIF; repeat the flag for multiple files. Interactive plan input also detects dragged/pasted image paths. Cruise copies attachments into the session so paths remain stable and appends those stored paths to the planning input. Image attachments disable `force_exec` for that invocation so planning can consume them.

### `--flag` (workflow flags)

`-f` is an alias for `--flag NAME`, which enables a workflow flag declared in top-level `flags:`; comma-separated names and repeated options enable selected flags. Absent flags are off. Explicit `NAME=on|off|true|false` assignments also work, including `--flag NAME=off` to override a saved enabled value. `cruise run` uses sparse saved flag values merged with explicit overrides without a flag selector, including fresh runs, resumes, `run --all`, and Run from `cruise list`; the implied `after_pr` prelude from `-f merge` does not add or change an `after_pr` entry in that map. Approve and Execute now select flags before skips in the approval menu unless `-f/--flag` was supplied. Saved enabled values are preselected: Space toggles and Enter confirms, saving selected flags as on and unselected flags as off (no selection means all declared flags off). Execute now forwards the approval-time values without prompting again. `cruise exec` and `force_exec` entry points select flags on a TTY if flags are declared and no `-f/--flag` is supplied; dry runs and non-TTY input skip the selector. CLI and WebUI skip selectors, and the WebUI Graph tab, hide flag-disabled steps, except that legacy unversioned graph checkpoints are displayed without filtering. Unknown names are rejected, and `--flag` conflicts with `--all`.

Execute now from `cruise plan` / `cruise "task"` uses the approval-time selection. Later `cruise run [id]` after approval or background planning, and Run from `cruise list`, use saved flags without a flag selector; skips are not asked again after approval. After `cruise plan -f merge "task"` and Approve, a later `cruise run <id>` without `-f` uses the saved `merge` value without prompting. `cruise exec` and `force_exec` entry points have their own selector; even `cruise --plan "task"` runs in the foreground with `force_exec: true` and prompts on TTY stdin when flags are declared unless `-f` is supplied. Piped `--plan stdin` does not prompt, and resuming a retained exec session by ID uses saved flags without prompting.

After a fatal after-PR failure, Run resumes the checkpointed after-PR node and retry counts without rerunning the main workflow or publishing the PR again. Legacy sessions without an after-PR stage marker restart the after-PR phase instead. Explicit flag assignments still override saved values.

### `--repo` (GitHub repo sessions)

`cruise plan --repo owner/repo "task"` (also `cruise --plan "task" --repo owner/repo`) targets a GitHub or GitLab repository instead of the current directory. cruise clones it via `gh repo clone` (or `glab repo clone` on GitLab) into `$XDG_DATA_HOME/cruise/clones/<session-id>/` and uses the clone as the session's base dir, so the normal worktree flow applies. The repo CLI is still required to access/clone the repository even when `create_pr: false`; that setting skips automatic commit, push, PR creation, `after-pr`, and PR-based cleanup but retains the session, artifacts, and worktree/clone. With PR creation enabled, the plan's worktree/clone is normally removed on approval, then recreated by `cruise run` and removed after PR creation. A successful run with no new branch commits skips PR creation without error, while existing commits remain eligible. Publication failures enter `PR Failed` and retain the workspace for **Create PR** recovery. Repo sessions are **pinned to worktree mode** (no current-branch option), and only clone-contained or builtin configs are copied to `sessions/<id>/config.yaml`; external `-c` or `CRUISE_CONFIG` paths are referenced in place. The WebUI equivalent is the **Directory / Repository (GitHub or GitLab)** source toggle (repository picker backed by `gh repo list`, or `glab repo list` when `CRUISE_FORGE=gitlab`).

If `create_pr: false` (including the built-in `research` workflow), or the run is `exec`, current-branch, or has no commits eligible for PR creation, no PR is created or merged. Any opted-in after-PR action is skipped with a reason; `merge` does not turn these runs into PR-producing workflows.

## Workspace modes (chosen at `cruise run`)

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| Mode | What it does | When to use |
|------|--------------|-------------|
| **Worktree** (default) | Isolated git worktree under `$XDG_DATA_HOME/cruise/worktrees/<id>/`, new branch `cruise/<id>-<slug>`, automatic PR by default. | The normal choice. Keeps your working copy untouched. Automatic PR creation requires `gh` (`glab` for GitLab); a local worktree with workflow `create_pr: false` needs no forge CLI. A `--repo` session still requires its CLI to clone/access the repository. |
| **Current branch** | Runs in place on the active branch. No worktree, auto-PR, or automatic merge. | Quick iterations on the current branch. Fresh normal runs reject **tracked** working-tree changes (untracked files are ignored) and need an **attached branch** (not detached HEAD); `exec`/`force_exec` may start dirty. On resume the branch must match. Opted-in after-PR actions are skipped with a reason because no PR is created. |

A single `cruise run` respects the workspace mode already persisted on the session; the prompt (or, with non-terminal stdin, the Worktree default) applies only to fresh sessions still on the default Worktree mode. `cruise run --all` and `--repo` sessions always force worktree mode (for `--repo` the prompt is skipped entirely). Repo sessions snapshot the resolved workflow config before clone cleanup, so workflow-call expansions remain usable.

**Copy files into the worktree** by listing relative paths in a `.worktreeinclude` at the repo root (e.g. `.env`, `secrets/`). Absolute paths and `..` are ignored for safety.

## `cruise list` — phase → available actions

The top-level picker offers **New Session**, **Clean**, **Run All**, and **Quit**, even when there are no sessions. Selecting a session opens the phase-specific menu below:

| Phase | Actions |
|-------|---------|
| **Draft** | Generate Plan, Delete, Back |
| **Awaiting Input** | Generate Plan, Delete, Back |
| **Awaiting Approval** | Approve, Publish as Issue, Edit Settings, Delete, Back |
| **Planned** | Run, Publish as Issue, Edit Settings, Replan, Delete, Back |
| **Running** | Resume, Tell (live runner only), Reset to Planned, Delete, Back |
| **Suspended** | Resume, Edit Settings, Reset to Planned, Delete, Back |
| **Failed** (non-exec) | Run, Edit Settings, Reset to Planned, Delete, Back |
| **Failed** (inactive exec) | Resume, Delete, Back |
| **PR Failed** | Create PR, Open PR*, Reset to Planned, Delete, Back |
| **Completed** | Open PR*, Merge PR*, Stage local merge**, Merge locally & cleanup**, Reset to Planned, Delete, Back |
| **Planning** / **Plan Failed** | Edit Settings, Delete, Back (Approve/Publish as Issue appear only once a non-empty `plan.md` exists and planning has no error) |

\* Open PR and Merge PR show only when the session has a PR URL.

\** Local merge requires an eligible local worktree session without a PR and with a recorded target. Stage is unavailable once a merge intent or commit is recorded.

Manager-owned child sessions are an exception: they offer only **View Diff** (when available) and **Back**. Control them with `cruise manager` or the WebUI Manager page.

- **Create PR** retries publication without rerunning the workflow: it commits remaining changes, pushes, and reuses an existing branch PR or creates one. Check the publication error and forge authentication first; `gh auth setup-git` can configure Git credentials for GitHub. Publication failure or cancellation retains **PR Failed**, the worktree and any existing PR URL. A fatal after-PR error leaves the session **Failed**. Success runs configured after-PR steps only when a PR was created or reused; a no-commits skip completes without running them. Reset to Planned restarts the workflow; Delete removes the session and workspace.

**View Diff** is additionally available in every phase when the session has a worktree branch. A deleted branch still produces an error.

- **View Diff** compares against the merge-base of the session branch and the base repository's current HEAD. The base can move. It includes committed changes, uncommitted tracked changes, and untracked files when the worktree exists, excluding untracked paths listed in the base repository's `.worktreeinclude` and nested repositories. Tracked changes are included even when listed in `.worktreeinclude`. A missing worktree falls back to committed changes only. Patch output is capped at 1 MiB and untracked files at 200, with truncation reported. The full CLI patch uses `less -R` on a TTY when available; `--stat` and non-TTY output go directly to stdout. There is no diff JSON output.
- Current-branch sessions have no worktree branch to compare. PR cleanup can remove the branch, including with default `builtin:tdd`; use **Open PR** after cleanup. The WebUI Diff tab shows the same comparison; use **Refresh** in the WebUI to reload it.

- **Merge PR** previews the PR (state, mergeability, review decision, checks), asks for a method (`Squash` default, `Merge`, `Rebase`), confirms, then runs `gh pr merge <url> <method flag>` (never `--auto`/`--admin`/`--delete-branch`). If the PR is then Closed/Merged, that session and its worktree are removed; if it is still Open (merge queue), the session is kept and `cruise clean` re-checks later. Closed/Merged PRs are not merged again: use `cruise clean`.
- **Tell** sends text to a live running session: `cruise tell <id> "instruction"`, or pipe text to `cruise tell <id> -`. Inspect history with `cruise tell <id> --log`; the WebUI also offers Tell. Accepted text reaches the next agent prompt, not the current step or command/option steps, and does not change permissions or transitions. Completed runs expire pending text; failed or suspended runs retain it, reset expires it, and crash recovery can redeliver. Limits are 8,192 bytes per instruction, 20 pending, and 65,536 pending bytes.
- **Stage local merge** commits outstanding worktree changes to the session branch and stages its squash in the base checkout, retaining the session without committing the base.
- **Merge locally & cleanup** makes one base squash commit and removes the worktree, branch and session. A cleanup failure retains the session for retry without another squash. An initial squash requires the recorded target checked out, a clean tracked base and no behind upstream. Cruise does not pull. A cleanup retry requires the recorded source commit, an unchanged branch, and an expected retained worktree with no tracked or untracked changes. Cruise reports conflicts, attempts rollback, and reports rollback failure. These actions exclude `--repo` and current-branch sessions and ignore `cleanup_after_pr` and the `merge` flag.
- **Reset to Planned** clears the current step so the session re-runs from the start — the go-to recovery for a wedged `Running`/`Failed` session.
- **Replan** regenerates the plan from feedback while staying `Planned`.
- **Publish as Issue** publishes `plan.md` verbatim as a GitHub issue (or GitLab issue) in the resolved repo, then deletes the local session. Optionally posts a follow-up `@cruise run` comment to trigger the Actions workflow (default off for `AwaitingApproval`, on for `Planned`, since publishing a `Planned` session replaces running it locally). If the comment fails to post, the issue stays but the local session is kept for a retry, which reuses that issue instead of creating a duplicate (the `@cruise run` comment prompt is skipped on GitLab).

## Resident daemon and CLI clients

Session commands (`plan`, `--plan`, `draft`, `run`, `run --all`, `exec`, `list`, `tell`, `manager`, `clean`, and `diff`) connect to the per-user daemon and start it on demand. `config`, `usage`, `update`, `install`, `workflow *`, `docs`, `ssh`, and every `--dry-run` are local-only. `cruise webui [-o|--open]` ensures the daemon is running, prints the login URL, and optionally opens the browser; `daemon start`, `restart`, and `serve` also print it.

```sh
cruise daemon start [--host <ADDR>] [--port <PORT>] [--webui-dir <DIR>] [-o|--open]
cruise daemon stop [--force]
cruise daemon restart [--host <ADDR>] [--port <PORT>] [--webui-dir <DIR>] [-o|--open]
cruise daemon status [--json]
cruise daemon serve [--host <ADDR>] [--port <PORT>] [--webui-dir <DIR>] [-o|--open]
```

`serve` runs in the foreground for containers or debugging. These options apply to `start`, `restart`, and `serve`. The default address is `127.0.0.1:8484`; if it is busy and no port was requested, Cruise chooses an ephemeral port and warns. Set `--port` or `CRUISE_DAEMON_PORT` explicitly on shared hosts; `0` requests an ephemeral port. Open the login URL printed by `cruise webui`; it contains a single-use nonce that expires after five minutes, not the registry API token. Successful login creates a random in-memory session ID in the HttpOnly, SameSite=Strict `cruise_webui` cookie; the session is separate from the API token and dies with the daemon. The bare origin returns 401. `-o`/`--open` uses a 0600 temporary HTML redirect file, so the login URL is not passed in the opener's command line. Loopback binds accept only `localhost`, `127.0.0.1`, and `[::1]` Host names on any port, blocking DNS-rebinding Host names while allowing SSH tunnels on any local port. Non-loopback binds serve plain HTTP; use them only on trusted networks or behind a TLS proxy.

The registry and log are in `$XDG_STATE_HOME/cruise/daemon/` (normally `~/.local/state/cruise/daemon/`). `instance.json` is mode `0600` and contains a random API token; `daemon.log` contains daemon logs. There is no boot-time service installation or idle auto-exit; the daemon exits when its registry record is removed. The old `cruise webui start|stop|restart|status` commands are removed; use `cruise daemon ...`.

When upgrading from the v0.3.0 detached WebUI, daemon start, restart, serve, and client autostart refuse while its server is live. Run `cruise daemon stop` first; it also stops the legacy server and cancels its active work, leaving sessions `Suspended` for resumption with `cruise run <id>`. `cruise daemon status` reports a live legacy server, and daemon startup removes dead or stale legacy records.

The daemon also refuses (HTTP 409, CLI exit 1, state unchanged) to run or plan a session that another cruise process is still executing as its recorded runner, such as a v0.3.0 `cruise run` in progress during the upgrade. v0.3.0 background planning workers record no runner and cannot be detected, so let background plans started before the upgrade finish first.

The CLI resolves workflow configuration locally and sends its working directory plus allowlisted workflow-override variables (such as `CRUISE_MODEL`, `CRUISE_PLAN_MODEL`, and `CRUISE_SDK`) with each request; they apply only to that request. Process-level variables, including `PATH`, `GH_TOKEN`, provider credentials, `CRUISE_FORGE`, `CRUISE_SKIP_AGENT_AUTH_CHECK`, `CRUISE_DISABLE_NOTIFICATIONS`, and `CRUISE_COMMIT_COAUTHOR_*`, come from the daemon environment. Restart with `cruise daemon restart` after changing them. Autostart removes workflow-override variables from the daemon environment; explicit `daemon start` and `serve` preserve them. WebUI requests use the daemon environment and its home-directory working directory. herdr reporting stays client-side.

When a client finds a daemon running another Cruise version, it restarts it automatically if idle; if busy, it asks you to run `cruise daemon restart` when idle. After `cruise update`, the next command restarts an idle daemon. Foreground commands stream progress; Ctrl+C cancels an operation: runs normally become `Suspended`, but canceling recovery of a failed exec before it enters `Running` leaves it `Failed`. Planning cancellation restores the prior planning state. Closing the terminal does not stop daemon work; reattach with `cruise run <session-id>` or the WebUI. In ordinary sessions, Ask and Option prompts appear in the terminal and WebUI; the first answer wins. If a non-interactive client has a pending prompt, the session waits for an answer in the WebUI or from `cruise run <session-id>`. Manager-owned Ask and Option prompts must be answered on the repository’s Manager page or with `cruise manager answer <GOAL> <PROMPT>`, because the child session page is read-only. `cruise --plan` also runs planning inside the daemon, not a detached worker. Claims prevent duplicate work on one session, so separate CLI processes and the WebUI can safely act on sessions concurrently.
After `cruise update`, a busy daemon keeps running its old version: CLI clients refuse requests with a hint to run `cruise daemon restart` when idle, and the WebUI stays on the old version until restart. An idle daemon is restarted automatically by the next client request. Detached daemons check `daemon.log` every 60 seconds; above 5 MiB, they replace `daemon.log.1` with the current log and truncate the live log, retaining one rotated generation.

## Config-file resolution

`cruise plan`/`exec` resolve the **workflow YAML** in this order. `cruise run` loads the session's tagged `config` reference from `state.json`. A `kind: file` reference follows its absolute live `path`. `kind: builtin_snapshot`, `kind: repo_snapshot`, and `kind: inline_snapshot` references load only `sessions/<id>/config.yaml` and never rediscover or fall back to another config.

1. `-c/--config <path>` (must exist; no prompt). `__builtin__` and `builtin:tdd` select `tdd`; `builtin:<name>` selects a named catalog workflow. The former selectors `builtin:default`, `builtin:simple`, and `builtin:review` are unsupported. `cruise workflow list` prints the catalog; `cruise workflow eject <name> [--to user|project]` copies a catalog workflow without overwriting an existing file.
2. `CRUISE_CONFIG` env var (must exist; no prompt)
3. Current dir: `./cruise.yaml` → `.yml` → `./.cruise.yaml` → `./.cruise.yml`, then `./.cruise/*.yaml|*.yml` (ASCII-sorted, excluding top-level `./.cruise/default.yaml`), then `$XDG_CONFIG_HOME/cruise/workflows/*.yaml|*.yml` and members of valid locked bundles. Candidates are offered in an interactive picker (TTY) or the highest-priority candidate is picked automatically (non-interactive). Application `workflow_categories` affects only interactive picker display, not resolution priority.
4. No config file found → an interactive terminal offers built-in workflows in the selector; non-interactive execution automatically selects the embedded `tdd` workflow (`builtin/tdd.yaml` in the source tree).
The built-in names are `tdd`, `refactor`, `add-tests`, `add-formal-specs`, `research`, `security`, and `pr-review-fix`; use one with `builtin:<name>` to select it.


`$XDG_CONFIG_HOME/cruise/default.yaml` supplies user defaults (when `XDG_CONFIG_HOME` is unset, macOS/Linux use `~/.config/cruise/default.yaml`; Windows uses `%APPDATA%\cruise\default.yaml` when `APPDATA` is set, otherwise it falls back to `~/.config/cruise/default.yaml`); `./.cruise/default.yaml` supplies project defaults and takes precedence over user defaults. Workflow fields override defaults, and environment overrides are applied afterward. Defaults `pre_steps` are command-only and are inserted at the start of the workflow as `pre/<name>`; project entries override same-named user entries. The top-level `.cruise/default.yaml` is not a workflow candidate, and explicitly selecting either defaults file with `-c` or `CRUISE_CONFIG` is rejected. A file such as `workflows/default.yaml` remains an ordinary workflow.

> To *write* or edit that YAML, switch to the **cruise-config** skill.

### Process environment overrides

`CRUISE_UI_LANGUAGE` is a UI setting, not a workflow override. The CLI resolves it before application `uiLanguage`, the system locale, and English. Set it to `en`, `zh-CN`, `es`, `fr`, `pt-BR`, `ja`, `ko`, or `auto`. Case and underscores are accepted for explicit tags. An invalid environment value produces an English warning and is ignored. System detection checks the first nonempty `LC_ALL`, `LC_MESSAGES`, `LANG`, or `LANGUAGE`, without falling through after an unsupported value. Windows also consults the user locale when environment variables do not determine one; macOS and Linux rely on environment variables, so GUI launches without them default to English.

Use `cruise config --set-language ja` to persist an application language and `cruise config --set-language auto` to clear it. `cruise config` reports the effective `ui_language` and source. The WebUI language selector saves a browser cookie, not the application setting, and overrides the server process default for that browser. UI language does not alter `languages.pr` or `languages.plan`.

When loading a workflow config, these process environment variables override the corresponding fields. String values are trimmed and empty values are ignored:

| Variable | Overrides |
|----------|-----------|
| `CRUISE_MODEL` | The workflow's default `model` for prompt steps. |
| `CRUISE_PLAN_MODEL` | The workflow's `plan_model` for the built-in plan step (which otherwise falls back to `model`). |
| `CRUISE_ADVISOR_MODEL` | The workflow's `advisor_model`, a model string or JSON fallback array for advisor steps. |
| `CRUISE_SUBAGENT_MODEL` | `subagent_model`, the subagent model entry. |
| `CRUISE_SDK` | The workflow's SDK backend, which must be `jcode`; it also clears `command` because the two are mutually exclusive. |
| `CRUISE_COMMAND` | `command`, a JSON array; clears `sdk`. |
| `CRUISE_MAX_RETRIES` | `max_retries`, a nonnegative integer. |
| `CRUISE_COMPUTER_USE` | `computer_use`, a boolean. |
| `CRUISE_PERMISSION` | `permission`: `read-only`, `edit`, or `full`. |
| `CRUISE_DISALLOWED_TOOLS` | `disallowed_tools`, a JSON array. |
| `CRUISE_COMMIT_MESSAGE_TEMPLATE` | `commit_message_template`, text. |
| `CRUISE_PR_BODY_TEMPLATE` | `pr_body_template`, text. |
| `CRUISE_RETRY` | `retry`, a JSON object or `null`. |
| `CRUISE_RETRY_BASE_DELAY_MS` | `retry.base_delay_ms`, a nonnegative integer. |
| `CRUISE_RETRY_MAX_DELAY_MS` | `retry.max_delay_ms`, a nonnegative integer. |
| `CRUISE_RETRY_MODEL_FALLBACK` | `retry.model_fallback`, a boolean. |
| `CRUISE_RETRY_FALLBACK_CHAINS` | `retry.fallback_chains`, a JSON object. |
| `CRUISE_LANGUAGES` | `languages`, a JSON object. |
| `CRUISE_LANGUAGE_PR` | `languages.pr`, the language for built-in PR title/body generation. |
| `CRUISE_LANGUAGE_PLAN` | `languages.plan`, the language for built-in planning prompts. |
| `CRUISE_CLEANUP_AFTER_PR` | `cleanup_after_pr`, whether to remove a worktree and branch automatically after a successful PR. |
| `CRUISE_INTERACTIVE_PLANNING` | `interactive_planning`, whether SDK planning uses its interactive planning tools; it has no effect for a `command:` backend. |
| `CRUISE_FORCE_EXEC` | `force_exec`, whether direct plan entry points execute in the current directory. |
| `CRUISE_RUN_ALL_PARALLELISM` | App `run_all_parallelism`, an integer of at least 1 for WebUI run-all, not CLI `run --all`. |

Values are trimmed and blank values ignored. Booleans accept only `true`, `false`, `1`, or `0`, and integers use unsigned decimal digits. Models are literal single models unless beginning with `[` or `{`, which means JSON in the field's normal shape. Model arrays enable fallback as in YAML. Other composite values require JSON, with `null` or `[]` clearing optional values or lists where supported. Parent `retry` and `languages` overrides apply before their children. `CRUISE_SDK` and `CRUISE_COMMAND` cannot both be nonblank. Invalid values are errors. Per-invocation CLI flags and saved session cleanup overrides take precedence where applicable. App parallelism is effective-only and is not saved; CLI `run --all` uses `--parallelism` instead. File-backed sessions reapply overrides on reload, while snapshots retain their creation-time config.

Named maps and secret-bearing `env`, `mcp_servers`, and `pre_steps`, workflow structure (`steps`, `groups`, `after-pr`, `inputs`, `returns`, `description`, `flags`), `create_pr`, and deprecated `pr_language` / `plan_language` are excluded. `formal_verify` is configured through workflow YAML or defaults, not a configuration environment override. See the [configuration override reference](../cruise-config/references/env-and-llm.md#process-level-config-overrides) and the README environment table for details.

Every actual jcode prompt needs an effective Cruise model. Ordinary prompts
resolve step `model` then workflow/default `model`; planning uses
`plan_model` then `model`. If a jcode call has no model, Cruise creates or
preserves the user-level `default.yaml` template and stops before starting
jcode. `JCODE_MODEL` and jcode's configured default do not fill the Cruise
setting. File-backed sessions reload defaults; snapshot sessions do not, so
recreate a snapshot session after changing its model configuration.

These process environment variables are read directly at runtime and are not workflow-config overrides. `CRUISE_RUNTIME` overrides the launcher's runtime executable path; `CRUISE_COMMIT_COAUTHOR_NAME` and `CRUISE_COMMIT_COAUTHOR_EMAIL` append a `Co-authored-by:` trailer when both are set; `CRUISE_DISABLE_NOTIFICATIONS=1` disables desktop, sound, and webhook delivery (not WebUI browser notifications); `CRUISE_NOTIFY_WEBHOOK` supplies the secret Slack Incoming Webhook URL, never a configuration-file value.

`CRUISE_DISABLE_HERDR=1` disables both Herdr integrations. Core uses OSC 7501 when `HC_TERMINAL_ID` is nonempty and a TTY is available; on Unix, Cruise must also be in the foreground process group. It needs no `hc` executable, RPC, or configuration.

Classic Herdr requires `HERDR_ENV=1`, `HERDR_PANE_ID`, and `HERDR_BIN_PATH`; when fully configured, it takes precedence over Core.

Reporting is best-effort and client-side. CLI `cruise plan`, `cruise run`, and `cruise exec`—including Run/Resume from `cruise list`—report `working` and `blocked` (with a reason). Only `cruise list` browsing and action-selection menus are excluded; other CLI prompts, such as `cruise run` workspace selection, report `blocked`. WebUI daemon sessions do not report workflow status.

On exit, Core sends `clear`; classic reports `idle` and releases its pane. Reporting does not affect workflow results or exit status.

`CRUISE_SKIP_AGENT_AUTH_CHECK=1` bypasses the file-only OAuth expiration preflight before session planning/execution (`cruise plan`, `cruise run`, `cruise exec`, and WebUI session planning/execution). Only the exact process-environment value `1` opts out, not `true` or whitespace-padded values. The check covers applicable explicit jcode Claude/OpenAI OAuth candidates, including effective fallbacks and step overrides, not every installed provider. It stops only for expired access tokens with no refresh token or a known expired refresh token. Refreshable expired access tokens pass without warning. Command backends, dry-runs, API-key/unsupported providers, bare jcode models, and unreadable or indeterminate credential files are skipped. This does not validate server-side revocation; step-level `env` API keys are not considered. See [jcode SDK authentication details](../cruise-config/references/sdk.md#startup-oauth-expiration-checks) for credential paths and limitations.

`CRUISE_USAGE_RECORDING=0` or `false` disables local usage recording. Unset, `1`, or `true` enables it; values are trimmed and case-insensitive. Unknown values enable recording with a warning. This is a runtime switch, not a workflow override.

## Runtime file layout (XDG)

| Kind | Path (default) |
|------|----------------|
| User workflow YAML configs (`workflows/*.yaml` / `*.yml`) | `$XDG_CONFIG_HOME/cruise/workflows/` → `~/.config/cruise/workflows/` |
| App settings (`config.json` or `config.yaml`, including `notifications`) | `$XDG_CONFIG_HOME/cruise/` → `~/.config/cruise/` |
| Sessions + worktrees + `--repo` clones | `$XDG_DATA_HOME/cruise/` → `~/.local/share/cruise/` (sessions with no filesystem config path, including `-c __builtin__`, keep a `sessions/<id>/config.yaml` snapshot; the main graph is `sessions/<id>/dag.json` and the after-PR graph is `sessions/<id>/after-pr-dag.json`) |
| State (`history.json`, `new_session_draft.json`, per-installation `installations/<installation-id>/update-transaction.json`) | `$XDG_STATE_HOME/cruise/` → `~/.local/state/cruise/` |
| Daemon registry and log (`instance.json`, `daemon.log`) | `$XDG_STATE_HOME/cruise/daemon/` → `~/.local/state/cruise/daemon/` |
| Manager records (conversation, proposals, goals) | `$XDG_DATA_HOME/cruise/manager/` → `~/.local/share/cruise/manager/` |
| Usage records | `$XDG_STATE_HOME/cruise/usage.ndjson` → `~/.local/state/cruise/usage.ndjson` |

On Windows, when the `XDG_*` variables are unset the defaults are `%APPDATA%\cruise` (config), `%LOCALAPPDATA%\cruise\data` (data) and `%LOCALAPPDATA%\cruise\state` (state); missing variables fall back to the `~/` defaults, and `~\` is expanded like `~/`.

> Older versions kept everything under `~/.cruise/`. If migrating, move workflow YAMLs to `~/.config/cruise/workflows/`, `config.json` to `~/.config/cruise/`, `sessions/`+`worktrees/` to `~/.local/share/cruise/`, and use `git worktree move`/`repair` for worktrees. Cruise also warns if workflow YAMLs are left directly in `~/.config/cruise/` (legacy location) instead of the `workflows/` subdirectory.

## Operational notes & gotchas

- **Forge CLI requirements:** `gh` is required for GitHub PR creation, PR-backed `cruise clean`, and opted-in native workflow merge; use `glab` for GitLab repositories. Current-branch and `exec` do not create or merge a PR.
- **`github-review` after-pr steps** run during `cruise run` after the PR is created. Unlike warning-only after-pr errors, a `github-review` failure marks the session `Failed`; cancellation and missing-model errors from jcode after-pr prompts also fail the run. With native merge opted in, all actual after-pr execution errors are fatal. These steps require `gh` to be authenticated.
- **`cruise clean` also removes eligible no-PR sessions** without calling `gh`, including Planned CurrentBranch sessions with no `current_step` when `plan.md` is missing or contains only whitespace. It retains failed exec sessions. Select **Delete** in `cruise list` to remove a failed exec record without changing working files.
- **`--all`** runs Planned or Suspended sessions sequentially by default, regardless of `cruise config --set-parallelism` (that value governs the **WebUI**). If a session state file cannot be reloaded for the final summary, that session is reported as `Failed` with the state path and error, and the batch still completes.
- **`--parallelism <N>`** is a one-run override for `cruise run --all` (default `1`, must be >= 1, requires `--all`). Each session still runs in its own worktree; one failure does not stop the other workers, and Ctrl+C suspends active sessions and stops new scheduling. It never reads or changes the persisted `cruise config --set-parallelism` value (that value governs the **WebUI**).
- **`--auto-requeue-max-attempts <N>`** overrides the persisted `run_all_auto_requeue_max_attempts` for one Run All batch. Unlike parallelism, CLI Run All reads the persisted value when omitted. Set it with `cruise config --set-auto-requeue-max-attempts <N>` or WebUI settings. It defaults to `0` (disabled) and is sampled at batch start.
- Eligible failed sessions retry from the failed step in the same worker. The persisted count limits automatic retries across batches. Manual Run and Retry do not clear it, while Reset to Planned does. Failed sessions are not selected at batch start, and single-session runs and exec sessions are not automatically retried. Unclassified command errors remain eligible, but cancellation, configuration, authentication and authorization errors are not. Normal failure notifications still appear, and a completed run with failed steps is not reclassified.
- In an interactive terminal, a non-dry `cruise run --all` shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output is retained in `sessions/{id}/run.log`. Non-TTY and dry-run invocations keep the normal log output and final summary.
- **Hot-reload:** during `cruise run`, the config is re-read between steps when its mtime changes — tweak prompts mid-run without restarting (only for external configs, and the current step must still exist; newly declared flags absent from the session's saved values are off).
- **Retries:** Without an SDK fallback policy, HTTP 429 uses exponential backoff (2s → 60s) with `--rate-limit-retries` retries (default 5, i.e. up to 6 attempts); an SDK `retry:` block or workflow-level model array with fallback entries switches a 429 straight to a usable fallback (same-model backoff only when none is left) and also makes 5xx/network failures and HTTP 4xx other than 429, 401, 403, 407, and 408 retryable and can switch to fallback models using the same `--rate-limit-retries` budget — such a 4xx switches straight to the next chain entry instead of being resent to the same model, and surfaces unchanged when no entry is left. Whether the model it left behind is put on the 30-minute cooldown that a 429/5xx/network skip applies depends on what the failure blames: a refused request (`client error`) leaves that model immediately usable, while an error naming the model as absent (`400 model_not_supported`, `404 model_not_found`, `unknown model`) is a `missing model` and does cool it down, so later turns and later steps stop selecting a model the provider does not have. 401/403/407 are auth or permission failures another model cannot fix, so they fail the step immediately; 408 counts as a network failure, so it switches to a usable fallback first like a 5xx (same-model resend only when no fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0`) and does cool the model it left behind down. Rate-limit wording and 5xx are classified ahead of 4xx-looking numbers, so a 429 or 503 keeps its class even when stderr noise in the message contains something 4xx-shaped. Budgeted graph transitions — including ordinary sequential transitions — are bounded by `--max-retries`, resolved as: the explicit flag wins, else the workflow config's top-level `max_retries`, else 3.
- **Stuck session?** `cruise list` → the session → **Reset to Planned** to restart it cleanly, or **Resume** to continue a `Running`/`Suspended` one. For **PR Failed**, fix the publication error and choose **Create PR** instead of restarting the workflow.

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

# Retry eligible failures at most twice for each session
cruise run --all --auto-requeue-max-attempts 2

# Throwaway run against the current branch, no PR
cruise exec "tidy up the imports in src/"

# Preview without executing
cruise run --dry-run

# Feed session state to a script
cruise list --json | jq '.[] | select(.phase=="Failed")'

# Review a session before PR cleanup
cruise diff <session-id>
cruise diff <session-id> --stat

# Garbage-collect merged/closed work
cruise clean
```
