<a id="sessions-and-workspaces"></a>
# Sessions and workspaces

**Languages:** [English](../en/sessions.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/sessions.md) · [简体中文](../zh/sessions.md) · [Index](README.md)

## Experimental Manager records

`cruise manager` requires a local Git repository. Records are stored under `$XDG_DATA_HOME/cruise/manager/` (normally `~/.local/share/cruise/manager/`), keyed by the canonical Git common directory, so linked worktrees share a record. `cruise manager status` displays saved messages and any proposal or goals. Conversation uses the configured interactive SDK and planning model from `builtin:tdd` defaults, not a command backend. A user turn is persisted before the provider call and remains interrupted if it does not complete. The SDK is requested to use read-only permissions in a scratch directory; this is not a hard sandbox. Each proposal freezes the child workflow: `builtin:tdd` resolved with project and user defaults and the daemon environment's overrides, with automatic PR creation, after-PR steps, the `after_pr` and `merge` flags, post-PR cleanup, and `force_exec` removed, because Manager publishes the Goal's single PR/MR itself. Unlike session commands, `cruise manager` does not forward the client's workflow-override variables such as `CRUISE_MODEL`.

Run `cruise manager` or `cruise manager status` to inspect the repository snapshot, or add `--json` to `status` for scripts. `cruise manager say [TEXT]` accepts a message argument, or reads multiline input from a terminal or piped stdin when omitted. `say` and `propose` normally wait and exit non-zero if the turn does not complete; `--no-wait` prints the turn ID and returns immediately. Ctrl-C stops waiting without cancelling the daemon turn.

`cruise manager propose` requests a validated proposal preview; its target branch must resolve to a local `origin` tracking ref, so fetch it first if necessary. `cruise manager approve` displays the proposal, including its executable checks and contract, before approval. On a terminal without `--yes`, type exactly `approve` to confirm. Without a terminal, both `--yes` and `--revision N` are required. Any supplied revision must match the proposal you reviewed. Approval registers one durable active Goal for the repository and target branch, authorizing its daemon coordinator to schedule eligible children asynchronously. The approval action itself does not run the children. Each child starts from the exact integrated Goal tip pinned at dispatch; successful results integrate serially, then the combined exact SHA is verified before Cruise publishes or recovers the Goal's single PR/MR. Manager never merges: it waits for a human to merge the verified head into the approved target. `cruise manager pause GOAL` and `cruise manager resume GOAL` record Goal-scoped state, and `cruise manager answer` accepts a matching live child question or option prompt when one exists. `cruise manager resume GOAL TASK` requests continuation of the latest eligible failed or suspended child in its existing worktree. `cruise manager retry GOAL TASK` instead reserves a new attempt for an eligible previously failed or suspended child of an active Goal, or for a completed child of a blocked Goal when an integration conflict is safely established. Eligibility also depends on dependency, ownership, repository, and other safety checks. A reserved attempt with no published session is pinned again to the current Goal tip at dispatch, so another child integrating first does not block it; a reservation whose session was already published still blocks the Goal for review. If an earlier version blocked the Goal with `interrupted Manager reservation no longer matches Goal tip`, run `cruise manager retry GOAL TASK` again for that task: it reactivates the Goal without reserving another attempt, and the pending reservation starts once from the current tip. `cruise manager retry GOAL` requests a fresh verification for an eligible blocked Goal whose tasks are integrated, preserving prior evidence; it does not retry an uncertain publication. Any unapproved corrective preview is discarded and kept in the Goal’s audit history, and can no longer be approved. `cruise manager corrective GOAL TASK` prompts for human-authored instructions and saves a preview for an eligible definite failed check, without authorizing execution. `cruise manager approve-corrective GOAL` refreshes and displays the goal and preview, then applies the same confirmation and revision rules as `cruise manager approve`. Approval applies only to that preview and integrated tip, adds the task, and preserves prior verification evidence and approved checks. A changed preview or tip requires fresh review. The execution path requires Git 2.38 or newer and an authenticated GitHub or GitLab `origin`.

