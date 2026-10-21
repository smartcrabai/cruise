---
name: cruise-cli
description: Use when running, operating, or troubleshooting the `cruise` CLI or its default interactive keyboard client — the YAML-driven coding-agent workflow orchestrator that wraps coding-agent CLIs. Covers command selection (plan / --plan / draft / run / exec / list / clean / config / ssh / webui / update / update --status), planning variants (--skip-planning / --grill / --no-interactive-planning / --image / --repo), workflow catalog and flags, bounded `run --all --parallelism` execution, the live dashboard, the TUI's WebUI-domain session workflows, session lifecycle and phases, workspace modes, config resolution, runtime file layout. Trigger whenever the user asks how to start a cruise session, run or resume a workflow, manage or clean sessions, use the TUI, pick a workspace mode, or debug why a session is stuck, failed, or not picked up by `cruise run`.
---

cruise is a CLI that drives coding-agent CLIs (like `claude -p`) through a declarative YAML workflow: **plan → approve → run (write tests → implement → test → review) → usually open a PR → after-pr automation when a PR is created**. This skill is the operator's manual — how to *drive* cruise. For writing the workflow YAML itself, see the **cruise-config** skill.

## Forges

GitHub (`gh`) and GitLab (`glab`) are supported. The forge is detected from the repo host (`github.com`, `gitlab.com`); set `CRUISE_FORGE=github|gitlab` for other hosts. `--repo` accepts `owner/repo`, `group/sub/project` (with `CRUISE_FORGE=gitlab`, host from `GITLAB_HOST`), or a full URL. On GitLab the PR is a merge request and the `@cruise run` issue-trigger toggle is unavailable.

Prompt steps accept `permission: read-only | edit | full` (default `full`; see the **cruise-config** skill). A `read-only` step that changes the workspace fails with "read-only step changed the workspace"; this is snapshot-diff detection, not a sandbox. The command backend rejects restricted modes at validation. The built-in `research` workflow and the inventory/audit/triage phases of `security` require these SDK read-only permissions; security's repair and verification phases use full permission. If neither `command:` nor `sdk:` remains set after workflow and applicable defaults are resolved, Cruise falls back to `sdk: jcode`.

## Generating workflow YAML

`cruise workflow generate "description" --name <name> [--user] [--config <path>]` drafts a new workflow with the backend selected by normal config resolution. It saves `./.cruise/<name>.yaml` (or the user workflows dir with `--user`), never overwrites an existing file, and does not run the result or create a session. Output must be raw YAML; each candidate passes the `exec` preflight, with up to 3 repairs, and nothing is written on failure. Validation does not prove command safety, and the backend keeps its normal repository permissions, so review the YAML before running it.

## Mental model

Work flows through **sessions**, each with a phase. The normal path is:

```
plan/draft  →  [AwaitingInput while an interactive question is pending]  →  AwaitingApproval  →  (approve)  →  Planned  →  run  →  Running  →  Completed  →  clean
```

- A **session** is a unit of work (one task → one plan → one run → usually one PR).
- **AwaitingInput** means SDK planning has persisted an unanswered question. Answering it resumes planning; `cruise list` offers **Generate Plan**, which regenerates the plan and moves the session to `AwaitingApproval`.
- `cruise plan`/`--plan`/`draft` *create* sessions; `cruise run` *executes* them; `cruise list` *manages* them; `cruise clean` *garbage-collects* them.
- `cruise exec` is the **odd one out**: it runs a workflow against the current directory with a **transient session**, no worktree, and no PR. Terminal exec sessions are removed automatically; paused or interrupted sessions remain resumable by ID. `force_exec: true` enables the same path for direct plan entry points; `--no-force-exec` opts out once.
- Bare `cruise` opens the official interactive keyboard client for the full WebUI-domain session workflows; use it for interactive management, while `cruise list` remains available as the CLI selector.

## Which command do I want?

