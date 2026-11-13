# Command-line reference

**Languages:** [English](../en/cli-reference.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/cli-reference.md) · [简体中文](../zh/cli-reference.md) · [Index](README.md)

## CLI Reference

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

### `cruise install`

`cruise install <jcode|claude|opencode> [--force]` installs the build's pinned provider version. `cruise install --status` inspects managed installations without network access or side effects. See [Managed provider CLIs](installation.md#managed-provider-clis) for resolution and authentication details.

### `cruise workflow generate`

```
cruise workflow generate <DESCRIPTION> --name <NAME> [--user] [--config <PATH>]
```

Drafts a new workflow YAML from a description. The backend is chosen by the normal config resolution (`--config`, `CRUISE_CONFIG`, local files, user workflows, or the built-in `tdd` workflow). The file is saved to `./.cruise/<NAME>.yaml`, or to the user workflow directory (`~/.config/cruise/workflows/`) with `--user`. `NAME` may contain only letters, digits, `-` and `_`.

- The reply must be raw YAML (no Markdown fence). Each candidate receives doctor static checks with flags off and isolated defaults (reference resolution relative to the destination directory, config validation, retry budget, compile, graph checks). On failure the diagnostic and previous candidate are sent back for up to 3 repairs (4 backend turns in total). If it is still invalid, nothing is written.
- The validated YAML is saved exactly as returned. An existing file with the same name is never overwritten, and the backend is not called in that case.
- The generated workflow is not run and no session is created.
- Validation does not prove that the commands are safe or that the workflow succeeds. The selected agent backend keeps its normal permissions, so it may modify the repository during generation. Review the YAML before running it.

For interactive drafting and revision, use `cruise workflow make` instead.

### `cruise workflow make`

```
cruise workflow make --name <NAME> [--user] [--config <PATH>] [--from <SELECTOR>] [DESCRIPTION]
```

Create a single workflow YAML through a conversation. Both stdin and stdout must be terminals; use `workflow generate` for non-interactive drafting. An optional description is submitted as the first request. `--config` selects the backend configuration, while `--from` supplies a read-only reference workflow, for example `builtin:tdd` or a file path. Cruise reads the reference without writing it, but an unsandboxed command backend could modify it. The destination is fixed at startup: `./.cruise/<NAME>.yaml`, or the user workflow directory with `--user`. Existing destinations are never overwritten, and the reserved project name `default` cannot be used.

Enter requests to create or revise the draft. The assistant may ask questions without changing it. Use `/show` to inspect the current draft and `/accept` to choose **Write workflow**, **Continue editing**, or **Cancel**. Escape or Ctrl+C at the request prompt cancels; at the approval menu they return to editing. Only choosing Write creates the destination, after re-reading and validating the staged YAML. The workflow is not run and no session is created.

Candidates pass the same `exec` preflight as `workflow generate`, with up to three repairs per request. If all attempts fail, the previous draft remains unchanged. The backend reply can include explanatory text and at most one `<cruise-workflow>` block; replies without a block are conversation only. Validation does not prove command safety, so review the result before running it.

Drafts and `conversation.jsonl` are retained under `$XDG_DATA_HOME/cruise/workflow-make/<id>/` (normally `~/.local/share/cruise/workflow-make/<id>/`) after writing or cancellation. The log includes requests. These directories are not removed by `cruise clean`; remove them manually when no longer needed.

The backend is asked to operate read-only in the project directory. Repository changes detected during a turn cause that turn to fail, including changes made concurrently by the user. Command backends are not sandboxed: change detection is not prevention and excludes some paths. Inspect the repository if a backend may have changed it.

### `cruise workflow doctor`

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

### `cruise ssh`

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

<a id="cruise-1"></a>

### `cruise`

```
cruise
```