For GitHub Goals, the PR title keeps the Objective as-is through 256 Unicode characters. Longer titles use the first 255 characters and an ellipsis; the full Objective is appended to the PR description. The approved proposal, contract, and checks do not change. For an older Goal blocked in `CreateIntent` with the recorded GitHub `Title is too long` rejection and a successful no-PR lookup, run `cruise manager retry GOAL` explicitly. Cruise rechecks the saved evidence and exact verification tip and reconciles all PR states with authenticated GitHub. It adopts a matching PR without creating another. Otherwise it submits the shortened title once, only if the approved remote base and Goal branch still point to their recorded SHAs. Missing rejection or verification evidence, moved refs or a conflicting PR, and unavailable GitHub observations leave the record unchanged. Timeouts, permission errors and other uncertain creates cannot be retried this way.

The legacy retry recognizes only the exact `gh` title rejection, with or without the `pull request create failed:` wrapper. Other errors that mention the rejection are not eligible.

`cruise manager corrective` also previews corrections to an approved Goal that add no task. Each becomes the Goal's single pending preview and authorizes nothing. `cruise manager approve-corrective GOAL` applies it only if the displayed revision, the Goal tip, and the contract are still the ones reviewed; otherwise preview it again. These previews are created with the CLI; the WebUI shows the pending one and can approve it.

`cruise manager corrective GOAL --retarget` is for a verified, unpublished GitHub Goal blocked by the definite legacy title rejection, with all tasks integrated. If the same remote target branch has advanced by fast-forward and the remote Goal ref still matches the verified tip, Cruise checks all PR states with authenticated GitHub and previews the exact old and new base, Goal tip, contract, rejected publication evidence, incoming commits, changed files, and merge conflicts. Previewing does not update refs or authorize work. A conflicting preview cannot be approved.

A task's latest attempt must be integrated. Earlier attempts superseded by that integration stay in the audit history and do not block retarget solely because they have a saved result that was never integrated. Unresolved effects, pending resumes, and other pending corrections still block retarget.

Approve that exact revision with `cruise manager approve-corrective GOAL`. Cruise rechecks both remote refs and authenticated PR absence, records the approval and merge intent, then updates the Goal once with a commit that retains both parents. The original proposal, prior approvals, attempts and verification evidence remain in the record; the new base is tracked separately. Cruise reruns every approved check in order, then checks the pinned remote base, expected head and authenticated PR absence again before each remote write. It uses a regular non-force push and the existing shortened title. Drift, missing evidence and uncertain effects stop the Goal; an uncertain push or create is observed on restart, not sent again. No child restarts, merges, or deploys are authorized.

`cruise manager corrective GOAL --amend FILE` narrows the wording of criteria or task instructions. FILE is a JSON array of edits, for example `[{"criterion": "C1", "replace": "old passage", "with": "new passage"}, {"task": "T2", "replace": "…", "with": "…"}]`. `replace` must occur exactly once in the current text. An edit may add `old_sha256`, the SHA-256 of the text it was written against, and is refused when the current text differs; the preview shows the digest the daemon recorded. Checks, IDs, and dependencies cannot be amended. Approved amendments are stored beside the proposal, which is never rewritten. Existing attempts keep the plan they were given. Children reserved afterwards and the final verification use the amended contract, and the PR/MR description lists every amendment with the digest of the text it replaced. A Goal that is verifying or already published cannot be amended.

`cruise manager corrective GOAL TASK --resume` resumes the task's failed attempt in its own session and worktree, from the step that failed and with uncommitted work kept. The instructions, and any contract text amended since the attempt's plan was written, are appended to that plan as an approved correction. The preview records the attempt, session, base, failed step, worktree `HEAD`, and a digest of the worktree's uncommitted and untracked files. Approval, and the launch after it, refuse the correction if any of these, the Goal tip, or the contract changed; the Goal reason then asks for a new preview, and `resume GOAL TASK` is refused until the correction is approved again or the task is retried. `resume` and `retry` never deliver an unapproved correction, and the failed step's `if.no-file-changes` rule still applies to the resumed run. If the daemon stops after the corrected resume started, the Goal is blocked for review; the run is not repeated.