| Goal | Command |
|------|---------|
| Plan a task, then approve it interactively (foreground) | `cruise plan "task"` |
| Plan in the background, review later, return immediately | `cruise --plan "task"` |
| I already wrote the plan myself — skip the LLM planning | `cruise plan --skip-planning "<plan text>"` (or `cruise --plan "…" --skip-planning`) |
| Interview me one question at a time, then write the plan | `cruise plan --grill "task"` (SDK backend (the default) + TTY) |
| Add executable Quint, Alloy, and Gherkin specifications with CI | `cruise exec -c builtin:add-formal-specs "task"` |
| Research with a durable, read-only report | `cruise plan --skip-planning -c builtin:research "question"`; approve if prompted, then `cruise run <session-id>` |
| Audit and safely repair confirmed security defects | `cruise plan -c builtin:security "audit request"`, then `cruise run <session-id>` |
| Disable SDK planning tools and have the agent write `plan.md` directly | `cruise plan --no-interactive-planning "task"` |
| Attach planning images | `cruise plan --image screenshot.png "task"` (repeat `--image` as needed) |
| Target a GitHub repo instead of a local directory | `cruise plan --repo owner/repo "task"` (also with `--plan`) |
| Just capture an idea now, plan later | `cruise draft "task"` |
| Execute or resume a non-exec session (Planned, Running, Failed, or Suspended; prompts when multiple) | `cruise run` |
| Execute a specific session | `cruise run <session-id>` |
| Run a config right here, no plan/worktree/PR | `cruise exec "task"` (or `cruise "task"` with `force_exec: true`; use `cruise plan "ssh"` when the task text is exactly `ssh`) |
| Execute every Planned or Suspended session back-to-back (live dashboard on TTY for non-dry runs) | `cruise run --all` |
| Run all planned or suspended sessions with bounded concurrency (one run) | `cruise run --all --parallelism 4` |
| Manage sessions interactively in the full keyboard client | `cruise` |
| Browse / approve / resume / delete sessions with the CLI selector | `cruise list` |
| Dump session state for scripts | `cruise list --json` |
| Automate, emit JSON, or run in CI | Existing CLI commands, especially `cruise list --json` and `cruise run` |
| Delete sessions whose PR is merged/closed or that are terminal no-PR exec/current-branch remnants | `cruise clean` |
| Run cruise on another machine | `cruise ssh <host> [--cwd <remote-path>] [-- <cruise-args>]` |
| Serve the local browser UI (add `-o`/`--open` to launch the browser) | `cruise webui` |
| Update Cruise itself | `cruise update` |
| Inspect in-progress update transactions across installations (read-only; no network access or changes) | `cruise update --status` |
| Install / update / remove / list a GitHub workflow package (confirms first; `--yes` required without a TTY) | `cruise workflow add owner/repo[/path][@ref]` / `update <name>` / `remove <name>` / `list` |
| Draft a new workflow YAML from a description | `cruise workflow generate "description" --name <name> [--user] [--config <path>]` |
| Show / change app-level settings (e.g. WebUI/TUI parallelism) | `cruise config` |
| Sign the default `jcode` backend in to a provider / inspect what's configured | `jcode login <provider>` / `jcode auth status` (cruise has no login command of its own) |
| See what *would* run without executing | add `--dry-run` to `plan` / `run` / `exec` |

### Updating Cruise

For a supported managed install, check the current version, update, then check again:

```sh
cruise --version; cruise update; cruise --version
```

`cruise update` fetches the latest official stable release and is a no-op when that release is the same as or older than the installed version. Native updates support `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`, and `x86_64-pc-windows-msvc`.

| Installation | Update method |
|---|---|
| Shell or PowerShell installer at canonical `$CARGO_HOME/bin` | `cruise update` |
| Source-built install at canonical `$CARGO_HOME/bin` | `cruise update` installs the official runtime; local source changes/build-time features are not preserved |
| Homebrew | `brew upgrade smartcrabai/tap/cruise` |
| GitHub Action, Docker, or another external install | Use its original distribution method |
| `CRUISE_RUNTIME` override set | Unset it; run update from the managed launcher |

Managed means the executable resolves into canonical `$CARGO_HOME/bin`, not that Cargo’s install ledger claims it. Updating adds a versioned `cruise-runtime-managed-…` runtime and `.json` sidecar, then switches `.cruise-runtime-active`; the fixed `cruise` (`cruise.exe` on Windows) launcher and older runtimes remain. The updater does not run Cargo or modify its install ledger. Homebrew, Action, Docker, and other external installs are not changed by this command. Published versions without the updater—including v0.2.16—cannot update themselves. An updater-capable build can select v0.2.16, but that published runtime cannot perform another native update. Bootstrap by installing an updater-enabled release with its platform installer, or rebuilding/installing that source version; use native updates only while the installed runtime includes the updater.

The updater downloads over HTTPS and checks the release manifest’s SHA-256 against the payload. The checksum comes from the same release publisher; it is not an independent signature.

`cruise update --status` prints recorded transactions for installations in sorted order and is read-only: no network, lock, or repair. Its transaction file is `$XDG_STATE_HOME/cruise/installations/<installation-id>/update-transaction.json` (default `~/.local/state/cruise/installations/<installation-id>/update-transaction.json`; on Windows, `%LOCALAPPDATA%\cruise\state\installations\<installation-id>\update-transaction.json` when `XDG_STATE_HOME` is unset). Status reports the recorded phase/error, not proof of the active runtime. A failed transaction keeps its error; rerun `cruise update` to reconcile an interruption. Failures before activation leave the previous selection, and a failed post-activation launcher probe restores it. If completion/cleanup warns after activation, the update is already applied; inspect status and retry rather than treating it as rolled back.

Older versioned runtimes coexist in `$CARGO_HOME/bin` (Windows default `%USERPROFILE%\.cargo\bin`). To pin an installed version, run that exact sibling with `--version`, confirm its `.json` sidecar matches the current fixed launcher/runtime baseline, and set `.cruise-runtime-active` to its exact basename; do not delete runtime files. The baseline is the fixed launcher/runtime size-and-mtime stamp: a reinstall that changes it invalidates the overlay, while a same-size, same-mtime replacement is not detected. Legacy pointers or pinning an older launcher may still require manual retirement. Before retiring the pointer, confirm the fixed sibling itself runs:

```sh
# macOS / Linux
bin="${CARGO_HOME:-$HOME/.cargo}/bin"
archive="$bin/.cruise-runtime-active.archive"
retired="$bin/.cruise-runtime-active.retired"
test ! -e "$archive" && test ! -e "$retired" &&
  "$bin/cruise-runtime" --version &&
  cp -n "$bin/.cruise-runtime-active" "$archive" &&
  mv -n "$bin/.cruise-runtime-active" "$retired"
```

