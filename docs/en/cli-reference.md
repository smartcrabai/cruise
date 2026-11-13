**Languages:** [English](../en/cli-reference.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/cli-reference.md) · [简体中文](../zh/cli-reference.md) · [Index](README.md)
<a id="cli-reference"></a>
# Command-line reference

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

<a id="cruise-install"></a>
## `cruise install`

`cruise install <jcode|claude|opencode> [--force]` installs a provider CLI. `cruise install --status` inspects managed installations. Provider CLIs are resolved in this order: environment-variable override, Cruise-managed installation, then PATH. See [Managed provider CLIs](installation.md#managed-provider-clis) for installation command examples.

<a id="cruise-workflow-generate"></a>
## `cruise workflow generate`

```
cruise workflow generate <DESCRIPTION> --name <NAME> [--user] [--config <PATH>]
```

Drafts a new workflow YAML from a description. The file is saved to `./.cruise/<NAME>.yaml`, or to the user workflow directory (`~/.config/cruise/workflows/`) with `--user`. `NAME` may contain only letters, digits, `-` and `_`.

- The validated YAML is saved exactly as returned. An existing file with the same name is never overwritten, and the backend is not called in that case.
- The generated workflow is not run and no session is created.
- Validation does not prove that the commands are safe or that the workflow succeeds. The selected agent backend keeps its normal permissions, so it may modify the repository during generation. Review the YAML before running it.

For interactive drafting and revision, use `cruise workflow make` instead.

<a id="cruise-workflow-make"></a>
## `cruise workflow make`

```
cruise workflow make --name <NAME> [--user] [--config <PATH>] [--from <SELECTOR>] [DESCRIPTION]
```

Create a single workflow YAML through a conversation. Both stdin and stdout must be terminals; use `workflow generate` for non-interactive drafting. An optional description is submitted as the first request. `--config` selects the backend configuration, while `--from` supplies a read-only reference workflow, for example `builtin:tdd` or a file path. Cruise reads the reference without writing it, but an unsandboxed command backend could modify it. The destination is fixed at startup: `./.cruise/<NAME>.yaml`, or the user workflow directory with `--user`. Existing destinations are never overwritten, and the reserved project name `default` cannot be used.

Enter requests to create or revise the draft. The assistant may ask questions without changing it. Use `/show` to inspect the current draft and `/accept` to choose **Write workflow**, **Continue editing**, or **Cancel**. Only choosing Write creates the destination. The workflow is not run and no session is created.

Each request candidate is validated and may be repaired up to three times. If all attempts fail, the previous draft remains unchanged. Validation does not guarantee command safety, so review the result before running it.

Drafts and `conversation.jsonl` are retained under `$XDG_DATA_HOME/cruise/workflow-make/<id>/` (normally `~/.local/share/cruise/workflow-make/<id>/`) after writing or cancellation. The log includes requests. These directories are not removed by `cruise clean`; remove them manually when no longer needed.

Command backends are not sandboxed. Inspect the repository if a backend may have changed it.

<a id="cruise-workflow-doctor"></a>
## `cruise workflow doctor`

```sh
cruise workflow doctor [--json] [--strict] [-f NAME[=on|off]]... [TARGET]...
cruise workflow doctor --all [--json] [--strict]
```

Doctor statically checks workflow YAML without executing it or creating a session, worktree, or project configuration. With no targets it checks `cruise.yaml`, `cruise.yml`, `.cruise.yaml`, `.cruise.yml`, and YAML files in `.cruise/`, excluding `.cruise/default.yaml`. `--all` additionally checks user workflows and all built-ins, and cannot be combined with targets or `-f`.

Targets can be paths, `builtin:<name>`, `__builtin__` (the default built-in), or bare user workflow or built-in names. A bare name matching both a user workflow and a built-in is ambiguous: use an explicit path or `builtin:<name>`. Repeat targets to check several workflows. Repeat `-f` or use comma-separated assignments to select flags for every target, for example `-f review,merge=off`. A flag not declared by a target is an error for that target. Without `-f`, flags are off.

Checks include local prompt and workflow references, configuration, routing targets, retry budgets, flag names, and graph reachability. Resolution uses the same defaults and environment overrides as execution. GitHub workflow references are resolved normally and may require network access. Doctor does not test backend, model, authentication, provider connectivity, command safety, or runtime-dependent template variables.

A step unreachable under the selected flags is a warning, not necessarily a defect. A reachable cycle that cannot reach a normal exit is an error when no normal exit is reachable from the start.

Human-readable reports go to stdout. `--json` emits a single object with `schema_version: 1`, `ok`, `exit_code`, `summary` (`checked`, `passed`, `failed`, `errors`, `warnings`) and ordered `targets`. Each target includes its target, kind (`project`, `user`, `builtin`, or `file`), package status, path, status (`ok`, `warning`, `error`, or `unresolved`), step counts, retry limit, declared flags, and ordered diagnostics. Diagnostics contain `level`, `code`, `stage`, nullable `step`, and `message`. Automations should use `code` and `step`, not message wording.

Exit status is `0` when checks pass (warnings are allowed), `1` for errors or warnings with `--strict`, and `2` for unresolved targets or no targets. Status `2` takes precedence over `1`; argument usage errors also exit `2`.

<a id="cruise-ssh"></a>
## `cruise ssh`

```
cruise ssh [OPTIONS] <DESTINATION> [-- <CRUISE_ARGS>...]
```

`cruise ssh` runs a command on the remote `cruise` through OpenSSH. With no forwarded arguments, the remote `cruise` starts its TUI when a PTY is available. The destination is an OpenSSH `[user@]host` or `~/.ssh/config` alias; use that config for ports, identity files, agents, and ProxyJump.

Options:

```
      --cwd <PATH>                 Change to this remote directory before starting cruise
      --cruise-bin <PATH>          Remote cruise executable or path [default: cruise]
      --tty <auto|always|never>    PTY policy: follow local TTYs, force a PTY, or disable one [default: auto]
```

Everything after `--` is passed to the remote cruise in the same order and is parsed there. In `auto` mode, a local TTY requests a PTY; piped or redirected streams do not, which keeps commands such as `cruise ssh host -- list --json` suitable for scripts. `--tty always` and `--tty never` override that choice.

The remote host is the execution boundary: its sessions, config, jcode authentication, worktrees, and repository clones are stored under its XDG directories and are not shared with local `cruise` commands. `--cwd`, `--config`, `--image`, and other path arguments are interpreted on the remote host. Issue input such as `#12` is resolved using the remote host's `gh` and authentication. Cruise does not upload local files, copy credentials or environment variables, install the remote binary, or synchronize repositories. The WebUI remains a local session client and does not browse remote sessions.

<a id="cruise-1"></a>

## `cruise`

```
cruise
```

Running `cruise` with no arguments opens the keyboard-only TUI. It requires a macOS/Linux interactive TTY; see [TUI (Interactive Keyboard Client)](usage.md#tui-interactive-keyboard-client) for its screens, workflows, keymap, responsive layout, and log behavior. Use the CLI subcommands for automation, JSON, and CI.

<a id="cruise-plan"></a>
## `cruise plan`

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

Non-git directories warn and plan in place, but `cruise run` still requires a git repository.

`--skip-planning` skips the LLM and writes the input directly to `plan.md`. A foreground `cruise plan --skip-planning` on an interactive TTY still opens the approval menu. Non-TTY stdin and background root `cruise --plan --skip-planning` are automatically approved as Planned (with `force_exec: true`, the workflow executes directly unless issue input or another opt-out is used; pass `--no-force-exec` to opt out explicitly). Empty or whitespace-only input is rejected. Use this when you have written the plan yourself and want Cruise to execute it.

`--grill` makes SDK planning an interview: the agent asks questions one at a time and recommends answers before writing `plan.md`. It requires an SDK backend (including the default `jcode` backend), an interactive terminal, and `interactive_planning: true`. It conflicts with `--skip-planning` and applies only to initial planning.

With `--no-interactive-planning`, SDK planning uses the direct-to-file flow even when the workflow config has `interactive_planning: true`. It conflicts with `--grill`.

**Built-in workflow flags.** The six general-purpose built-ins (`tdd`, `refactor`, `add-tests`, `add-formal-specs`, `research`, and `security`) declare `after_pr` and `merge`; every flag is off unless explicitly enabled. `pr-review-fix` declares no flags. `tdd` and `refactor` additionally declare `browser_e2e`, `app_e2e`, and `formal_methods`. For example, `cruise plan -c builtin:tdd -f browser_e2e,app_e2e "task"` enables only the two E2E flags, while `cruise plan -c builtin:tdd -f merge "task"` opts into after-PR automation and the native synchronous squash merge. `-f browser_e2e,app_e2e,merge` combines them. `merge` runs the same after-PR prelude as `after_pr`, once; `after_pr` alone never merges.

- `browser_e2e` enables actual browser exercises, assertions, and failure fixing.
- `app_e2e` enables applicable mobile and desktop native-app exercises using repository/platform-native runners.
- `after_pr` enables after-PR automation without merging.
- `merge` opts into the same after-PR prelude plus a final native synchronous squash merge. On GitHub, PRs that are behind the base or have conflicts are automatically synchronized and recovered before merging. This recovery is not performed on GitLab. It does not enable auto-merge, bypass policy, or use a merge queue.
- `formal_methods` enables workflow-level executable Quint, Alloy, and Gherkin specifications and CI; it is not an initial-plan mode.

`research` remains read-only with `create_pr: false`; `merge` never creates a PR or overrides that no-PR policy. If an opted-in after-PR action is unavailable because no PR is created (including `research`, `exec`, current-branch runs, or a no-commit run), Cruise skips it and prints the reason.

With `--repo <owner/repository>`, the session targets a GitHub or GitLab repository instead of the current directory. Repo sessions always run in Worktree mode and still require `gh` / `glab` to access the repository. With `create_pr: false`, Cruise skips automatic commit, push, PR creation, `after-pr`, and PR-based cleanup. Successful runs with no new branch commits finish without attempting PR creation, but existing branch commits remain eligible for a PR. A publishing failure in a repo or local worktree session results in **PR Failed** and preserves the workspace for recovery with **Create PR**, without rerunning the workflow. `--repo` also works for background planning (`cruise --plan "task" --repo owner/repository`). Free-form `owner/repository` is accepted.

**Fix reviews on an existing GitHub PR.** Run `cruise plan --pr 12` from the repository, or `cruise plan --repo owner/repo --pr 12 "focus on tests"`, approve the generated plan if prompted, then run `cruise run <session-id>`. The optional positional text is an additional instruction; this mode does not read stdin or use an LLM to generate the plan. TUI and WebUI new-session forms also accept a PR number for a GitHub repository. Authenticated `gh` is required. The PR must be open and have a nonempty head branch in the same repository; draft PRs are supported, but fork PRs and GitLab merge requests are not. `--grill` and `--image` cannot be combined with `--pr`.

The default workflow is `builtin:pr-review-fix`, not the project workflow. A custom `-c` workflow must enable PR handling and include an after-PR `github-review` step with `reviewers: all`. The built-in declares no flags and handles reviews without creating or merging a PR. The review loop commits and pushes fixes, replies to or resolves target threads, and can be retried after failure. PR sessions do not offer Publish as Issue or Replan.

<a id="cruise-draft"></a>
## `cruise draft`

```
cruise draft [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description (omit to prompt interactively; reads from stdin when piped)

Options:
      --issue <N>                  Use GitHub issue N as the task
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
```

Saves the input as a `Draft` session without invoking the LLM. The plan can be generated later by choosing **Generate Plan** from `cruise list`. Useful when you have an idea you want to capture immediately but don't want to start (or pay for) planning yet.

<a id="cruise-run"></a>
## `cruise run`

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

**Workflow flags.** A workflow config can declare `flags:` and attach them to steps with `flag: <name>`. A flagged step runs only when its effective value is on; absent flags are off. Enable one with `--flag NAME`, several with `--flag browser_e2e,app_e2e`, or repeat `--flag`; `-f` is an alias for `--flag`. Explicit `NAME=on|off|true|false` assignments are also supported, so `--flag NAME=off` overrides a saved enabled value. Every `cruise run` merges saved flag values with explicit CLI overrides without showing a flag selector, including fresh runs, resumes, `run --all`, and Run from `cruise list`. Unknown names are rejected, and `--flag` conflicts with `--all`. If `-f/--flag` is not supplied, Approve and Execute now select flags before skips. Saved enabled flags are preselected; use Space to toggle and Enter to confirm. If nothing is selected, all declared flags are off.

`--all` runs every Planned or Suspended session in sequence by default. Worktree mode is always forced, regardless of the mode when the session was first created. After all sessions finish, a summary table shows each outcome and PR link. `--all` and `[SESSION]` are mutually exclusive.

`--parallelism <N>` is an invocation-scoped override that runs up to `N` sessions concurrently during `--all`. It defaults to `1` (sequential) when omitted, must be at least `1`, and is rejected unless `--all` is present. Each session runs in its own worktree, a failure in one session does not stop other workers, and Ctrl+C suspends running sessions and stops new scheduling.

`--auto-requeue-max-attempts <N>` overrides the automatic requeue limit for this batch only. The default is `0` (disabled).

When stderr is an interactive terminal and `--dry-run` is not set, `run --all` shows a live dashboard with each scheduled session's title, current step, status, and elapsed time. In non-TTY environments such as CI, or during `--dry-run`, it uses normal log output and the final summary.

<a id="cruise-exec"></a>
## `cruise exec`

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
Runs workflow steps directly in the current directory: no plan is generated, no git worktree is created, and no PR is opened automatically. Sessions paused for input (`Running`) or interrupted with Ctrl+C (`Suspended`) are kept and can be resumed with `cruise run <id>`; exec sessions are excluded from automatic `cruise run` selection and `cruise run --all`. Existing uncommitted changes are allowed; Cruise runs on top of them without stashing, committing, or resetting the working tree, and warns that workflow-generated files may be mixed with those changes. An attached branch is required.

`--instant` builds a workflow with one implement step in memory without resolving workflow YAML. Without it, `cruise exec` retains normal config resolution. With no config file, an interactive terminal offers built-in workflows in a categorized selector, while non-interactive execution selects `tdd` automatically. Instant requires a nonblank task. Workflow defaults can prepend command pre-steps; workflow defaults and environment overrides still apply, and `CRUISE_SDK` / `CRUISE_MODEL` take precedence over `--sdk` / `--model`.

```sh
cruise exec --instant --sdk jcode --model my-model --review --review-rounds 2 \
  --verify 'cargo test' --verify 'cargo clippy' "implement the feature"
cruise exec --instant --dry-run "implement the feature"
```

Review fixes confirmed defects and repeats when a review pass changes files, up to the specified number of passes (default 2). A pass with no changes ends the loop. Verification commands run in the supplied order after implementation and review; a failed command fails the run without starting an automatic repair loop. `--max-retries`, `--rate-limit-retries`, and `--dry-run` also apply; a `--max-retries` budget below the review limit is invalid. Instant declares no workflow flags and cannot be combined with `-c` or `-f`. It requires a git repository with an attached branch. Remote use is available with `cruise ssh <host> -- exec --instant "task"`.

When `force_exec: true` is set in the workflow config, the legacy positional form `cruise "task"`, `cruise plan "task"`, and `cruise --plan "task"` use the current-directory execution path without planning, a worktree, or PR creation. `--no-force-exec`, `--repo`, `--grill`, issue input, and image attachments opt out; `--skip-planning` and `--no-interactive-planning` do not.

<a id="cruise---plan"></a>
## `cruise --plan`

```
cruise --plan <INPUT|stdin> [--skip-planning] [--no-force-exec] [--repo <OWNER/REPO>] [--image <PATH>]... [--flag <VALUE>]...
```

Creates the session immediately, starts plan generation in a detached worker, and returns the new session ID. While the worker is running, `cruise list` shows `Planning`. If generation fails, `cruise list` shows `Plan Failed`, and approval stays disabled until planning succeeds.

Adding `--skip-planning` skips the background worker, writes the input directly to `plan.md`, and creates a session already in `Planned`. No approval step is needed (with `force_exec: true`, the workflow executes directly instead unless issue input or another opt-out is used; pass `--no-force-exec` to opt out explicitly). The flag also works without `--plan` (for example, `cruise --skip-planning "task"`), in which case it behaves like `cruise plan --skip-planning "task"`.

`--repo <owner>/<repository>` is accepted here too and targets the repository as described under [`cruise plan`](#cruise-plan). `--grill` is not available on this path because background planning has no interactive user to interview.

With no subcommand, no `--plan`, and no positional input, non-TTY stdin is read to the end and its contents become positional input. Thus `echo "task" | cruise` takes the legacy positional path and plans like `cruise plan "task"` (`--skip-planning`, `--no-force-exec`, `--repo`, and `--image` still apply). With interactive stdin, the argument-free invocation opens the TUI. Pass `--plan stdin` to read piped input explicitly on the background planning path.

<a id="cruise-tell"></a>
## `cruise tell`

```sh
cruise tell <session-id> "Also cover the timeout case"
printf '%s\n' 'Check the logs' | cruise tell <session-id> -
cruise tell <session-id> --log
cruise ssh devbox --cwd /srv/project -- tell <session-id> "Check the logs"
```

Submit to a live Running session only. Omit the instruction to read non-interactive stdin; `--log` reads instruction history without submitting. Acceptance is not delivery: the currently running agent step cannot see a new instruction. It is appended to the next agent prompt, including all prompt children of a parallel block, but not command or option steps. It cannot change permissions, commit policy, output format, or workflow transitions.

Instructions are trimmed and limited to 8,192 bytes each, with at most 20 pending instructions and 65,536 pending bytes. Completed runs expire undelivered instructions; failed or suspended runs retain them for a subsequent run, and resetting to planned expires them. Do not put secrets in instructions.

<a id="cruise-list"></a>
## `cruise list`

```
cruise list [OPTIONS]

Options:
      --json   Print all sessions as a JSON array to stdout instead of opening the interactive selector
```

With no flags, opens an interactive session browser whose menu depends on each session's phase (see [`cruise list` Actions](sessions.md#cruise-list-actions)). With `--json`, prints every session as a JSON array and exits.

<a id="cruise-diff"></a>
## `cruise diff`

```
cruise diff <SESSION_ID> [--stat]
```

Review a session branch's patch, or use `--stat` for a summary. When the worktree exists, the diff includes committed and uncommitted tracked changes and untracked files. If the worktree is missing, only committed branch changes are shown.

The full patch uses `less -R` on a terminal when available; otherwise, it is written directly to stdout. `--stat` is written directly to stdout.

A session needs a worktree branch for this feature. Current-branch sessions are not supported, and the branch may be removed during cleanup after PR creation (including the default `builtin:tdd` workflow). After cleanup, use **Open PR** to review the changes instead. Diff output is text, not JSON.

<a id="cruise-config"></a>
## `cruise config`

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

CLI and TUI UI language precedence is `CRUISE_UI_LANGUAGE` > application `uiLanguage` > system locale > `en`. Blank or `auto` environment values, and unset or `auto` config values, leave that layer unspecified. Invalid environment values are warned about in English and ignored; the CLI setter and application-config validation reject invalid values.

`cruise config --set-auto-requeue-max-attempts <N>` sets `run_all_auto_requeue_max_attempts` for CLI, TUI, and WebUI Run All. The CLI flag `--auto-requeue-max-attempts <N>` overrides it for one batch.

Set `prevent_sleep: true` in the machine-level `config.yaml` (or `"prevent_sleep": true` in `config.json`) to prevent sleep during runs on macOS; the default is `false`. This covers CLI, TUI, and WebUI runs, but not planning alone. Other operating systems do nothing. Changes take effect on the next run. This setting is not supported in project `default.yaml`.

If both files exist, `config.json` wins; otherwise an existing `config.yaml` is used. The files are not merged. Updates write back to the selected file in its format; YAML comments are not retained.

Use `cruise config --set-language <LANG>` to save a UI language, for example:

```sh
cruise config --set-language ja
cruise config --set-language auto
```

The TUI also offers a language picker on `L`. A WebUI selection is saved in a browser cookie rather than application settings. See [application configuration](configuration.md#ui-language) for locale precedence and the distinction between UI and prompt languages.

<a id="notifications"></a>
### Notifications

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

Event keys are `actionRequired`, `planReady`, `completed`, and `failed`; `action_required` and `plan_ready` are also accepted. Each event can specify `desktop`, `sound`, and `webhook`. An omitted event switch defaults to true, but cannot override a disabled channel master switch. Sound is independent of the desktop banner. Windows sends neither desktop banners nor sounds, but supports webhooks.

Set `CRUISE_NOTIFY_WEBHOOK` in the Cruise process environment to a Slack Incoming Webhook URL. Do not put the secret URL in the settings file. An unset, empty, or invalid HTTP(S) URL disables webhook delivery. Treat the destination as receiving potentially sensitive task details.

Delivery is best effort. Non-default notification settings also appear in `cruise config`, but the webhook URL is never shown.

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

The equivalent JSON is `{"model":["openai/gpt-6.1-sol:medium",{"sdk":"claude","model":"claude-sonnet-5-5:medium"},"openrouter/z-ai/glm-5.3-flash:high"]}`. String entries inherit the workflow's effective SDK, defaulting to `jcode` when neither `sdk` nor `command` is set. Provider prefixes such as `openai/` and `openrouter/` are model references, not SDK names. Existing `opencode` SDK selection is also supported.

Objects require a nonblank `model` and at least one of `sdk` or nonempty `disabled_during`, and are allowed only inside arrays. Unknown keys and explicit null SDKs are rejected. An omitted `sdk` inherits the workflow SDK, not the previous entry's SDK. Objects cannot be combined with `command`, even in a fallback position. Legacy string-only command arrays still use their first model. Environment model overrides replace the entire list. Capability checks apply to all applicable candidates. Computer use requires jcode; remote HTTP/SSE MCP cannot use jcode, and OpenCode does not support legacy SSE.

<a id="candidate-unavailable-windows-disabled_during"></a>
## Candidate unavailable windows (`disabled_during`)

Declare unavailable times per candidate in `model` or `plan_model` arrays. This works in workflows, layered `default.yaml`, and application JSON/YAML settings (including the `planModel` alias). These are user-maintained exclusions, not price-aware routing or provider presets.

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

Cruise checks windows at each prompt start and when switching candidates during retries. It never interrupts an already-streaming turn. A matching window excludes the candidate independently of retry policy or retry budget. If every candidate is disabled or cooling down, the prompt fails without starting a backend.

Capability validation also covers disabled candidates. An override inherits the workflow list and its windows only when its resolved SDK and base model both match the primary candidate. If the SDK also matches, changing only effort preserves inheritance. Other overrides do not inherit the workflow list. Environment model overrides replace the entire list, including its windows.

<a id="cruise-clean"></a>
## `cruise clean`

```
cruise clean
```

Checks each Completed session's PR status via `gh pr view` (or `glab mr view` on GitLab). Sessions whose PR is closed or merged are deleted along with their worktrees (and any leftover `--repo` clone). Planned current-branch sessions whose `plan.md` is missing or blank and that never started are deleted too. Suspended sessions and other planned sessions remain available for resumption.

> **Note:** A session may lack a PR URL if `gh pr create` failed or was not reached. PR-backed sessions without a PR URL are retained; inspect the session logs or re-run PR creation manually with `gh pr create`.

<a id="cruise-usage"></a>
## `cruise usage`

`cruise usage [--by session,step,purpose,backend,model] [--session ID] [--format text|json|csv]` summarizes local attempt records. The default grouping is `step,backend,model` and the default format is text. Missing provider values are unknown, not zero; totals include input and output only. Costs are provider-reported, not estimates.