`cruise manager corrective GOAL TASK --evidence --criterion ID --path FILE` (both options repeatable) is for a failed task whose requirement the Goal tip already satisfies. The preview records the blob of each reviewed file at the tip and every approved check of the selected criteria; this set cannot be narrowed. After approval the daemon runs those checks in a detached worktree at exactly that tip. The task is resolved, with no commit and without being marked integrated, only when every check passes and the tip, contract, and attempt are still the reviewed ones. Its dependents can then start, and the final verification still runs every approved check at the final tip. A failed or interrupted check, a daemon stop during the run, or a tip that moved leaves the run recorded as unresolved and the task failed. Runs are never repeated automatically; preview and approve again to try at the current tip. The PR/MR description lists tasks resolved this way.

For an Ask prompt, supply `cruise manager answer GOAL PROMPT TEXT` or enter multiline text interactively. For an Option prompt, terminal stdin uses the menu. Non-terminal stdin requires an exact `--choice LABEL` and `--text TEXT` only for choices that require text input. Supply corrective instructions with `--instructions TEXT`, via the terminal prompt, or through piped stdin.

In the WebUI, select **Manager** in the sidebar and choose a repository, or use **Open repository** with a local Git checkout path on the daemon host. Opening a repository requires both an existing Manager record and an accessible main Git checkout on the daemon host, even if you enter a linked worktree path. Opening a checkout does not create a record or start work. The WebUI and CLI share the same conversation, proposals, and goals. Send messages and request a proposal from the repository page. Review the target commit, tasks, and executable checks, then explicitly confirm the displayed revision to authorize execution. The page updates automatically and shows eligible controls for pausing or resuming a goal, resuming or retrying a task, retrying verification, previewing and approving a corrective task, and answering pending Ask or Option prompts. Corrective previews do not authorize execution. Manager-owned child session pages remain read-only. **Open in Manager** returns to the repository’s Manager page and appears only when Cruise finds a readable Manager record containing the child’s Goal.

<a id="runtime-file-layout"></a>
## Runtime File Layout

Cruise follows the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/) and splits its runtime files across three directories:

