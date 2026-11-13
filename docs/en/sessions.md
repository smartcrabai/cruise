# Sessions and workspaces

**Languages:** [English](../en/sessions.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/sessions.md) · [简体中文](../zh/sessions.md) · [Index](README.md)

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

   **Fix Plan**, **Ask About Plan**, and **Verify Spec** in the TUI and **Fix**, **Ask**, and **Verify** in the WebUI are available for a usable plan in either **Awaiting Approval** or **Planned**. Verify is a menu action, not an `/verify` command in Ask. It requires `quint` on PATH (0.33.0 recommended) and Java 17 or later for Alloy. Alloy uses `CRUISE_ALLOY_JAR` if supplied; otherwise Cruise downloads and checksum-verifies Alloy 6.2.0 into its data directory. Reports and logs are retained under the session's `formal/<run-id>/` directory until the session is deleted, not in the repository. Each Quint parse, typecheck and simulation has a 60-second limit. Model checks use `formal_verify.model_check_timeout_seconds` (default 900 seconds). Alloy commands run sequentially, so N commands can take up to N times that limit, plus setup and other checks. A passed bounded check is not a proof beyond its modeled scope.

   After choosing **Approve** or **Execute now**, declared workflow flags are selected first when no `-f/--flag` was supplied. Then a **step skip selector** lists only steps enabled by those values (grouped steps appear as a parent with children); no selector is shown when there are no candidates. Toggle any steps you want to skip for this run. Only selections from the visible candidates are persisted per config file in `$XDG_STATE_HOME/cruise/history.json` and pre-selected for the next session using the same config. Cancelling either selector returns to the approve-plan menu without saving partial flag changes, approving, or executing the session. Execute now forwards the approval-time flag values as explicit assignments, suppressing another flag prompt at execution start. After Approve, a later `cruise run` uses its saved flags and CLI overrides without a flag selector and does not ask for skips again (see [Workflow flags](cli-reference.md#cruise-run)).

5. **`cruise run`** -- Without an ID, picks a pending session (Planned, Running, Failed, or Suspended; prompts when several qualify), reuses (or creates) the git worktree under `$XDG_DATA_HOME/cruise/worktrees/<session-id>/`, and executes the workflow. By default it creates a PR when the run leaves branch commits, then runs configured `after-pr` steps. In built-in workflows, `-f after_pr` enables that prelude without merging; `-f merge` enables the same prelude once plus the final native synchronous squash merge. If a successful run has no new branch commits, PR creation is skipped without error (existing branch commits are still eligible), and opted-in after-PR actions are skipped with a reason. Workflow-level `create_pr: false` skips automatic commit, push, PR creation, `after-pr`, and PR-based cleanup, while preserving the normal session, artifacts, and worktree; opted-in actions are skipped with a reason and merge never overrides the no-PR setting. Explicit `if.no-file-changes` failure/retry behavior is unchanged. Publication failures enter **PR Failed** for both repo and local worktree sessions; use **Create PR** to retry publication without rerunning the workflow.

For a confirmed current-head GitHub conflict, `wait-ci` exits promptly after validating the commit and rollup unless CI reports a failure. Failed CI retains its diagnostic and repair path, including waiting for running Actions suites. A stale PR head still observes the propagation grace period. GitHub built-in `wait-ci` steps pin the local `HEAD`. Readiness requires the PR head and CI rollup commit to match that expected commit, an existing `statusCheckRollup.state` of `SUCCESS`, and every GitHub Actions check suite on the commit with a `workflowRun` to be `COMPLETED`. A completed suite with a failing conclusion fails CI; a suite without `workflowRun` (for example, from a third-party app) does not block. Readiness must hold for two consecutive polls; any non-ready poll resets the streak. A PR-head mismatch stays pending for a 90-second grace period to allow API propagation; if it catches up in time, the wait can succeed, but a persistent mismatch after the grace or overall deadline fails with the existing `head_mismatch` diagnostic. The built-in polls every 5 seconds for up to 55 minutes (3300 seconds) within a 60-minute step timeout; unregistered or pending checks are not success. Existing snapshot sessions retain their saved wait command and timeout until explicitly updated; changing a built-in does not rewrite them.
The shared after-PR CI wait allows up to 60 minutes. If checks are still
running at its deadline and no failed rollup is known, it reports pending
without treating it as a code failure or launching repair; native merge
independently rejects non-ready CI. If a GitHub rollup is already failing,
`wait-ci` keeps polling attached Actions suites until they finish or the
deadline; at the deadline it preserves the failure and invokes repair with
the available diagnostics.

The shared repair prompt receives wait-ci stderr, enumerates every
non-success check on the current head, and reads job logs (including canceled
or timed-out jobs). If diagnostics are empty or truncated, it queries the
provider directly. It reproduces CI commands, fixes all in-scope failures in
one pass, and runs the corresponding formatters, linters, and tests before
committing. Unrelated or flaky jobs are rerun once or reported clearly; a
no-change repair does not loop.

Native GitHub squash merge applies the same CI readiness gate: the PR head and an existing rollup commit must match the expected local `HEAD`, the rollup state must be `SUCCESS`, and every GitHub Actions check suite on that commit with a `workflowRun` must be `COMPLETED`; a completed suite with a failing conclusion fails CI, while suites without `workflowRun` do not block. Readiness must hold for two consecutive polls; any non-ready poll resets the streak. A PR-head mismatch remains pending for up to 90 seconds for API propagation; a transient mismatch that catches up can succeed, while one persisting past the grace or overall readiness deadline fails. The built-in `wait-ci` retains its existing `head_mismatch` diagnostic. The merge also waits for GitHub's `UNKNOWN` mergeability to resolve, polling every 2 seconds for up to 10 minutes (a shorter step timeout still applies). This runtime check protects existing saved workflow snapshots without rewriting them. Conflicts, malformed responses, and unmet policy or review requirements still reject the merge; the final merge request remains single-shot and SHA-pinned.

- The conflict rejection in the preceding paragraph excludes confirmed GitHub behind/diverged ancestry and `CONFLICTING`/`DIRTY` mergeability, which are the only retryable readiness outcomes. At merge time, GitHub's compare API checks whether the PR includes the current base. The built-in squash-merge step routes these cases to `follow-up/sync-base`, then through `sync-base` → `resolve-conflict` → `push-after-sync` → `wait-ci` → `fix-ci-error` before retrying. Only native `ResyncRequired` routing from `squash-merge` to its `if.fail` target starts a fresh after-PR pass: it resets `budgeted_traversals` for every after-PR edge except `squash-merge` → target and clears group retry counts, preserving edge traversal history. The resync edge remains capped at `max_retries`; generic `if.fail` retains shared budgets. Worst case: up to `max_retries` resyncs, each pass allowing up to `max_retries - 1` CI-fix iterations; zero retry budget permits no resync. Other readiness failures still fail; this ancestry check and recovery do not change GitLab behavior.

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

> **`cruise exec`** is a separate path with a transient lifecycle: it executes in the current directory without planning, worktree creation, or PR creation, and removes its session after terminal completion. Paused or interrupted exec sessions remain resumable by ID. `force_exec: true` enables the same path for direct plan entry points; use `--no-force-exec` to opt out once. See [`cruise exec`](cli-reference.md#cruise-exec).

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
  A fatal after-PR failure resumes its checkpointed step without rerunning the main workflow or publishing the PR again. Saved flags apply with any explicit `--flag` overrides. Older sessions without an after-PR stage marker retain the legacy behavior of restarting the after-PR steps.
- **Tell** -- Submit an instruction to the running session's next agent prompt. TUI and WebUI also offer Tell for a live run.
- **Create PR** -- Retry publication for a **PR Failed** session without rerunning the workflow. Cruise commits remaining changes, pushes, and reuses an existing PR for the branch or creates one. Check the displayed error and forge authentication first (for GitHub HTTPS credentials, `gh auth setup-git` can configure Git). Publication failure or cancellation keeps the session in **PR Failed** with its worktree and any existing PR URL. A fatal after-PR error leaves it **Failed**. Success completes the session and runs configured after-PR steps only when a PR was created or reused; a no-commits skip completes without running them. Reset to Planned instead restarts the workflow, while Delete removes the session and its workspace.
- **View Diff** -- Review the session branch's changes and return to the menu. See [`cruise diff`](cli-reference.md#cruise-diff) for comparison and cleanup limitations.
- **Replan** -- Provide feedback to re-generate the plan; the session stays in the Planned phase.
- **Open PR** -- Open the session's pull request in the browser via `gh pr view --web`.
- **Merge PR** -- Human-initiated merge of the session's pull request (also available in the TUI and WebUI). Cruise first shows the PR's state, mergeability, review decision, and each CI check. You then choose `Squash` (the default), `Merge`, or `Rebase` and confirm; the confirmation names the chosen method. Cruise runs `gh pr merge <url> --squash|--merge|--rebase` and never adds `--auto`, `--admin`, or `--delete-branch`. If GitHub reports the PR as Closed or Merged afterwards, only that session and its worktree (and any `--repo` clone) are cleaned up, as `cruise clean` would. If the PR is still Open (for example in a merge queue) the session is kept and `cruise clean` can remove it later. A PR that is already Closed or Merged is not merged again: run `cruise clean`. This remains the human-confirmed, GitHub-only route; built-in `-f merge` is a separate native synchronous squash path. See [after-pr](../../skills/cruise-config/references/after-pr.md) for its checks and constraints.
- **Stage local merge** -- Squash the session branch into the base checkout's index without committing or removing the session. Uncommitted worktree changes are committed to the session branch first.
- **Merge locally & cleanup** -- Squash into one base commit, then remove the worktree, branch, and session. Cleanup failure retains the session for retry without repeating the squash. An initial squash requires the recorded target checked out locally, a clean tracked base and no behind upstream; Cruise does not fetch or pull. A cleanup retry checks that the recorded source commit is available, the branch has not changed, and any retained worktree is the expected checkout with no tracked or untracked changes. Conflicts are reported, rollback is attempted, and rollback failure is reported; AI does not resolve them. These actions exclude `--repo` and current-branch sessions and do not depend on `cleanup_after_pr` or the `merge` flag.
- **Reset to Planned** -- Reset the session back to the Planned phase, clearing the current step and allowing it to be re-run from the beginning.
- **Delete** -- Permanently remove the session.
- **Back** -- Return to the session list.

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

## Stale Session Detection

When `cruise list`, the TUI (`cruise`), or the WebUI session detail view loads a session, a session in the `Running` phase is checked for liveness. If the runner process (identified by PID and start time) is no longer alive, the session is automatically transitioned to the `Suspended` phase. This prevents sessions from being stuck in `Running` indefinitely after a crash or forced termination.

Suspended sessions can be resumed from `cruise list` or the TUI, or reset to Planned. The `run --all` command also picks up Suspended sessions alongside Planned ones.

Execution uses a fixed-size directed **Graph**, with one stable node per compiled step (a parallel block remains one unit). Increasing `max_retries` does not expand the graph. The versioned `dag.json` checkpoint is authoritative for the next step, accepted edge traversals, budgeted traversals, group-call retry counts, `{prev.*}` variables and file snapshots. Checkpoints are written atomically before the first step and after accepted transitions, normal completion and loop protection. A persistence failure stops execution before the next side effect. A mid-step interruption retains the pre-step checkpoint, so external command effects are not exactly-once.

Normal resume and prompt/model reloads retain counters. `--max-retries` applies a new ceiling to the saved counts, including when lowered. A rejected transition remains pending and cannot replenish its budget by resuming. Changing the resume position retains counters. Only an explicit restart creates a new execution and archives the previous checkpoint. Removing the current step during hot reload keeps the old workflow rather than silently returning to its start.

The main workflow uses the compatibility filename `dag.json`; after-PR uses the independent `after-pr-dag.json` checkpoint and counters. A checkpointed after-PR failure can resume without rerunning the main workflow or publishing again. The IPC name `get_session_dag` remains. Unversioned legacy graphs are viewable but cannot resume with guessed counters. Missing, corrupt and unknown-version checkpoints stop resume without deleting data. Explicitly restart the session to run from the beginning.

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