```powershell
# Windows
$bin = if ($env:CARGO_HOME) { Join-Path $env:CARGO_HOME 'bin' } else { Join-Path $HOME '.cargo\bin' }
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

`cruise ssh <host> [-- <args>]` delegates the entire command to a `cruise` installed on the SSH host. Omit `-- <args>` to start the remote TUI when a PTY is available. `--cwd` and `--cruise-bin` are remote directory and executable paths, and all other arguments after `--` are parsed only by the remote cruise. Use the system OpenSSH configuration for aliases, ports, identity files, ssh-agent, and ProxyJump.

The remote host owns its XDG session/config/state directories, jcode authentication, worktrees, and repository clones. Local and remote session lists are separate. No local config, image, repository, credential, or environment variable is uploaded implicitly. In particular, `--config`, `--image`, and `--cwd` paths must exist or be meaningful on the remote host. The WebUI does not act as a remote session browser.

> **Legacy shortcut:** `cruise "task"` with positional input and no subcommand is treated as `cruise plan "task"`. The `ssh` subcommand reserves the exact legacy input `ssh`; use `cruise plan "ssh"` for that task. Piping (`echo "task" | cruise`) feeds the task on stdin.

## The session lifecycle, step by step

1. **Create a session.**
   - `cruise plan "task"` — runs the built-in plan step in an isolated *planning worktree*; in a non-git directory, planning runs in the base directory without a worktree, and `cruise run` then requires a git repository. It then drops you into the **approve-plan menu** (below). Foreground.
   - `cruise --plan "task"` — creates the session and spawns a detached worker to generate the plan, then returns the session ID immediately. `cruise list` shows it as `Planning`, then `AwaitingApproval` (or `Plan Failed`).
   - `cruise draft "task"` — records the task as a `Draft` with no planning at all. Plan it later via **Generate Plan** in `cruise list`.

2. **Approve the plan.** The approve-plan menu offers:
   - **Approve** → session becomes `Planned`, ready to run.
   - **Fix** → give feedback; the plan step reruns with your input.
   - **Ask** → ask a question; the answer is captured internally, then the menu reappears without displaying it.
   - **Execute now** → skip approval and run immediately.
   - **Publish as Issue** → publish `plan.md` as a GitHub issue (or GitLab issue) and delete the local session (see [Publish as Issue](#cruise-list--phase--available-actions)).

   After **Approve** or **Execute now**, the step-skip selector lets you omit steps for this run; cancelling it returns to the action menu without approving or executing.

3. **Run.** `cruise run` without an ID selects a non-exec `Planned`, `Running`, `Failed`, or `Suspended` session (prompting when several qualify), prompts for a **workspace mode** (below), reuses/creates the worktree, and executes or resumes workflow steps. By default Cruise opens a PR when branch commits are available, then runs configured `after-pr` steps. `-f after_pr` enables that prelude only; `-f merge` enables the same prelude once and adds the final native synchronous squash merge. Both flags are default-off, and the implied prelude does not rewrite `after_pr` in the session's sparse saved flag map. A successful run with no new branch commits skips PR creation without error; any opted-in after-PR action is skipped with a reason when no PR is created. Workflow-level `create_pr: false` skips automatic commit, push, PR creation, `after-pr`, and PR-based cleanup while retaining the normal session, artifacts, and worktree; it never allows `merge` to override the no-PR policy. Configured `if.no-file-changes` failure/retry behavior is unchanged. An actual PR-creation failure makes a `--repo` session `Failed` (retryable), while a plain worktree session logs a warning and completes without a PR, skipping `after-pr`. A paused step leaves the session `Running`; Ctrl+C leaves it `Suspended`. Transient exec sessions are excluded from automatic selection and `run --all`; resume them only with an explicit ID.
   `create_pr` is a workflow-only boolean that defaults to `true`; it is not a CLI option and is rejected in user/project `default.yaml`. Setting it to `false` skips Cruise's automatic commit, push, PR creation, `after-pr`, and PR-based cleanup while preserving the normal session, artifacts, and worktree. If `after_pr` or `merge` was opted in, unavailable after-PR actions are skipped with a reason.

4. **Clean up.** `cruise clean` checks each PR-backed `Completed` session via `gh pr view` (or `glab mr view` on GitLab) and deletes the session + worktree (and any leftover `--repo` clone) once the PR is merged or closed. A normal session with `create_pr: false` retains its session, artifacts, and worktree without waiting for a PR.

### `--skip-planning`

`--skip-planning` skips the planning call: the trimmed input (plus stored attachment paths, when present) is written to `plan.md`; empty/whitespace input is rejected. Foreground TTY use still opens the approval menu. Foreground non-TTY use auto-approves to `Planned`; background `cruise --plan … --skip-planning` also creates a `Planned` session immediately. SDK-mode foreground approval makes a separate title-generation request; background skip-planning and command mode derive the title from `plan.md`. When the resolved local workflow has `force_exec: true`, `--skip-planning` does not opt out: force-exec runs before this flag is handled; only `--no-force-exec`, `--grill`, and `--image` opt out. Such sessions store an empty `input` with `input_as_plan: true` in `state.json`; `plan.md` holds the task text and `{input}` resolves to the current `plan.md` content.

### `--grill` (interview planning)

`cruise plan --grill "task"` turns planning into an interview: the SDK agent asks one question at a time, recommends an answer, and then writes `plan.md`. It requires an SDK backend, an interactive terminal, and `interactive_planning: true`; a non-TTY invocation errors before session creation, while an SDK/tool-less mismatch errors and deletes the new session. It conflicts with `--skip-planning` and affects only the initial plan; Fix/Ask, replan, drafts, and background `--plan` use the standard prompt. The WebUI equivalent is the **"Grill me"** toggle.

### `--no-interactive-planning`

`cruise plan --no-interactive-planning "task"` disables interactive SDK planning for this invocation and asks the agent to write `plan.md` directly. It conflicts with `--grill`; the WebUI equivalent is **Non-interactive planning**. When the resolved local workflow has `force_exec: true`, `--no-interactive-planning` does not opt out: force-exec runs before this flag is handled; only `--no-force-exec`, `--grill`, and `--image` opt out.

### Built-in workflow flags

All six built-ins (`tdd`, `refactor`, `add-tests`, `add-formal-specs`, `research`, `security`) declare `after_pr` and `merge`, all off when absent. `tdd` and `refactor` additionally declare `browser_e2e`, `app_e2e`, and `formal_methods`. `after_pr` runs after-PR automation without merging; `merge` runs the same prelude and then the native synchronous squash merge once. Examples: `cruise plan -c builtin:tdd -f merge "task"` and `cruise plan -c builtin:tdd -f browser_e2e,app_e2e,merge "task"`. `research` remains read-only with `create_pr: false`; merge never overrides that no-PR policy. `browser_e2e` enables browser exercises, assertions, and fixes; `app_e2e` enables applicable mobile and desktop native-app exercises; `formal_methods` enables workflow-level executable Quint, Alloy, and Gherkin specifications plus CI. The fixed builtin order is `tdd`, `refactor`, `add-tests`, `add-formal-specs`, `research`, `security`. `add-tests` inventories consumer-visible behaviors and real journeys with stable `COV`/`GAP` IDs, existing test/assertion evidence, `Covered` / `Partial` / `Missing` status, prioritized actionable gaps, and explicit exclusions/limits without numerical coverage quotas; it writes `test-ci-gap-plan.md`, `test-ci-implementation.md`, and `test-ci-verification.md`, with independent per-ID closure. `research` performs bounded read-only plan → investigate → review → synthesize phases and retains `research/plan.md`, `research/evidence.md`, `research/review.md`, and `research/report.md`; `security` inventories scope, performs parallel code/dependency/delivery audits, triages, repairs confirmed safe defects, and independently verifies each finding, retaining `security/scope.md`, `security/code-findings.md`, `security/dependency-findings.md`, `security/delivery-findings.md`, `security/worklist.md`, `security/repairs.md`, and `security/verification.md`. Research/security reports distinguish evidence from inference, preserve finding IDs and review dispositions, cite sources, and state blocked or unrun work honestly.

### `--image`

`cruise plan --image screenshot.png "task"` attaches a PNG, JPG/JPEG, WebP, or GIF; repeat the flag for multiple files. Interactive plan input also detects dragged/pasted image paths. Cruise copies attachments into the session so paths remain stable and appends those stored paths to the planning input. Image attachments disable `force_exec` for that invocation so planning can consume them.

### `--flag` (workflow flags)

`-f` is an alias for `--flag NAME`, which enables a workflow flag declared in top-level `flags:`; comma-separated names and repeated options enable selected flags. Absent flags are off. Explicit `NAME=on|off|true|false` assignments also work, including `--flag NAME=off` to override a saved enabled value. Resuming a session preserves sparse saved flag values unless explicitly overridden; the implied `after_pr` prelude from `-f merge` does not add or change an `after_pr` entry in that map. CLI commands never prompt for missing flags. Unknown names are rejected, and `--flag` conflicts with `--all`.

### `--repo` (GitHub repo sessions)

`cruise plan --repo owner/repo "task"` (also `cruise --plan "task" --repo owner/repo`) targets a GitHub or GitLab repository instead of the current directory. cruise clones it via `gh repo clone` (or `glab repo clone` on GitLab) into `$XDG_DATA_HOME/cruise/clones/<session-id>/` and uses the clone as the session's base dir, so the normal worktree flow applies. The repo CLI is still required to access/clone the repository even when `create_pr: false`; that setting skips automatic commit, push, PR creation, `after-pr`, and PR-based cleanup but retains the session, artifacts, and worktree/clone. With PR creation enabled, the plan's worktree/clone is normally removed on approval, then recreated by `cruise run` and removed after PR creation. A successful run with no new branch commits skips PR creation without error, while existing commits remain eligible; actual `--repo` PR-creation failures remain retryable `Failed` sessions. Repo sessions are **pinned to worktree mode** (no current-branch option), and only clone-contained or builtin configs are copied to `sessions/<id>/config.yaml`; external `-c` or `CRUISE_CONFIG` paths are referenced in place. The WebUI equivalent is the **Directory / Repository (GitHub or GitLab)** source toggle (repository picker backed by `gh repo list`, or `glab repo list` when `CRUISE_FORGE=gitlab`).

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
| **Current branch** | Runs in place on the active branch. No worktree, auto-PR, or automatic merge. | Quick iterations on the current branch. Fresh normal runs reject **tracked** working-tree changes (untracked files are ignored) and need an **attached branch** (not detached HEAD); `exec`/`force_exec` and the TUI's dirty-tree allowance may start dirty. On resume the branch must match. Opted-in after-PR actions are skipped with a reason because no PR is created. |

A single `cruise run` respects the workspace mode already persisted on the session; the prompt (or, with non-terminal stdin, the Worktree default) applies only to fresh sessions still on the default Worktree mode. `cruise run --all` and `--repo` sessions always force worktree mode (for `--repo` the prompt is skipped entirely). Repo sessions snapshot the resolved workflow config before clone cleanup, so workflow-call expansions remain usable.

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
| **Completed** | Open PR*, Merge PR*, Reset to Planned, Delete, Back |
| **Planning** / **Plan Failed** | Edit Settings, Delete, Back (Approve/Publish as Issue appear only once a non-empty `plan.md` exists and planning has no error) |

\* Open PR and Merge PR show only when the session has a PR URL.

- **Merge PR** previews the PR (state, mergeability, review decision, checks), asks for a method (`Squash` default, `Merge`, `Rebase`), confirms, then runs `gh pr merge <url> <method flag>` (never `--auto`/`--admin`/`--delete-branch`). If the PR is then Closed/Merged, that session and its worktree are removed; if it is still Open (merge queue), the session is kept and `cruise clean` re-checks later. Closed/Merged PRs are not merged again: use `cruise clean`.
- **Reset to Planned** clears the current step so the session re-runs from the start — the go-to recovery for a wedged `Running`/`Failed` session.
- **Replan** regenerates the plan from feedback while staying `Planned`.
- **Publish as Issue** publishes `plan.md` verbatim as a GitHub issue (or GitLab issue) in the resolved repo, then deletes the local session. Optionally posts a follow-up `@cruise run` comment to trigger the Actions workflow (default off for `AwaitingApproval`, on for `Planned`, since publishing a `Planned` session replaces running it locally). If the comment fails to post, the issue stays but the local session is kept for a retry, which reuses that issue instead of creating a duplicate (the `@cruise run` comment prompt is skipped on GitLab).

## `cruise` — interactive keyboard client

```sh
cruise
```

Typical flow: run `cruise`, press `n` to open **New Session**, choose the Source, working directory or repository, and workflow config, then type the task. Press `Ctrl-P` for normal planning, `Ctrl-G` for grill planning, or `Ctrl-U` to use the input directly as the plan, or press `Tab` to answer the remaining questions one at a time and pick the launch mode at the end. After Task, the remaining questions are Images, Skipped steps, declared workflow flags (if any), Workspace (Directory sessions only), Dirty tree when applicable, and Launch. Press `1` to inspect or act on sessions, then `3` to run the planned queue in **Run All**.

Bare `cruise` opens the official keyboard-only client beside the CLI and WebUI. It exposes the WebUI's session-management workflows in a terminal. It requires an interactive TTY on macOS, Linux, or Windows (Ctrl+C is handled on Windows); non-TTY use is not supported. On Windows, desktop notifications are not sent, `cruise ssh` needs the Windows OpenSSH client, herdr reporting is limited, and `command:` steps run through `cmd.exe /C`. GitHub-backed workflows use the external `gh` CLI; current-branch-only work does not require `gh` (`glab` for GitLab repositories).

### TUI screens and workflows

The TUI has exactly three views:

- **Sessions** — Browse the global session list. Each selected session has **Info**, **Graph**, **Plan**, and **Log** detail tabs. The Graph tab shows its node list plus the selected node's dependency and edge details: each `TRANSITIONS` entry appends `[N traversals, M budgeted]` once that edge has been traversed, and a `LAST VISITED` timestamp is shown for a node that has run; Markdown is parsed and styled. The TUI uses its own phase-dependent capability set rather than exposing the complete action matrix from [`cruise list` — phase → available actions](#cruise-list--phase--available-actions). Depending on phase, its session palette can expose **Answer Prompt**, **Fix Plan**, **Ask About Plan**, **Discard**, **Run in Worktree**, **Run on Current Branch**, **Retry**, **Edit Current Step**, and **Cancel**, with TUI labels such as **Publish Issue** and **Open Pull Request**; **Clean**, prompt handling, workspace selection, and URL actions are separate TUI behavior.
- **New Session** — Create a session or draft through a step-by-step dialogue: one question is shown at a time with the answers so far listed above and the remaining questions below. The order is Source (local Directory or GitHub repository), working directory or repository, workflow config, task, images, skipped steps, workflow flags, workspace mode, dirty-tree allowance (current-branch runs only), and finally the launch mode (normal planning, grill planning, input-as-plan, or save as draft). Questions that earlier answers make moot are skipped. Workflow flags start off, and Enter proceeds without requiring a selection. For local Directory sessions, the Workflow config question shows the CLI's prioritized candidates plus **Auto-detect** and all built-in workflows with `tdd` first. GitHub sessions omit caller-local candidates and defer Auto-detect until the repository is cloned. The config editor also accepts a typed path instead of a listed candidate. `Ctrl-P`, `Ctrl-G`, `Ctrl-U`, and `Ctrl-S` start or draft the session from any question with the current answers. Directory and path answers offer completion, and history is recalled with the arrow keys; draft and selection history are retained.
- **Run All** — Run Planned or Suspended sessions with live parallelism, in-app status, and bell feedback. Press `p` to edit the shared WebUI/TUI `run_all_parallelism` setting, type a positive integer, and press `Enter` to save or `Esc` to cancel. The value is persisted in `$XDG_CONFIG_HOME/cruise/config.json` and can be changed while a batch is active; a new limit applies at the next scheduling point and does not cancel workers already running. The CLI's `cruise run --all --parallelism <N>` remains a separate one-run override. Plan and run streams continue while navigating between views.

Ask and Option prompts are handled in the TUI. An SDK planning question pauses its session without a modal: the session is marked **Awaiting Input**, and the question appears in that session's **Plan** tab. Press `o` or choose **Answer Prompt** to open the Plan tab and focus the detail pane, `Enter` to edit, `Enter` again to submit, and `Esc` to keep the draft. Execution-time `Option` requests remain in the modal queue; a single-run Option opens automatically, while Run All shows a queue badge. Multiline questions are shown as adjacent lines, with an inline, session-scoped answer field. PR and Issue URLs are shown as text except that a successful Publish as Issue opens its URL; dedicated URL actions use `open` on macOS, `explorer.exe` on Windows, or `xdg-open` on Linux, while other Markdown links remain textual. CLI-only `config` and `exec` operations remain CLI commands.

The New Session dialogue autosaves its answers 500 ms after a change. Other screen state is ephemeral, except the shared Run All parallelism setting, which is persisted in the app configuration, and manually selected detail tabs, which are retained per session for the duration of the TUI process, including refreshes and planning updates. Only Delete, Discard, Reset to Planned, Publish as Issue, Merge PR (preview modal with method choice, then Enter), Clean, Run All / Cancel Run All, and quitting with active work ask for confirmation; Cancel, Approve, Run, Resume, Retry, Generate Plan, and Open PR execute immediately. In New Session, a non-blank task or at least one image attachment is required and the GitHub source requires a repository; a blank working directory means `.` and a blank workflow config means auto-detect. Cancelling a run moves its session to `Suspended`; cancelling planning restores the prior state and plan. Terminal state is restored on normal exit, panic, SIGTERM, and SIGHUP. Session errors stay in the app and session state; only terminal/root/event-loop failures exit the TUI.

### TUI keyboard map

Keys are fixed and cannot be configured:

| Key | Action |
|-----|--------|
| `1` / `2` / `3` | Switch to Sessions / New Session / Run All |
| `n` | Open New Session at its first question, Source; choose the location and config before typing the task |
| `r` | Refresh |
| `c` | Open the Clean confirmation (Sessions view) |
| `?` | Show help |
| `q` / `Ctrl-C` | Quit; an active quit confirms and then cancels work |
| `Ctrl-P` | Create the New Session from the current answers and start normal planning |
| `Ctrl-G` | Create the New Session from the current answers and start grill planning |
| `Ctrl-U` | Create the New Session from the current answers using the input directly as the plan |
| `Ctrl-S` | Save the New Session answers as a draft |
| `Ctrl-R` | Toggle save / regenerate in the multiline Edit Settings input |
| `Tab` / `Shift-Tab` | Next / previous question (Tab completes a path first when one matches, including workflow config paths); in Sessions, toggle focus between the sidebar and the detail pane |
| Arrow keys / `j` / `k` / `PgUp` / `PgDn` / `Home` / `End` | Navigate lists and choices. In the dialogue, `Up` / `Down` select workflow config candidates, recall recent directories or `gh` repositories, or move through the skipped-step list. In text questions, `Left` / `Right` / `Home` / `End` edit text, and `j` / `k` are entered as text; `j` / `k` navigate non-text choices, and in Sessions, arrow keys follow focus: the sidebar switches sessions (`Right` focuses the detail pane), and the detail pane scrolls and `Left` / `Right` move between detail tabs |
| `[` / `]` / `Left` / `Right` | Move between detail tabs (`Left` / `Right` only outside text questions, where they edit text instead) |
| `a` | Open the action palette |
| `p` | On Run All, edit the shared parallelism setting; `Enter` saves and `Esc` cancels |
| `o` | In Sessions, open the selected session's Ask in its Plan tab. Otherwise handle the Option queue or open a dedicated PR/Issue URL |
| `f` | Follow the log |
| `Enter` | In a Plan Ask, start editing or submit the answer. Otherwise accept the New Session answer and move to the next question, start or draft the session, or insert a task/image newline |
| `Ctrl-Enter` | Advance the current New Session question (required in the task and image editors, where `Enter` inserts a newline); in the multiline Edit Settings dialog, save the dialog |
| `Space` | Toggle the current choice or the highlighted skipped step |
| `Esc` | Leave Plan Ask editing without discarding the draft. Otherwise back one question or, at the first question, return to Sessions |

`Ctrl-P` / `Ctrl-G` / `Ctrl-U` / `Ctrl-S` are swallowed while a modal or confirmation is open: nothing happens until it is dismissed.

### TUI layout, logs, and concurrency

- At **120 or more columns**, the layout uses horizontal splits with a fixed **34-column sidebar** and a detail pane.
- At **80–119 columns**, Sessions and Run All use vertically stacked panes (Sessions sidebar above detail; Run All summary above batch log).
- Below **80x24**, it shows a resize notice.
- `NO_COLOR` is honored, and labels/statuses are never conveyed by color alone.
- Idle updates are event-driven; external state is polled every 3 seconds; active work uses a 100 ms spinner.
- In-memory logs are bounded: the session log keeps the latest 10,000 lines and Run All keeps the latest 2,000. Complete per-session output remains at `$XDG_DATA_HOME/cruise/sessions/<session-id>/run.log` (default `~/.local/share/cruise/sessions/<session-id>/run.log`).
- A child process never shares TUI stdin. Interactive child commands therefore cannot prompt through the TUI. Set `CRUISE_DISABLE_NOTIFICATIONS=1` in the process environment to suppress desktop notifications (only the exact value `1` disables them).
- Distinct sessions may run concurrently within one TUI process, but duplicate work for one session is rejected. Concurrent mutation of the same session by separate cruise processes is unsupported.

Use the CLI as the canonical client for automation, JSON, and CI/non-interactive use. `cruise list` and all existing CLI commands remain documented below; `cruise list --json` is the machine-readable session-state interface.

## Config-file resolution

`cruise plan`/`exec` resolve the **workflow YAML** in this order. `cruise run` loads the session's tagged `config` reference from `state.json`. A `kind: file` reference follows its absolute live `path`. `kind: builtin_snapshot`, `kind: repo_snapshot`, and `kind: inline_snapshot` references load only `sessions/<id>/config.yaml` and never rediscover or fall back to another config.

1. `-c/--config <path>` (must exist; no prompt). `__builtin__` and `builtin:tdd` select `tdd`; `builtin:<name>` selects a named catalog workflow. The former selectors `builtin:default`, `builtin:simple`, and `builtin:review` are unsupported. `cruise workflow list` prints the catalog; `cruise workflow eject <name> [--to user|project]` copies a catalog workflow without overwriting an existing file.
2. `CRUISE_CONFIG` env var (must exist; no prompt)
3. Current dir: `./cruise.yaml` → `.yml` → `./.cruise.yaml` → `./.cruise.yml`, then `./.cruise/*.yaml|*.yml` (ASCII-sorted, excluding top-level `./.cruise/default.yaml`), then `$XDG_CONFIG_HOME/cruise/workflows/*.yaml|*.yml`. Multiple candidates → interactive picker with the built-in `tdd` entry first (TTY) or highest-priority auto-pick (non-interactive).
4. None found → the built-in `tdd` workflow (`builtin/tdd.yaml` in the source tree, embedded at build time), adopted without prompting.
The exact built-in names, in catalog order, are `tdd`, `refactor`, `add-tests`, `add-formal-specs`, `research`, and `security`; use one with `builtin:<name>` to select it.


`$XDG_CONFIG_HOME/cruise/default.yaml` supplies user defaults (when `XDG_CONFIG_HOME` is unset, macOS/Linux use `~/.config/cruise/default.yaml`; Windows uses `%APPDATA%\cruise\default.yaml` when `APPDATA` is set, otherwise it falls back to `~/.config/cruise/default.yaml`); `./.cruise/default.yaml` supplies project defaults and takes precedence over user defaults. Workflow fields override defaults, and environment overrides are applied afterward. Defaults `pre_steps` are command-only and are inserted at the start of the workflow as `pre/<name>`; project entries override same-named user entries. The top-level `.cruise/default.yaml` is not a workflow candidate, and explicitly selecting either defaults file with `-c` or `CRUISE_CONFIG` is rejected. A file such as `workflows/default.yaml` remains an ordinary workflow.

> To *write* or edit that YAML, switch to the **cruise-config** skill.

### Process environment overrides

When loading a workflow config, these process environment variables override the corresponding fields. String values are trimmed and empty values are ignored:

| Variable | Overrides |
|----------|-----------|
| `CRUISE_MODEL` | The workflow's default `model` for prompt steps. |
| `CRUISE_PLAN_MODEL` | The workflow's `plan_model` for the built-in plan step (which otherwise falls back to `model`). |
| `CRUISE_SDK` | The workflow's `sdk` backend; it also clears `command` because the two backends are mutually exclusive. |
| `CRUISE_LANGUAGE_PR` | `languages.pr`, the language for built-in PR title/body generation. |
| `CRUISE_LANGUAGE_PLAN` | `languages.plan`, the language for built-in planning prompts. |
| `CRUISE_CLEANUP_AFTER_PR` | `cleanup_after_pr`, whether to remove a worktree and branch automatically after a successful PR. |
| `CRUISE_INTERACTIVE_PLANNING` | `interactive_planning`, whether SDK planning uses its interactive planning tools; it has no effect for a `command:` backend. |
| `CRUISE_FORCE_EXEC` | `force_exec`, whether direct plan entry points execute in the current directory. |

The boolean overrides (`CRUISE_CLEANUP_AFTER_PR`, `CRUISE_INTERACTIVE_PLANNING`, and `CRUISE_FORCE_EXEC`) accept only `true`, `false`, `1`, or `0` after trimming; any other non-empty value is an error. The string overrides above are applied only when their trimmed value is non-empty.

Every actual jcode prompt needs an effective Cruise model. Ordinary prompts
resolve step `model` then workflow/default `model`; planning uses
`plan_model` then `model`. If a jcode call has no model, Cruise creates or
preserves the user-level `default.yaml` template and stops before starting
jcode. `JCODE_MODEL` and jcode's configured default do not fill the Cruise
setting. File-backed sessions reload defaults; snapshot sessions do not, so
recreate a snapshot session after changing its model configuration.

These process environment variables are read directly at runtime and are not workflow-config overrides. `CRUISE_RUNTIME` overrides the launcher’s runtime executable path; `CRUISE_COMMIT_COAUTHOR_NAME` and `CRUISE_COMMIT_COAUTHOR_EMAIL` append a `Co-authored-by:` trailer when both are set; `CRUISE_DISABLE_NOTIFICATIONS=1` disables desktop notifications; `CRUISE_DISABLE_HERDR=1` disables herdr reporting. When `HERDR_ENV=1`, `HERDR_PANE_ID`, and `HERDR_BIN_PATH` are set, the TUI and `cruise plan` / `cruise run` / `cruise exec` report lifecycle states to herdr.

## Runtime file layout (XDG)

| Kind | Path (default) |
|------|----------------|
| User workflow YAML configs (`workflows/*.yaml` / `*.yml`) | `$XDG_CONFIG_HOME/cruise/workflows/` → `~/.config/cruise/workflows/` |
| App settings (`config.json`) | `$XDG_CONFIG_HOME/cruise/` → `~/.config/cruise/` |
| Sessions + worktrees + `--repo` clones | `$XDG_DATA_HOME/cruise/` → `~/.local/share/cruise/` (sessions with no filesystem config path, including `-c __builtin__`, keep a `sessions/<id>/config.yaml` snapshot; the execution graph is persisted as `sessions/<id>/dag.json`) |
| State (`history.json`, `new_session_draft.json`, per-installation `installations/<installation-id>/update-transaction.json`) | `$XDG_STATE_HOME/cruise/` → `~/.local/state/cruise/` |

On Windows, when the `XDG_*` variables are unset the defaults are `%APPDATA%\cruise` (config), `%LOCALAPPDATA%\cruise\data` (data) and `%LOCALAPPDATA%\cruise\state` (state); missing variables fall back to the `~/` defaults, and `~\` is expanded like `~/`.

> Older versions kept everything under `~/.cruise/`. If migrating, move workflow YAMLs to `~/.config/cruise/workflows/`, `config.json` to `~/.config/cruise/`, `sessions/`+`worktrees/` to `~/.local/share/cruise/`, and use `git worktree move`/`repair` for worktrees. Cruise also warns if workflow YAMLs are left directly in `~/.config/cruise/` (legacy location) instead of the `workflows/` subdirectory.

## Operational notes & gotchas

- **Forge CLI requirements:** `gh` is required for GitHub PR creation, PR-backed `cruise clean`, and opted-in native workflow merge; use `glab` for GitLab repositories. Current-branch and `exec` do not create or merge a PR.
- **`github-review` after-pr steps** run during `cruise run` after the PR is created. Unlike warning-only after-pr errors, a `github-review` failure marks the session `Failed`; cancellation and missing-model errors from jcode after-pr prompts also fail the run. With native merge opted in, all actual after-pr execution errors are fatal. These steps require `gh` to be authenticated.
- **`cruise clean` also removes terminal no-PR exec/current-branch sessions** without calling `gh`, plus Planned CurrentBranch sessions with no `current_step` when `plan.md` is missing or contains only whitespace; resumable sessions and ordinary planned sessions are retained.
- **`--all`** runs Planned or Suspended sessions sequentially by default, regardless of `cruise config --set-parallelism` (that value governs the **WebUI and TUI**). If a session state file cannot be reloaded for the final summary, that session is reported as `Failed` with the state path and error, and the batch still completes.
- **`--parallelism <N>`** is a one-run override for `cruise run --all` (default `1`, must be >= 1, requires `--all`). Each session still runs in its own worktree; one failure does not stop the other workers, and Ctrl+C suspends active sessions and stops new scheduling. It never reads or changes the persisted `cruise config --set-parallelism` value (that value governs the **WebUI and TUI**).
- In an interactive terminal, a non-dry `cruise run --all` shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output is retained in `sessions/{id}/run.log`. Non-TTY and dry-run invocations keep the normal log output and final summary.
- **Hot-reload:** during `cruise run`, the config is re-read between steps when its mtime changes — tweak prompts mid-run without restarting (only for external configs, and the current step must still exist; newly declared flags absent from the session's saved values are off).
- **Retries:** Without an SDK fallback policy, HTTP 429 uses exponential backoff (2s → 60s) with `--rate-limit-retries` retries (default 5, i.e. up to 6 attempts); an SDK `retry:` block or workflow-level model array with fallback entries also makes 5xx/network failures and HTTP 4xx other than 429, 401, 403, 407, and 408 retryable and can switch to fallback models using the same `--rate-limit-retries` budget — such a 4xx switches straight to the next chain entry instead of being resent to the same model, and surfaces unchanged when no entry is left. Whether the model it left behind is put on the 30-minute cooldown that a 429/5xx/network skip applies depends on what the failure blames: a refused request (`client error`) leaves that model immediately usable, while an error naming the model as absent (`400 model_not_supported`, `404 model_not_found`, `unknown model`) is a `missing model` and does cool it down, so later turns and later steps stop selecting a model the provider does not have. 401/403/407 are auth or permission failures another model cannot fix, so they fail the step immediately; 408 counts as a network failure, so it switches to a usable fallback first like a 5xx (same-model resend only when no fallback is left, visible text was already streamed, or `--rate-limit-retries` is `0`) and does cool the model it left behind down. Rate-limit wording and 5xx are classified ahead of 4xx-looking numbers, so a 429 or 503 keeps its class even when stderr noise in the message contains something 4xx-shaped. Budgeted graph transitions — including ordinary sequential transitions — are bounded by `--max-retries`, resolved as: the explicit flag wins, else the workflow config's top-level `max_retries`, else 3.
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