| Kind | Path |
|------|------|
| User workflow YAML configs (`workflows/*.yaml` / `*.yml`) | `$XDG_CONFIG_HOME/cruise/workflows/` (default: `~/.config/cruise/workflows/`) |
| User workflow defaults | `$XDG_CONFIG_HOME/cruise/default.yaml` (default: `~/.config/cruise/default.yaml`) |
| Project workflow defaults | `./.cruise/default.yaml` |
| State files, WebUI instance data, and more | `$XDG_STATE_HOME/cruise/` (default: `~/.local/state/cruise/`) |
| Daemon registry and log (`instance.json`, `daemon.log`) | `$XDG_STATE_HOME/cruise/daemon/` (default: `~/.local/state/cruise/daemon/`) |
| Sessions, worktrees, and temporary `--repo` clones | `$XDG_DATA_HOME/cruise/` (default: `~/.local/share/cruise/`) |
| Manager records | `$XDG_DATA_HOME/cruise/manager/` (default: `~/.local/share/cruise/manager/`) |

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

   The WebUI's **Fix**, **Ask**, and **Verify** actions are available for a usable plan in either **Awaiting Approval** or **Planned**. Verify is a menu action, not an `/verify` command in Ask. It requires `quint` on PATH and Java 17 or later for Alloy.

   After choosing **Approve** or **Execute now**, declared workflow flags are selected first when no `-f/--flag` was supplied. Then a **step skip selector** lists only steps enabled by those values. Toggle any steps you want to skip for this run. Selections are saved per config file and pre-selected for the next session using the same config. Cancelling either selector returns to the approve-plan menu without saving partial flag changes, approving, or executing the session. After Approve, a later `cruise run` uses saved flags and CLI overrides without a flag selector or another prompt to choose skipped steps (see [Workflow flags](cli-reference.md#cruise-run)).

5. **`cruise run`** -- Without an ID, picks a pending session (Planned, Running, Failed, or Suspended; prompts when several qualify), reuses (or creates) the git worktree under `$XDG_DATA_HOME/cruise/worktrees/<session-id>/`, and executes the workflow. By default it creates a PR when the run leaves branch commits, then runs configured `after-pr` steps. Among built-ins, `tdd` is the only one whose `-f after_pr` or `-f merge` creates a PR: `-f after_pr` enables this after-PR sequence without merging; `-f merge` runs the same sequence once, then performs the native synchronous squash merge. `research` declares these flags but creates no PR, and `refactor`, `add-tests`, `add-formal-specs`, and `security` declare neither flag. Workflow-level `create_pr: false` skips automatic commits, push, PR creation, `after-pr`, and PR-based cleanup, while preserving the normal session, artifacts, and worktree. If publication fails, both repository and local worktree sessions enter **PR Failed**. Use **Create PR** to retry publication without rerunning the workflow.

   Before a fresh workflow run, Cruise checks CI on the configured `base_branch` or the forge's default branch on GitHub or GitLab. Failing CI or an inability to retrieve its status stops the run, while pending or absent checks produce a warning and allow it to continue. This check does not apply to CLI dry runs, `exec`, resumed workflows, after-PR resumes, or repositories whose origin cannot be resolved to a supported forge. Set `check_base_ci: false` in `default.yaml` to opt out.

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

`cruise exec` runs in the current directory without planning, worktree creation, or PR creation. A successful run removes the session. A failed run retains the same ID for explicit recovery or deletion. Paused or interrupted exec sessions remain resumable by ID. This lifecycle also applies to `--instant` and `force_exec`. See [the exec command reference](cli-reference.md#cruise-exec). Use `--no-force-exec` to opt out of `force_exec` once.

Failed exec sessions are visible in `cruise list` but not in WebUI inventory, automatic `cruise run` selection, or `cruise run --all`. Their menu offers **Resume**, **Delete**, and **Back** instead of ordinary Failed actions. In `cruise list --json`, these rows add `"exec": true` and have `"plan_available": false`. Ordinary rows keep their existing shape.

The WebUI offers **Exec** for the Directory source, or **Execute** when `force_exec` is enabled. Use **Create draft** to plan instead. Running and Suspended exec sessions appear in the sidebar, and Suspended sessions can be resumed there.

<a id="cruise-list-actions"></a>
## `cruise list` Actions

The interactive picker stays open even when there are no sessions. The following actions don't require selecting a session:

- **New Session** uses the current directory as the session’s base directory and follows the normal workflow configuration lookup. Choose **Plan interactively** to enter a task and review the generated plan, **Plan in background** to let the daemon generate the plan while you return to the picker, or **Save draft** to record the task without running the planner. Cancelling task entry creates no session. Both planning modes require an explicit decision before running the workflow, even if the configuration sets `force_exec`.
- **Clean** uses the same cleanup rules as `cruise clean`. It asks for confirmation (default: No), then returns to the refreshed picker, even if it removed the last visible session.
- **Run All** runs eligible Planned and Suspended sessions using the same defaults as `cruise run --all`. If a batch is already active, it offers to attach to it instead. Both prompts default to No. If there's no batch and no eligible work, it shows a message and returns to the picker.
- **Quit**, Esc, or Ctrl+C exits from the top-level picker. Cancelling a nested menu returns to the picker.

The interactive session list shows a menu of actions depending on the session's phase:

Manager-owned child sessions are an exception: their terminal list controls offer only **View Diff** when available and **Back**. Their WebUI session pages are read-only. They link back via **Open in Manager** only when Cruise finds a readable Manager record containing the child’s Goal. Use the Manager page or `cruise manager` subcommands for supported child actions.

| Phase | Available Actions |
|-------|-------------------|
| **Draft** | Generate Plan, Delete, Back |
| **AwaitingInput** | Generate Plan, Delete, Back |
| **AwaitingApproval** | Approve, Publish as Issue, Edit Settings, Delete, Back |
| **Planned** | Run, Publish as Issue, Edit Settings, Replan, Delete, Back |
| **Running** | Resume, Tell*, Reset to Planned, Delete, Back |
| **Suspended** | Resume, Edit Settings, Reset to Planned, Delete, Back |
| **Failed** (non-exec) | Run, Edit Settings, Reset to Planned, Delete, Back |
| **Failed** (inactive exec) | Resume, Delete, Back |
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
- **Tell** -- Submit an instruction to the running session's next agent prompt. The WebUI also offers Tell during a run.
- **Create PR** -- Retry publication for a **PR Failed** session without rerunning the workflow. First check the displayed error and forge authentication (for GitHub HTTPS credentials, `gh auth setup-git` can configure Git). The session remains **PR Failed** after publication fails or is canceled. A fatal after-PR error moves it to **Failed**. On success, the session completes and configured after-PR steps run only if a PR was created or reused.
- **View Diff** -- Review the session branch's changes and return to the menu. See [`cruise diff`](cli-reference.md#cruise-diff) for comparison and cleanup limitations.
- **Replan** -- Provide feedback to regenerate the plan; the session remains in the Planned phase.
- **Open PR** -- Open the session's pull request in the browser via `gh pr view --web`.
- **Merge PR** -- Human-initiated merge of the session's pull request (also available in the WebUI). Cruise first shows the PR's state, mergeability, review decision, and each CI check. Then choose `Squash` (the default), `Merge`, or `Rebase` and confirm. This is a GitHub-only route that requires human confirmation. Built-in `-f merge` is a separate native synchronous squash path.
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
- When a worktree or `--repo` clone session completes, Cruise first copies untracked and ignored test evidence from the workspace into `<data_dir>/sessions/<id>/artifacts/evidence/`, so it survives worktree removal. Only `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, `.md`, `.txt`, and `.log` files are copied, skipping `node_modules`, virtualenvs, and similar dependency or cache directories. Limits are 5 MiB per file, 25 MiB and 100 files per session; images are taken first, and skipped files are listed with the reason in `artifacts/evidence.md`. The manifest path appears in the run log, the WebUI **Info** tab, and the `cruise list` session view. Evidence stays on the local machine and is not added to the PR. Current-branch sessions are not copied because their workspace is never removed.

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

When `cruise list` or the WebUI session detail view loads a session, a session in the `Running` phase is checked for liveness. If the runner process is no longer alive, the session is automatically transitioned to the `Suspended` phase. This prevents sessions from being stuck in `Running` indefinitely after a crash or forced termination.

Ordinary suspended non-exec sessions can be resumed from `cruise list` or the WebUI, or reset to Planned. The `run --all` command also picks them up alongside Planned sessions. Suspended exec sessions can be resumed in the WebUI or with `cruise run <id>`, but cannot be reset to Planned and are excluded from `run --all`. Manager-owned children instead use the `cruise manager` subcommands described above.

<a id="parallel-session-execution"></a>
## Parallel Session Execution

Run All can automatically retry eligible failures from the failed step within the same worker. Set the non-negative limit in WebUI settings or with `cruise config --set-auto-requeue-max-attempts <N>`. It defaults to `0`, disabling retries. Unclassified command errors, eligible provider errors, and no-file-changes errors may retry, while cancellation, configuration, authentication, and authorization errors do not. An attempt's normal failure notification still appears. Completed runs with failed steps are not reclassified as failures.

The persisted per-session count is a lifetime budget across batches. Manual Run or Retry does not reset it, but Reset to Planned does. Failed sessions are not picked up at batch start, single-session runs and exec sessions are not automatically retried, and lowering the limit does not erase the count. Counts appear in session details, CLI list JSON, and batch summaries.

The WebUI and CLI support running multiple sessions concurrently during `run --all`. All session mutations go through the resident daemon, so separate CLI processes and the WebUI can act on sessions concurrently; claims prevent duplicate work on one session.

- **WebUI**: The parallelism level is controlled by `run_all_parallelism` in the selected application settings file (`config.json` or `config.yaml` under `$XDG_CONFIG_HOME/cruise/`), configurable via `cruise config --set-parallelism <N>`, defaulting to `1`. This setting applies to WebUI Run All only.
- **CLI**: Pass `cruise run --all --parallelism <N>` for a one-run override (default: `1`, i.e. sequential). The persisted WebUI setting is neither read nor modified. In an interactive terminal, when `--dry-run` is not set, the CLI also shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output is retained in `sessions/{id}/run.log`. Non-TTY and dry-run invocations keep the normal log output and final summary.

The batch scheduler:
- Seeds from Planned and Suspended sessions.
- Launches up to `N` sessions concurrently.

<a id="new-session-form-persistence"></a>
## New Session Form Persistence

The WebUI persists two pieces of state across sessions:

- **Draft**: The current contents of the New Session form (task description, config path, working directory, repository, and skipped steps). The WebUI saves them when **Save draft** is clicked. The draft is restored when the form is reopened.
- **History**: A log of past New Session selections. It is used to pre-populate the step skip selector with the most recent choices for each config file and to recall previous working directory / config combinations.
