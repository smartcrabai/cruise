<a id="sessions-and-workspaces"></a>
# Sessions and workspaces

**Languages:** [English](../en/sessions.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/sessions.md) · [简体中文](../zh/sessions.md) · [Index](README.md)

<a id="runtime-file-layout"></a>
## Runtime File Layout

Cruise follows the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/) and splits its runtime files across three directories:

| Kind | Path |
|------|------|
| User workflow YAML configs (`workflows/*.yaml` / `*.yml`) | `$XDG_CONFIG_HOME/cruise/workflows/` (default: `~/.config/cruise/workflows/`) |
| User workflow defaults | `$XDG_CONFIG_HOME/cruise/default.yaml` (default: `~/.config/cruise/default.yaml`) |
| Project workflow defaults | `./.cruise/default.yaml` |
| State files, WebUI instance data, and more | `$XDG_STATE_HOME/cruise/` (default: `~/.local/state/cruise/`) |
| Sessions, worktrees, and temporary `--repo` clones | `$XDG_DATA_HOME/cruise/` (default: `~/.local/share/cruise/`) |

On Windows, when the `XDG_*` variables are unset, the defaults are `%APPDATA%\cruise` (config), `%LOCALAPPDATA%\cruise\data` (data), and `%LOCALAPPDATA%\cruise\state` (state). If those variables are not set, the `~/` defaults above apply, and `~\` is expanded like `~/`.

> **Migrating from `~/.cruise/`?** Earlier versions stored everything under `~/.cruise/`. Move `*.yaml` / `*.yml` into `~/.config/cruise/workflows/`, `config.json` into `~/.config/cruise/`, `sessions/` and `worktrees/` into `~/.local/share/cruise/`, and `history.json` and `new_session_draft.json` into `~/.local/state/cruise/`. Use `git worktree move` (or `git worktree repair`) when relocating worktree directories.
>
> **Workflow configs previously in `~/.config/cruise/*.yaml` / `*.yml`?** Automatic user-config discovery now looks for workflow YAML files only in the `workflows/` subdirectory. Move them to `~/.config/cruise/workflows/`; cruise prints a warning if it finds legacy workflow YAML files directly under `~/.config/cruise/`. Application settings `config.yaml` and workflow defaults `default.yaml` can stay in that directory without triggering the warning.

<a id="session-lifecycle"></a>
## Session Lifecycle

1. **`cruise plan "task"`** -- Runs the built-in plan step in an isolated planning worktree to generate an implementation plan, then presents an approve-plan menu when stdin is an interactive TTY; with non-TTY stdin, the plan is auto-approved instead.
2. **`cruise --plan "task"`** -- Creates the session immediately and generates the plan in the background. Review it later from `cruise list`.
3. **`cruise draft "task"`** -- Records the task as a `Draft` session without running the plan step. Use **Generate Plan** from `cruise list` to start planning when you're ready.
4. **Approve-plan menu** -- Choose one of:
   - **Approve** -- Mark the session as ready to run.
   - **Fix** -- Provide feedback; the plan step reruns with your input.
   - **Ask** -- Ask a question; the answer is captured but not displayed, and the menu reappears.
   - **Verify** -- Generate Quint and Alloy specifications, run their checks, and display the report and its interpretation without changing the saved plan. Use **Fix** to revise the plan.
   - **Execute now** -- Skip approval and run immediately.
   - **Publish as Issue** -- Publish `plan.md` as a GitHub issue (or GitLab issue) and delete the local session.

   **Fix Plan**, **Ask About Plan**, and **Verify Spec** in the TUI and **Fix**, **Ask**, and **Verify** in the WebUI are available for a usable plan in either **Awaiting Approval** or **Planned**. Verify is a menu action, not an `/verify` command in Ask. It requires `quint` on PATH and Java 17 or later for Alloy.

   After choosing **Approve** or **Execute now**, declared workflow flags are selected first when no `-f/--flag` was supplied. Then a **step skip selector** lists only steps enabled by those values. Toggle any steps you want to skip for this run. Selections are saved per config file and pre-selected for the next session using the same config. Cancelling either selector returns to the approve-plan menu without saving partial flag changes, approving, or executing the session. After Approve, a later `cruise run` uses saved flags and CLI overrides without a flag selector or another prompt to choose skipped steps (see [Workflow flags](cli-reference.md#cruise-run)).

5. **`cruise run`** -- Without an ID, picks a pending session (Planned, Running, Failed, or Suspended; prompts when several qualify), reuses (or creates) the git worktree under `$XDG_DATA_HOME/cruise/worktrees/<session-id>/`, and executes the workflow. By default it creates a PR when the run leaves branch commits, then runs configured `after-pr` steps. In built-in workflows, `-f after_pr` enables this after-PR sequence without merging; `-f merge` runs the same sequence once, then performs the native synchronous squash merge. Workflow-level `create_pr: false` skips automatic commits, push, PR creation, `after-pr`, and PR-based cleanup, while preserving the normal session, artifacts, and worktree. If publication fails, both repository and local worktree sessions enter **PR Failed**. Use **Create PR** to retry publication without rerunning the workflow.

   PR-backed sessions can be removed with `cruise clean` after the PR is closed or merged. A normal session configured with `create_pr: false` retains its session, artifacts, and worktree without waiting for a PR.

   Eligible local worktree sessions without a PR can instead stage a squash merge while retaining the session, or explicitly merge and clean up the worktree, branch, and session.

<a id="model-and-fallback-notices"></a>
## Model and Fallback Notices

When a visible status or info channel is available, each prompt execution reports the model selected by Cruise there. Every notice is also written to the session's `run.log`.

```text
Model: provider/model:free:xhigh
```

When the retry policy switches models, Cruise reports the transition and the replacement model.

```text
Warning: Fallback: provider/primary -> provider/fallback (503, attempt 1/5)
  Error: API Error: 503 service temporarily unavailable
Model: provider/fallback
```

> **`cruise exec`** is a separate path with a transient lifecycle: it runs in the current directory without planning, worktree creation, or PR creation, and deletes its session when it reaches a terminal state. Paused or interrupted exec sessions can still be resumed by ID. Set `force_exec: true` to enable the same path for direct plan entry points; use `--no-force-exec` to opt out once. See [`cruise exec`](cli-reference.md#cruise-exec).

<a id="cruise-list-actions"></a>
## `cruise list` Actions

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

* Open PR and Merge PR are shown only when the session has a PR URL. Tell is shown only while the running session's runner is alive.

** Local merge actions require an eligible local worktree session without a PR and a recorded target branch. Stage is unavailable once a merge intent or commit has been recorded; legacy sessions without a recorded target are ineligible.

**View Diff** is additionally available in every phase when the session has a worktree branch. The branch may already have been deleted, in which case viewing the diff reports an error.

- **Generate Plan** -- Generate the plan for a `Draft` or `AwaitingInput` session (transitions it to `AwaitingApproval`).
- **Approve** -- Approve the plan and transition the session to the Planned phase.
- **Publish as Issue** -- Publish `plan.md`, unchanged, as a GitHub issue (or GitLab issue) in the resolved repository, then delete the local session.
- **Run / Resume** -- Execute (or continue) the session.
- **Tell** -- Submit an instruction to the running session's next agent prompt. TUI and WebUI also offer Tell during a run.
- **Create PR** -- Retry publication for a **PR Failed** session without rerunning the workflow. First check the displayed error and forge authentication (for GitHub HTTPS credentials, `gh auth setup-git` can configure Git). The session remains **PR Failed** after publication fails or is canceled. A fatal after-PR error moves it to **Failed**. On success, the session completes and configured after-PR steps run only if a PR was created or reused.
- **View Diff** -- Review the session branch's changes and return to the menu. See [`cruise diff`](cli-reference.md#cruise-diff) for comparison and cleanup limitations.
- **Replan** -- Provide feedback to regenerate the plan; the session remains in the Planned phase.
- **Open PR** -- Open the session's pull request in the browser via `gh pr view --web`.
- **Merge PR** -- Human-initiated merge of the session's pull request (also available in the TUI and WebUI). Cruise first shows the PR's state, mergeability, review decision, and each CI check. Then choose `Squash` (the default), `Merge`, or `Rebase` and confirm. This is a GitHub-only route that requires human confirmation. Built-in `-f merge` is a separate native synchronous squash path.
- **Stage local merge** -- Squash the session branch into the base checkout's index without committing or removing the session. Uncommitted worktree changes are committed to the session branch first.
- **Merge locally & cleanup** -- Squash into one base commit, then remove the worktree, branch, and session. An initial squash requires the recorded target branch to be checked out locally, a clean tracked base, and no behind upstream; Cruise does not fetch or pull.
- **Reset to Planned** -- Reset the session back to the Planned phase, clearing the current step and allowing it to be rerun from the beginning.
- **Delete** -- Permanently remove the session.
- **Back** -- Return to the session list.

<a id="workspace-mode"></a>
## Workspace Mode

Execute now from `cruise plan` or `cruise "task"` selects flags before skip choices in the approval menu, unless `-f/--flag` was supplied, and forwards those values without a second execution-time prompt. A later `cruise run [id]`, including Run from `cruise list`, uses saved flags plus any CLI overrides without a flag selector. For example, after `cruise plan -f merge "task"` and Approve, `cruise run <id>` uses the saved `merge` value without prompting. Separately, `cruise exec` and `force_exec` entry points offer the [workflow flag selector](cli-reference.md#cruise-run) on TTY stdin when flags are declared and no `-f/--flag` is supplied; dry runs and non-TTY execution do not prompt.

When `cruise run` starts a new session, it prompts you to choose a workspace mode:

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| Mode | Description |
|------|-------------|
| **Worktree** (default) | Creates an isolated git worktree at `$XDG_DATA_HOME/cruise/worktrees/<session-id>/` (default: `~/.local/share/cruise/worktrees/<session-id>/`). A new branch `cruise/<session-id>-<sanitized-input>` is checked out from HEAD or the configured `base_branch` as described below. Requires `gh` CLI for PR creation (`glab` for GitLab repositories). |
| **Current branch** | Executes directly in the current repository on the active branch. No worktree or PR is created automatically, and no automatic merge runs. If `after_pr` or `merge` is opted in, unavailable after-PR actions are skipped with a reason. |

With `--all`, worktree mode is always used; a single `cruise run` keeps the workspace mode already recorded on the session, and in non-interactive environments (piped stdin) fresh sessions default to worktree mode. Sessions created with `--repo` (or the WebUI repository picker) are always pinned to Worktree mode -- the prompt is skipped and current-branch mode is unavailable, since a PR is the only way changes leave the temporary clone.

<a id="current-branch-mode-constraints"></a>
### Current-branch mode constraints

- For a fresh `cruise run` current-branch session, the working tree must be clean (no uncommitted changes to tracked files; untracked files are ignored). `cruise exec` and `force_exec` sessions may start with a dirty working tree, with a warning; Cruise does not stash, commit, or reset existing changes.
- Requires an attached branch (not detached HEAD).
- On resume, the active branch must match the branch recorded at the start of the session.

<a id="worktree-isolation"></a>
### Worktree isolation

- For PR-backed sessions, the worktree is retained until the PR is closed or merged; run `cruise clean` to delete it. Eligible sessions without a PR can instead use local merge cleanup.
- Set `cleanup_after_pr: true` in the config (or pass `--cleanup-after-pr` at runtime) to automatically delete the local worktree and branch immediately after the PR is created. Use `--no-cleanup-after-pr` to override the config setting and keep them.
- An explicit **Merge locally & cleanup** also removes the eligible local worktree and branch after the squash commit, independently of `cleanup_after_pr`.

New worktrees start at the repository's current HEAD unless `base_branch` is set in user or project `default.yaml`. With a base configured, Cruise prefers the local branch, then `origin/<base>`; with `auto_fetch: true`, it fetches only that branch from `origin` and prefers the updated remote-tracking ref. A failed fetch warns and falls back to local refs; a missing base is an error. The configured base is saved with the session and used as the PR or MR target; without it, the forge CLI chooses its default. Existing worktrees are reused without fetching or resolving the base again.

<a id="copying-files-into-the-worktree"></a>
### Copying files into the worktree

Create a `.worktreeinclude` file in the repo root to copy files or directories into the new worktree before the workflow starts:

```
# .worktreeinclude
.env
.cruise/
secrets/config.yaml
```

Each line is a relative path (file or directory). Absolute paths and `..` traversal are ignored for safety.

<a id="stale-session-detection"></a>
## Stale Session Detection

When `cruise list`, the TUI (`cruise`), or the WebUI session detail view loads a session, a session in the `Running` phase is checked for liveness. If the runner process is no longer alive, the session is automatically transitioned to the `Suspended` phase. This prevents sessions from being stuck in `Running` indefinitely after a crash or forced termination.

Suspended sessions can be resumed from `cruise list` or the TUI, or reset to Planned. The `run --all` command also picks up Suspended sessions alongside Planned ones.

<a id="parallel-session-execution"></a>
## Parallel Session Execution

Run All can automatically retry eligible failures from the failed step within the same worker. Set the non-negative limit in WebUI settings, with `e` in the TUI Run All view, or with `cruise config --set-auto-requeue-max-attempts <N>`. It defaults to `0`, disabling retries. Unclassified command errors, eligible provider errors, and no-file-changes errors may retry, while cancellation, configuration, authentication, and authorization errors do not. An attempt's normal failure notification still appears. Completed runs with failed steps are not reclassified as failures.

The persisted per-session count is a lifetime budget across batches. Manual Run or Retry does not reset it, but Reset to Planned does. Failed sessions are not picked up at batch start, single-session runs and exec sessions are not automatically retried, and lowering the limit does not erase the count. Counts appear in session details, CLI list JSON, and batch summaries.

The WebUI, TUI, and CLI support running multiple sessions concurrently during `run --all`.

- **WebUI/TUI**: The parallelism level is controlled by `run_all_parallelism` in the selected application settings file (`config.json` or `config.yaml` under `$XDG_CONFIG_HOME/cruise/`), configurable via `cruise config --set-parallelism <N>`, defaulting to `1`.
- **CLI**: Pass `cruise run --all --parallelism <N>` for a one-run override (default: `1`, i.e. sequential). The persisted WebUI/TUI setting is neither read nor modified. In an interactive terminal, when `--dry-run` is not set, the CLI also shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output is retained in `sessions/{id}/run.log`. Non-TTY and dry-run invocations keep the normal log output and final summary.

The batch scheduler:
- Seeds from Planned and Suspended sessions.
- Launches up to `N` sessions concurrently.

<a id="new-session-form-persistence"></a>
## New Session Form Persistence

The WebUI and TUI persist two pieces of state across sessions:

- **Draft**: The current contents of the New Session form (task description, config path, working directory, repository, and skipped steps). The TUI saves changes automatically. The WebUI saves them when **Save draft** is clicked. The draft is restored when the form is reopened.
- **History**: A log of past New Session selections. It is used to pre-populate the step skip selector with the most recent choices for each config file and to recall previous working directory / config combinations.