Running `cruise` with no arguments opens the keyboard-only TUI. It requires a macOS/Linux interactive TTY; see [TUI (Interactive Keyboard Client)](usage.md#tui-interactive-keyboard-client) for its screens, workflows, keymap, responsive layout, and log behavior. Use the CLI subcommands for automation, JSON, and CI.

### `cruise plan`

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
- `merge` opts into the same after-PR prelude plus a final native synchronous squash merge. On GitHub, merge readiness checks the current base ancestry; confirmed behind/diverged or `CONFLICTING`/`DIRTY` PRs route through built-in sync-base recovery. Only native `ResyncRequired` routing from `squash-merge` to its `if.fail` target starts a fresh after-PR pass: it resets `budgeted_traversals` for every after-PR edge except `squash-merge` → target and clears group retry counts, preserving edge traversal history. The resync edge remains capped at `max_retries`; generic `if.fail` retains shared budgets. Worst case: up to `max_retries` resyncs, each pass allowing up to `max_retries - 1` CI-fix iterations; zero retry budget permits no resync. Other readiness failures still fail, and GitLab behavior is unchanged. It does not enable auto-merge, bypass policy, or use a merge queue.
- `formal_methods` enables workflow-level executable Quint, Alloy, and Gherkin specifications and CI; it is not an initial-plan mode.

`research` remains read-only with `create_pr: false`; `merge` never creates a PR or overrides that no-PR policy. If an opted-in after-PR action is unavailable because no PR is created (including `research`, `exec`, current-branch runs, or a no-commit run), Cruise skips it and prints the reason.

With `--repo <owner/repository>`, the session targets a GitHub or GitLab repository instead of the current directory. The repository is cloned via `gh repo clone` (or `glab repo clone` on GitLab) into `$XDG_DATA_HOME/cruise/clones/<session-id>/`, which becomes the session's base directory. Repo sessions always run in Worktree mode and still require `gh`/`glab` to access the repository. With `create_pr: false`, Cruise skips automatic commit, push, PR creation, `after-pr`, and PR-based cleanup while retaining the normal session, artifacts, and worktree/clone. Otherwise, the planning worktree and clone are removed once the plan is approved (only the branch name is kept), then `cruise run` re-clones and recreates them; the clone is removed after PR creation. A successful run with no new branch commits completes without attempting PR creation; existing branch commits remain eligible for a PR. Publication failures in either repo or local worktree sessions enter **PR Failed** and retain the workspace for **Create PR** recovery, rather than rerunning the workflow. A config inside the clone (or the built-in `tdd` when no config file is found) is copied to `sessions/<session-id>/config.yaml` so it stays readable after clone removal, including inlined `prompt_file` contents; an external `-c` path or `CRUISE_CONFIG` file is referenced in place instead. `--repo` also works with background planning (`cruise --plan "task" --repo owner/repository`). The WebUI exposes the same behavior via the **Directory / Repository (GitHub or GitLab)** source toggle on the New Session form, with a repository picker backed by `gh repo list` (or `glab repo list` when `CRUISE_FORGE=gitlab`) (free-form `owner/repository` input is accepted too).

**Fix reviews on an existing GitHub PR.** Run `cruise plan --pr 12` from the repository, or `cruise plan --repo owner/repo --pr 12 "focus on tests"`, approve the generated plan if prompted, then run `cruise run <session-id>`. The optional positional text is an additional instruction; this mode does not read stdin or use an LLM to generate the plan. TUI and WebUI new-session forms also accept a PR number for a GitHub repository. Authenticated `gh` is required. The PR must be open and have a nonempty head branch in the same repository; draft PRs are supported, but fork PRs and GitLab merge requests are not. `--grill` and `--image` cannot be combined with `--pr`.

The default is `builtin:pr-review-fix`, not the project workflow. A custom `-c` workflow must enable PR handling and include an after-PR `github-review` step with `reviewers: all`. The builtin declares no flags, synchronizes the existing head with `git pull --ff-only`, and addresses reviews without creating or merging a PR. The session is pinned to an isolated worktree on the PR head branch, tracking `origin/<head>`; a diverged branch or a branch already checked out elsewhere causes an error rather than a reset. Cleanup removes the worktree but preserves the PR head branch. The review loop commits and pushes fixes, replies or resolves eligible threads, and writes `review-report.json`. Failures leave a retryable session. Publish as Issue and Replan are unavailable for PR sessions.

### `cruise draft`

```
cruise draft [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description (omit to prompt interactively; reads from stdin when piped)

Options:
      --issue <N>                  Use GitHub issue N as the task
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
```

Saves the input as a `Draft` session without invoking the LLM. The plan can be generated later by choosing **Generate Plan** from `cruise list`. Useful when you have an idea you want to capture immediately but don't want to start (or pay for) planning yet.

### `cruise run`

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

**Workflow flags.** A workflow config can declare `flags:` and attach them to steps with `flag: <name>`. A flagged step runs only when its effective value is on; absent flags are off. Enable one with `--flag NAME`, several with `--flag browser_e2e,app_e2e`, or repeat `--flag`; `-f` is an alias for `--flag`. Explicit `NAME=on|off|true|false` assignments remain supported, so `--flag NAME=off` overrides a saved enabled value. Every `cruise run` uses saved flag values merged with explicit CLI overrides without a flag selector, including fresh runs, resumes, `run --all`, and Run from `cruise list`. Unknown names are rejected, and `--flag` conflicts with `--all`. Approve and Execute now select flags before skips unless `-f/--flag` was supplied. In that selector, saved enabled values are preselected; Space toggles choices and Enter confirms, saving selected flags as on and unselected flags as off. With no selection, all declared flags are off. Execute now forwards that selection without prompting again at execution start. `cruise exec` and `force_exec` entry points select flags on a TTY when no `-f/--flag` is supplied, except on dry runs or when no flags are declared. TUI and WebUI Info tabs show all declared flags as `NAME=on|off`, with unspecified flags off. TUI New Session flags start off and Enter proceeds without requiring a selection; WebUI on/off selectors default to off. CLI, TUI, and WebUI skip selectors, and the WebUI and TUI Graph tabs, hide flag-disabled steps, except that legacy unversioned graph checkpoints are displayed without filtering. The TUI Edit Settings dialog accepts `flag:NAME=on|off` lines. See [Top-level structure](../../skills/cruise-config/references/top-level.md#flags).

`--all` runs every Planned or Suspended session in sequence by default. Worktree mode is always forced (even if the session was originally started in current-branch mode). After all sessions finish, a summary table is printed showing the outcome and PR link for each session. If a session state file cannot be reloaded for the summary, that session is reported as `Failed` with the state path and error, and the batch still completes. `--all` and `[SESSION]` are mutually exclusive.

Each attempt that reaches a persisted `Completed` or `Failed` state sends a best-effort notification through enabled channels, including a configured webhook. This includes intermediate failures before an automatic retry. A Run All batch does not send an additional aggregate notification. Option prompts and interactive planning questions send **Action required** immediately before waiting for input.

`--parallelism <N>` is an invocation-scoped override that runs up to `N` sessions concurrently during `--all`. It defaults to `1` (sequential) when omitted, must be at least `1`, and is rejected unless `--all` is present. Each concurrent session still runs in its own worktree, failures in one session do not stop the other workers, and Ctrl+C suspends the running sessions and stops scheduling new ones. This flag does not read or modify the persisted WebUI/TUI setting (`cruise config --set-parallelism`).

`--auto-requeue-max-attempts <N>` overrides the persisted auto-requeue limit for this batch only. Unlike parallelism, CLI Run All reads the persisted limit when the flag is omitted. The default is `0` (disabled).

When stderr is an interactive terminal and `--dry-run` is not set, `run --all` shows a live dashboard with each scheduled session's title, current step, status, and elapsed time; detailed agent output remains in that session's `sessions/{id}/run.log`. In non-TTY environments such as CI, or during `--dry-run`, it keeps the normal log output and final summary behavior.

### `cruise exec`

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

When `force_exec: true` is set in the workflow config, the legacy positional form `cruise "task"` (except the exact inputs `ssh` and `mcp-bridge`, which are reserved for their subcommands), `cruise plan "task"`, and `cruise --plan "task"` use the current-directory execution path without planning, worktree, or PR creation. `--no-force-exec`, `--repo`, `--grill`, issue input, and image attachments opt out; `--skip-planning` and `--no-interactive-planning` do not. Background `--plan` runs foreground because there is no plan worker for direct execution. Both `exec` and these `force_exec` entry points show a flag selector before creating a session when stdin is a TTY and no `-f/--flag` is supplied; dry runs, piped input (including `--plan stdin`), and workflows without declared flags do not prompt. Resuming a retained exec session by ID keeps its saved flags without prompting.

### `cruise --plan`

```
cruise --plan <INPUT|stdin> [--skip-planning] [--no-force-exec] [--repo <OWNER/REPO>] [--image <PATH>]... [--flag <VALUE>]...
```

Creates the session immediately, starts plan generation in a detached worker, and returns the new session ID. While the worker is still running, `cruise list` shows the session as `Planning`. If generation fails, the session remains in `AwaitingApproval` phase internally but `cruise list` shows `Plan Failed`, and approval stays disabled until planning succeeds.

Adding `--skip-planning` skips the background worker entirely: the input is written directly as `plan.md` and the session is created already in `Planned` — no approval step needed (with `force_exec: true` the workflow executes directly instead unless issue input or another opt-out is used; pass `--no-force-exec` to opt out explicitly). The flag also works without `--plan` (e.g. `cruise --skip-planning "task"`), in which case it behaves like `cruise plan --skip-planning "task"`.

`--repo <owner>/<repository>` is accepted here too and behaves as described under [`cruise plan`](#cruise-plan): the repository is cloned into a temporary directory and the session targets the clone. `--grill` is not available on this path — background planning has no interactive user to interview.

With no subcommand, no `--plan`, and no positional input, a non-TTY stdin is read to the end and its trimmed contents become the positional input, so `echo "task" | cruise` takes the legacy positional path and plans like `cruise plan "task"` (`--skip-planning`, `--no-force-exec`, `--repo`, and `--image` still apply). Blank piped input is ignored, and with an interactive stdin the argument-free invocation opens the TUI instead. Pass `--plan stdin` to read piped input explicitly on the background planning path.

### `cruise tell`

```sh
cruise tell <session-id> "Also cover the timeout case"
printf '%s\n' 'Check the logs' | cruise tell <session-id> -
cruise tell <session-id> --log
cruise ssh devbox --cwd /srv/project -- tell <session-id> "Check the logs"
```

Submit to a live Running session only. Omit the instruction to read non-interactive stdin; `--log` reads instruction history without submitting. Acceptance is not delivery: the currently running agent step cannot see a new instruction. It is appended to the next agent prompt, including all prompt children of a parallel block, but not command or option steps. It cannot change permissions, commit policy, output format, or workflow transitions.

Instructions are trimmed and limited to 8,192 bytes each, with at most 20 pending instructions and 65,536 pending bytes. Delivered or expired items are terminal. Completed runs expire undelivered instructions; failed or suspended runs retain them for a subsequent run, and reset to planned expires them. Delivery is at least once: a crash or failure to record acknowledgement can cause redelivery. The session's XDG data directory stores `instructions.jsonl` and its lock; `--log` shows statuses. Do not put secrets in instructions.

### `cruise list`

```
cruise list [OPTIONS]

Options:
      --json   Print all sessions as a JSON array to stdout instead of opening the interactive selector
```

With no flags, opens an interactive session browser whose menu depends on each session's phase (see [`cruise list` Actions](sessions.md#cruise-list-actions)). With `--json`, prints every session as a JSON array (id, phase, input, PR URL, plan-error info, `pending_instructions`, ...) and exits -- useful for scripting or feeding session state to external tooling.

### `cruise diff`

```
cruise diff <SESSION_ID> [--stat]
```

Review a session branch's patch, or use `--stat` for a summary. The comparison starts at the merge-base of the session branch and the base repository's current HEAD. As HEAD moves, the comparison base may change. When the worktree exists, the diff includes committed and uncommitted tracked changes and untracked files, excluding untracked paths listed in the base repository's `.worktreeinclude` and nested repositories. Tracked changes are included even when listed in `.worktreeinclude`. If the worktree is missing, only committed branch changes are shown. Patch output is limited to 1 MiB and at most 200 untracked files are included; truncation is reported.

The full patch uses `less -R` on a terminal when available, otherwise it is written directly to stdout. `--stat` is written directly to stdout. The list's **View Diff** action returns to the menu afterwards. The TUI and WebUI also have a Diff detail tab; refresh the TUI with `r` or use the WebUI tab's **Refresh** button to reload it.

A session needs a worktree branch for this feature. Current-branch sessions are not supported, and the branch may be removed during cleanup after PR creation (including the default `builtin:tdd` workflow). After cleanup, use **Open PR** to review the changes instead. Diff output is text, not JSON.

### `cruise config`

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

`cruise config` displays application settings, including `ui_language` and its effective source (`env`, `config`, `system`, or `default`). Use `cruise config --set-language <LANG>` to save a UI language, for example:

```sh
cruise config --set-language ja
cruise config --set-language auto
```

The second command clears the saved language so the next available preference applies. Supported explicit tags are `en`, `zh-CN`, `es`, `fr`, `pt-BR`, `ja`, and `ko`. `--set-language` rejects unsupported values. The TUI also offers a language picker on `L`. A WebUI selection is instead saved in a browser cookie.

See [application configuration](configuration.md#ui-language) for locale precedence and the distinction between UI and prompt languages.

#### Notifications

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

Workflow and `default.yaml` model settings override these defaults; environment overrides still win. If no layer sets `plan_model`, planning uses the effective `model`. SDK arrays use the existing [fallback behavior](sdk.md#sdk-mode); command mode uses only the primary. `planModel` is accepted as an alias for `plan_model`. Missing parallelism defaults to `1`, so model-only settings are valid. Updating parallelism through the CLI, WebUI, or TUI preserves both model settings.

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

`workflow_categories` is an ordered tree of category maps. Each category may contain a `workflows` list and nested categories. Entries are names or single-key maps whose value overrides the selector description. `show_others_category` defaults to `true`, and `others_category_name` defaults to `Others`. These settings belong in application `config.yaml` or `config.json`, not workflow files or `default.yaml`. See [config file resolution](configuration.md#config-file-resolution) for matching and fallback behavior.

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

### Candidate unavailable windows (`disabled_during`)

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

### `cruise clean`

```
cruise clean
```

Checks each Completed session's PR status via `gh pr view` (or `glab mr view` on GitLab). Sessions whose PR is closed or merged are deleted along with their worktrees (and any leftover `--repo` clone). Terminal exec/current-branch sessions that cannot have a PR, including legacy exec remnants, are deleted without a GitHub status check. Planned current-branch sessions whose `plan.md` is missing or blank and that never started are deleted too. Suspended sessions and other planned sessions remain available for resumption.

> **Note:** A session may lack a PR URL if `gh pr create` failed or was not reached. PR-backed sessions without a PR URL are retained; inspect the session logs or re-run PR creation manually with `gh pr create`.

### `cruise usage`

`cruise usage [--by session,step,purpose,backend,model] [--session ID] [--format text|json|csv]` summarizes local attempt records. The default grouping is `step,backend,model` and the default format is text. Missing provider values are unknown, not zero; totals include input and output only. Command backends are recorded as unavailable. Costs are provider-reported, not estimates. OpenCode auxiliary title and compaction usage is excluded, so usage may be undercounted. Command backend internal retries are recorded as one call. `cruise ssh` records usage on the remote host, not locally.
