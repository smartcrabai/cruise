# cruise

A CLI tool that orchestrates coding agent workflows defined in a YAML config file.

Cruise wraps CLI coding agents such as `claude -p` and drives them through a declarative workflow: plan -> approve -> write tests -> implement -> test -> review -> open PR -> post-PR automation. It handles variable passing between steps, conditional branching, and loop control.

> **Note:** This project supports macOS and Linux only. **Windows is not supported** and Windows binaries are not built or tested. Development and testing happen primarily on macOS; Linux has not been fully verified.

## Prerequisites

- [`gh` CLI](https://cli.github.com/) -- required for worktree mode (PR creation and cleanup). Not needed when using current-branch mode.

## Installation

### cargo install

```sh
cargo install cruise
```

### Homebrew

```sh
brew install smartcrabai/tap/cruise
```

### GUI (Desktop App)

A desktop GUI is also available. Download the latest installer from [GitHub Releases](https://github.com/smartcrabai/cruise/releases):

| Platform | Format |
|----------|--------|
| macOS (Apple Silicon) | `.dmg` |
| Linux (x86_64) | `.deb`, `.AppImage` |

#### macOS GUI Installation

After downloading the DMG and copying `cruise.app` to `/Applications`, run the following in Terminal before the first launch:

```sh
xattr -cr /Applications/cruise.app
```

This removes the Gatekeeper quarantine attribute, allowing the app to launch.

## Usage

```sh
# Create a session (plan -> approve)
cruise plan "implement the feature"

# Create a session and generate the plan in the background
cruise --plan "implement the feature"

# Interview-style planning: answer one question at a time, then the plan is written (SDK backend + TTY)
cruise plan --grill "implement the feature"

# Plan against a GitHub repository instead of a local directory (temporary clone)
cruise plan --repo owner/repository "implement the feature"

# Background planning from stdin
echo "implement the feature" | cruise --plan stdin

# Save the task as a draft (no plan yet); generate the plan later from `cruise list`
cruise draft "implement the feature"

# Execute the approved session
cruise run

# Execute a config directly in the current directory (no plan, no worktree, no PR)
cruise exec "do this"

# List and manage sessions interactively
cruise list

# Remove sessions with closed/merged PRs
cruise clean

# Legacy: no subcommand is treated as `cruise plan`
cruise "implement the feature"
```

### CLI Reference

```
cruise [OPTIONS] [INPUT] [COMMAND]

Commands:
  plan         Create an implementation plan for a task
  draft        Save a task description as a draft without generating a plan
  run          Execute a planned session
  exec         Execute the workflow config directly in the current directory
  list         List and manage sessions interactively
  clean        Remove sessions with closed/merged PRs
  config       Show or update application-level configuration

Options:
      --plan <INPUT>   Create a plan in the background and return immediately
```

#### `cruise plan`

```
cruise plan [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description

Options:
  -c, --config <PATH>              Path to the workflow config file (see Config File Resolution)
      --dry-run                    Print the plan step without executing it
      --skip-planning              Use the input directly as the plan, skipping LLM-based plan generation
      --grill                      Interview-style planning: the agent asks one question at a time, then writes the plan (requires the SDK backend and a TTY; conflicts with --skip-planning)
      --no-interactive-planning    Disable interactive planning tools for this session; the agent writes plan.md directly (conflicts with --grill)
      --repo <OWNER/REPO>          GitHub repository to clone into a temporary directory for planning and execution
      --rate-limit-retries <N>     Maximum number of rate-limit retries per LLM call [default: 5]
```

`cruise plan` creates an isolated git worktree at `$XDG_DATA_HOME/cruise/worktrees/<session-id>/` before invoking the LLM, so plan-phase edits never touch your working copy. The same worktree is reused by `cruise run` in Worktree mode, or cleaned up automatically when you pick Current-branch mode or cancel planning. Non-git directories fall back to running in place with a warning.

With `--skip-planning`, no LLM is called: the (trimmed) input is written straight to `plan.md` and the session goes directly to `Planned`, ready for `cruise run` with no approval step. Empty or whitespace-only input is rejected. Use this when you've already written the plan yourself and just want cruise to execute it. The desktop GUI exposes the same behavior via the **"Use input as plan (skip LLM planning)"** checkbox on the New Session form (the submit button changes from "Generate plan" to "Create session").

With `--grill`, the plan step becomes an interview: instead of writing the plan in one shot, the SDK agent asks you questions **one at a time** (via the `ask_user` tool) — recommending an answer for each — until scope, edge cases, and the implementation approach are fully pinned down, and only then writes `plan.md`. It requires the SDK backend (`sdk:` in the workflow config) and an interactive terminal; cruise errors out (and discards the session) otherwise. `--grill` conflicts with `--skip-planning` and applies only to initial plan generation — Fix/Ask turns, replans, drafts, and background planning use the standard prompt. The desktop GUI exposes the same behavior via the **"Grill me"** toggle on the New Session form (mutually exclusive with "Use input as plan").

With `--no-interactive-planning`, the interactive planning tools (`submit_plan` / `update_plan` / `ask_user`) are disabled for this session even if the workflow config has `interactive_planning: true`. The agent writes `plan.md` directly instead — exactly like the `command` backend. This is useful when using tool-incapable providers (e.g. `sdk: claude-terminal`). The flag conflicts with `--grill` (which requires the interactive tools). It is equivalent to setting `interactive_planning: false` in the workflow config but only affects the current session. The desktop GUI exposes the same behavior via the **"Non-interactive planning"** checkbox on the New Session form (mutually exclusive with "Grill me").

With `--repo <owner>/<repository>`, the session targets a GitHub repository instead of the current directory. The repository is cloned via `gh repo clone` into `$XDG_DATA_HOME/cruise/clones/<session-id>/`, which becomes the session's base directory, so the existing worktree and PR machinery work on the clone unchanged. The clone is removed once the plan is approved (the branch name is kept), re-created by `cruise run`, and removed again after the PR has been created; on failure or suspend it is kept so the session can be resumed or retried (PR-creation failure marks the session `Failed`, not `Completed`). Repo sessions always run in Worktree mode — the no-PR current-branch mode is not available — and a workflow config found inside the clone is copied to `sessions/<session-id>/config.yaml` so it stays readable after the clone is removed. `--repo` also works with background planning (`cruise --plan "task" --repo owner/repository`). The desktop GUI exposes the same behavior via the **Directory / GitHub Repository** source toggle on the New Session form, with a repository picker backed by `gh repo list` (free-form `owner/repository` input is accepted too).

#### `cruise draft`

```
cruise draft [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description (omit to prompt interactively; reads from stdin when piped)

Options:
  -c, --config <PATH>              Path to the workflow config file
```

Saves the input as a `Draft` session without invoking the LLM. The plan can be generated later by choosing **Generate Plan** from `cruise list`. Useful when you have an idea you want to capture immediately but don't want to start (or pay for) planning yet.

#### `cruise run`

```
cruise run [OPTIONS] [SESSION]

Arguments:
  [SESSION]  Session ID to execute (if omitted, picks from pending sessions)

Options:
      --all                        Run all planned sessions sequentially
      --max-retries <N>            Maximum number of times a single loop edge may be traversed [default: 3]
      --rate-limit-retries <N>     Maximum number of rate-limit retries per step [default: 5]
      --dry-run                    Print the workflow flow without executing it
      --cleanup-after-pr           Delete local worktree and branch after PR creation
      --no-cleanup-after-pr        Keep local worktree and branch after PR creation
```

`--all` runs every Planned session in sequence. Worktree mode is always forced (even if the session was originally started in current-branch mode). After all sessions finish, a summary table is printed showing the outcome and PR link for each session. `--all` and `[SESSION]` are mutually exclusive.

#### `cruise exec`

```
cruise exec [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description bound to {input} (optional if your config doesn't reference {input})

Options:
  -c, --config <PATH>              Path to the workflow config file
      --max-retries <N>            Maximum number of times a single loop edge may be traversed [default: 3]
      --rate-limit-retries <N>     Maximum number of rate-limit retries per step [default: 5]
      --dry-run                    Print the workflow flow without executing it
```

Runs the workflow steps directly in the current directory: no plan is generated, no git worktree is created, and no PR is opened automatically. The session is still recorded so progress is visible in `cruise list`. Use this when you want to drive a config against the active branch -- the same constraints as the Current-branch workspace mode apply (clean working tree, attached branch).

#### `cruise --plan`

```
cruise --plan <INPUT|stdin> [--skip-planning] [--repo <OWNER/REPO>]
```

Creates the session immediately, starts plan generation in a detached worker, and returns the new session ID. While the worker is still running, `cruise list` shows the session as `Planning`. If generation fails, the session remains in `AwaitingApproval` phase internally but `cruise list` shows `Plan Failed`, and approval stays disabled until planning succeeds.

Adding `--skip-planning` skips the background worker entirely: the input is written directly as `plan.md` and the session is created already in `Planned` — no approval step needed. The flag also works without `--plan` (e.g. `cruise --skip-planning "task"`), in which case it behaves like `cruise plan --skip-planning "task"`.

`--repo <owner>/<repository>` is accepted here too and behaves as described under [`cruise plan`](#cruise-plan): the repository is cloned into a temporary directory and the session targets the clone. `--grill` is not available on this path — background planning has no interactive user to interview.

#### `cruise list`

```
cruise list [OPTIONS]

Options:
      --json   Print all sessions as a JSON array to stdout instead of opening the interactive selector
```

With no flags, opens an interactive session browser whose menu depends on each session's phase (see [`cruise list` Actions](#cruise-list-actions)). With `--json`, prints every session as a JSON array (id, phase, input, PR URL, plan-error info, ...) and exits -- useful for scripting or feeding session state to external tooling.

#### `cruise config`

```
cruise config [OPTIONS]

Options:
      --set-parallelism <N>   Set the max number of sessions the desktop GUI runs concurrently in `run --all` mode (must be >= 1)
```

Shows or updates application-level settings stored in `$XDG_CONFIG_HOME/cruise/config.json` (default: `~/.config/cruise/config.json`) -- this is separate from the per-workflow YAML configs. With no flags, prints the current configuration. `--set-parallelism <N>` sets `run_all_parallelism` (default `1`), which controls how many sessions the **desktop GUI** executes in parallel during `run --all`. The CLI `cruise run --all` always runs sessions sequentially regardless of this value.

#### `cruise clean`

```
cruise clean
```

Checks each Completed session's PR status via `gh pr view`. Sessions whose PR is closed or merged are deleted along with their worktrees (and any leftover `--repo` clone). Sessions without a PR URL or with an open PR are skipped.

> **Note:** A session may lack a PR URL if `gh pr create` failed or was not reached (e.g. the workflow failed before completion, or PR creation returned an error). If a session is unexpectedly skipped by `cruise clean`, check the session logs or re-run PR creation manually with `gh pr create`.

## Session Management

Cruise stores session data in `$XDG_DATA_HOME/cruise/sessions/` (default: `~/.local/share/cruise/sessions/`).

### Runtime File Layout

Cruise follows the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/) and splits its runtime files across three directories:

| Kind | Path |
|------|------|
| User YAML configs and application settings | `$XDG_CONFIG_HOME/cruise/` (default: `~/.config/cruise/`) |
| Sessions, worktrees, and temporary `--repo` clones | `$XDG_DATA_HOME/cruise/` (default: `~/.local/share/cruise/`) |
| State files (`history.json`, `new_session_draft.json`) | `$XDG_STATE_HOME/cruise/` (default: `~/.local/state/cruise/`) |

> **Migrating from `~/.cruise/`?** Earlier versions stored everything under `~/.cruise/`. Move `*.yaml`/`config.json` into `~/.config/cruise/`, `sessions/` and `worktrees/` into `~/.local/share/cruise/`, and `history.json`/`new_session_draft.json` into `~/.local/state/cruise/`. Use `git worktree move` (or `git worktree repair`) when relocating worktree directories.

### Session Lifecycle

1. **`cruise plan "task"`** -- Runs the built-in plan step in an isolated planning worktree to generate an implementation plan, then presents an approve-plan menu.
2. **`cruise --plan "task"`** -- Creates the session immediately and generates the plan in the background. Review it later from `cruise list`.
3. **`cruise draft "task"`** -- Records the task as a `Draft` session without running the plan step. Use **Generate Plan** from `cruise list` to start planning when you're ready.
4. **Approve-plan menu** -- Choose one of:
   - **Approve** -- Mark the session as ready to run.
   - **Fix** -- Provide feedback; the plan step reruns with your input.
   - **Ask** -- Ask a question; the answer is shown before the menu reappears.
   - **Execute now** -- Skip approval and run immediately.

   After approving (or choosing "Execute now"), a **step skip selector** is shown if the workflow config defines more than zero steps. A multi-select prompt lists all steps (grouped steps appear as a parent with children); toggle any steps you want to skip for this run. The selection is persisted per config file in `$XDG_STATE_HOME/cruise/history.json` and pre-selected as the default for the next session using the same config.

5. **`cruise run`** -- Picks up the approved session, reuses (or creates) the git worktree under `$XDG_DATA_HOME/cruise/worktrees/<session-id>/`, executes the workflow steps, automatically creates a PR with `gh pr create`, then runs any configured `after-pr` steps.

Sessions remain in `$XDG_DATA_HOME/cruise/sessions/` until their PR is closed or merged, after which `cruise clean` will remove them.

> **`cruise exec`** is a separate path that skips this lifecycle entirely: it executes the workflow in the current directory without planning, worktree creation, or PR creation. See [`cruise exec`](#cruise-exec).

### `cruise list` Actions

The interactive session list shows a menu of actions depending on the session's phase:

| Phase | Available Actions |
|-------|-------------------|
| **Draft** | Generate Plan, Delete, Back |
| **AwaitingApproval** | Approve, Delete, Back |
| **Planned** | Run, Replan, Delete, Back |
| **Running** | Resume, Reset to Planned, Delete, Back |
| **Suspended** | Resume, Reset to Planned, Delete, Back |
| **Failed** | Run, Reset to Planned, Delete, Back |
| **Completed** | Open PR*, Reset to Planned, Delete, Back |

\* Open PR is shown only when the session has a PR URL.

`cruise list` may also show `Planning` while `--plan` is still running, or `Plan Failed` when background planning wrote a durable `plan_error`. Those states only offer `Delete` and `Back`; `Approve` appears only after a non-empty `plan.md` is available.

- **Generate Plan** -- Start planning for a `Draft` session (transitions it through the normal planning flow).
- **Approve** -- Approve the plan and transition the session to the Planned phase.
- **Run / Resume** -- Execute (or continue) the session.
- **Replan** -- Provide feedback to re-generate the plan; the session stays in the Planned phase.
- **Open PR** -- Open the session's pull request in the browser via `gh pr view --web`.
- **Reset to Planned** -- Reset the session back to the Planned phase, clearing the current step and allowing it to be re-run from the beginning.
- **Delete** -- Permanently remove the session.
- **Back** -- Return to the session list.

## Config File Resolution

cruise resolves the workflow config as follows:

1. **`-c/--config` flag** -- highest priority. The specified file must exist or cruise exits with an error. No prompt is shown.
2. **`CRUISE_CONFIG` environment variable** -- if set, used directly (error if the file does not exist). No prompt is shown.
3. Otherwise, cruise collects every candidate from the following locations and presents them as choices:
   - `./cruise.yaml` -> `./cruise.yml` -> `./.cruise.yaml` -> `./.cruise.yml` (current directory)
   - `$XDG_CONFIG_HOME/cruise/*.yaml` / `*.yml` (default: `~/.config/cruise/`), sorted by filename

   When stdin and stdout are both TTYs, candidates are shown in an interactive selector and the user picks one. With a single candidate the choice is auto-picked. In non-interactive contexts (piped stdin, scripts) the highest-priority candidate is taken automatically without a prompt.
4. **No candidate found** -- cruise falls back to a built-in 2-step workflow (`write-tests` -> `implement`); no config file is required, but you'll usually want one.

The `description:` field of each config file is shown next to its filename in both the CLI selector and the GUI, making it easier to tell similar files apart.

## Config File Reference

### Basic Structure

```yaml
command:                   # LLM invocation command (mutually exclusive with `sdk`)
  - claude
  - --model
  - "{model}"
  - -p

# sdk: seher              # alternative to `command`: drive prompts via seher's resolved provider (see SDK Mode)
# sdk: pi                 # alternative to `command`/`sdk: seher`: drive pi_agent_rust directly, no seher config needed

description: |             # one-line summary shown next to the filename in selectors (optional)
  Team-shared review-heavy flow with auto-PR.

model: sonnet             # default model for all prompt steps (optional)
plan_model: opus          # model used for the built-in plan step (optional)
pr_language: English      # language for auto-generated PR title/body (optional, default: English)
plan_language: English    # language for built-in planning prompts (optional, default: English)

env:                      # environment variables applied to all steps (optional)
  API_KEY: sk-...
  PROJECT: myproject

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

after-pr:                # optional: steps that run automatically after PR creation
  step_name:
    # step configuration (same format as `steps`)
```

### Dynamic Model Selection

When the `command` array contains a `{model}` placeholder, cruise resolves it at runtime based on the effective model for each step:

- **Model specified** (via top-level `model` or step-level `model`): replaces `{model}` with the model name.
- **No model specified**: removes the `{model}` argument and its immediately-preceding `--model` flag automatically.

A step-level `model` field overrides the top-level `model` default for that step only.

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

Instead of spawning an external CLI via `command`, prompt steps can be driven in-process through an SDK by setting the top-level `sdk` field. `command` and `sdk` are mutually exclusive -- exactly one of them must be specified. Two values are accepted: `seher` and `pi`.

```yaml
sdk: seher        # resolve a provider/model through seher's config.yaml

model: build      # in seher mode, interpreted as a seher mode_key (default: build)
plan_model: plan  # mode_key for the built-in plan step (falls back to `model`, then `plan`)
```

In `sdk: seher` mode, `model` / `plan_model` / per-step `model` are reinterpreted as seher **mode keys** rather than LLM model names. When omitted, `model` defaults to `build`; `plan_model` falls back to `model`, or to `plan` when neither is set.

#### `sdk: pi` -- drive pi_agent_rust directly

`sdk: pi` drives `pi_agent_rust` directly in-process, **bypassing seher's provider resolution and `~/.config/seher/config.yaml` entirely** -- no seher configuration is required at all.

```yaml
sdk: pi

model: anthropic/claude-sonnet-4-6   # plain model reference, not a mode key
plan_model: openai/gpt-5.5:high      # "provider/model[:thinking]"
```

In `sdk: pi` mode, `model` / `plan_model` / per-step `model` are plain **model references** instead of seher mode keys (same override precedence as command mode: step `model` > top-level `model` / `plan_model`):

- `"provider/model"` (optionally `:thinking`, e.g. `openai-codex/gpt-5.5:xhigh`) -- selects that provider and model explicitly.
- `"model"` (no `/`) -- provider is left unset; pi resolves it by searching its own model registry for that model id (same as running the `pi` CLI with `--model` but no `--provider`).
- Unset (both `model` and `plan_model` omitted) -- pi auto-selects a provider/model from its built-in preference order (Codex, then OpenAI, ... down to Anthropic and others), picking the first one with usable credentials.

Authentication is resolved entirely by pi itself, in this order: an explicit key (not exposed by cruise) > pi's stored `~/.pi/agent/auth.json` OAuth/Bearer credentials > ambient environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and similar provider-specific vars) -- so a stored `pi login` credential always wins over a stale shell env var. `env:` values configured in the workflow are applied on top of the process environment before each pi call.

Rate limits are retried against the **same** provider/model with exponential backoff (2s, doubling up to a 60s cap, same schedule as command mode) up to `--rate-limit-retries` attempts -- there is no other seher provider to fall back to, unlike `sdk: seher`.

#### Tool-less (non-interactive) planning

By default, SDK-mode planning drives the plan through custom tools (`submit_plan` / `update_plan` / `ask_user`). Under `sdk: seher`, custom tools require a tool-capable seher SDK (`pi` or `claude`), so this pins planning to those providers; `sdk: pi` always supports custom tools (there is no tool-incapable pi provider), so this setting matters less there.

Set `interactive_planning: false` to turn that off. Planning then embeds the target plan-file path in the prompt and asks the agent to write `plan.md` directly — exactly like the `command` backend — and registers no custom tools. The resulting `plan.md` is read back afterward (falling back to the agent's captured output if the file was not written, same as `command` mode). Under `sdk: seher` this makes tool-incapable providers eligible, so SDK modes backed by `sdk: claude-terminal` or `sdk: claude-headless` (both of which shell out to the local `claude` CLI) can be used for planning.

```yaml
sdk: seher
interactive_planning: false   # tool-less, file-based planning; allows claude-terminal / claude-headless providers
```

`--grill` requires the interactive tool-based flow and is rejected when `interactive_planning` is off. The field has no effect in `command` mode, which is always file-based.

### PR Language

The `pr_language` field controls the language used for the auto-generated PR title and body. Defaults to `"English"` when omitted.

```yaml
pr_language: Japanese     # PR title/body will be generated in Japanese
```

### Plan Language

The `plan_language` field controls the language used by cruise's built-in planning prompts, including initial plan generation, plan fixes, and plan Q&A. Defaults to `"English"` when omitted. The normalized value is available to built-in planning templates as `{plan.language}`.

```yaml
plan_language: Japanese   # generated/updated plans and plan answers will be in Japanese
```

### Session Title Generation

After plan approval, cruise generates a concise session title (up to 80 characters) shown in `cruise list` and the GUI sidebar instead of the raw task input. The behavior depends on the backend:

- **SDK mode (`sdk:` configured)** -- cruise invokes the agent with the `generate_title` SDK tool, using the same model resolution as the plan step (`plan_model` -> `model` -> `plan`, reinterpreted as a seher mode key under `sdk: seher` or passed through as a model reference under `sdk: pi`). If the call fails, cruise falls back to extracting the title from `plan.md`.
- **Command mode (`command:` configured)** -- no LLM is called for title generation. The title is derived automatically from the first heading or first non-empty line in the generated `plan.md`.

No additional configuration is required.

### Environment Variables

Environment variables can be set at two levels. Step-level values override top-level values for that step only. Values support template variable substitution.

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

```yaml
steps:
  planning:
    model: claude-opus-4-5        # model to use (optional; overrides top-level model)
    instruction: |                # system prompt (optional)
      You are a senior engineer.
    prompt: |                     # prompt body (required)
      Create an implementation plan for:
      {input}
    timeout: 10m                  # per-step timeout (optional; see Step Timeout)
    env:                          # environment variables for this step (optional)
      ANTHROPIC_MODEL: claude-opus-4-5
```

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

#### Step Timeout

Any step may set `timeout:` to abort the step if it runs too long. Accepted formats:

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
        next: ~                          # null next = end of workflow
```

### Post-PR Automation (`after-pr`)

Use `after-pr` for steps that should run automatically after `cruise run` successfully creates a pull request. `after-pr` uses the same step format as `steps`, so you can define prompt steps, command steps, and grouped steps there as well.

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
- An empty or syntactically invalid glob is rejected at config validation time.
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

#### No file changes detection (`if.no-file-changes`)

When a step has `if: no-file-changes`, a snapshot of the working directory is taken **before** the step runs. If the step completes without modifying any workspace files, the configured action is taken. Two modes are available:

- **`fail: true`** -- Abort the workflow with an error and transition the session to the `Failed` state. This is useful for detecting cases where an LLM claims to have implemented something but did not actually modify any files.
- **`retry: true`** -- Re-execute the current step. This is useful for retrying a step until it produces meaningful file changes.

```yaml
steps:
  implement:
    prompt: "Implement the feature described in {plan}"
    if:
      no-file-changes:
        fail: true

  fix:
    prompt: "Fix the issue"
    if:
      no-file-changes:
        retry: true
```

**Constraints:**
- `fail` and `retry` are mutually exclusive -- exactly one must be true.
- Cannot be used in `after-pr` steps (rejected at validation time).
- Cannot be used at the group level (`if` in group definitions).
- Cannot be combined with the legacy `fail-if-no-file-changes: true` on the same step.
- Can be combined with `if: file-changed` on the same step, but when both are present, `no-file-changes` takes priority for change detection.

The legacy `fail-if-no-file-changes: true` syntax is still supported and is equivalent to `if: { no-file-changes: { fail: true } }`.

#### Failure handling (`if.fail`)

`if.fail` decides what happens when a step fails. A failure means any of: a non-zero exit code from a command step, a prompt step error (including LLM transport errors), a `timeout`, or a `no-file-changes: fail` trigger.

Two forms are accepted:

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

`if.fail` is subject to the same loop-protection budget as other flow-control jumps (`--max-retries`), so a misconfigured retry loop will not run forever.

**Constraints:**
- `if.fail` is rejected at the group level and in `after-pr` steps.
- Can be combined with other `if:` keys (`file-changed`, `no-file-changes`) on the same step.

### Step Groups

Steps can be grouped to coordinate retry loops across multiple steps. A group retries all its member steps together when the `if: file-changed` condition triggers.

Groups can define their steps inline and are invoked from the main `steps` section with `group: <name>`:

```yaml
groups:
  review:
    if:
      file-changed: test    # if any step in the group changes files, retry from the group start
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
- When the group's `if: file-changed` condition triggers, execution jumps back to the **first step of the group** and all group steps re-run.
- A call-site step (e.g. `review-pass: group: review`) cannot have its own `if:` condition.

### Workflow Composition (`workflow_call`)

A step can delegate to another workflow config file by setting `workflow_call` instead of `prompt`, `command`, or `option`. The called workflow's steps are inlined into the parent at the call site, with each step ID prefixed by the call-site name (e.g. `shared-review/simplify`).

```yaml
steps:
  build:
    command: cargo build

  shared-review:
    workflow_call: ./workflows/review.yaml    # local relative path

  deploy:
    command: cargo publish
```

The referenced file is a regular cruise config. Its top-level execution settings (`command`, `sdk`, `model`, `env`, etc.) are ignored -- only its `steps` are imported. The parent's settings apply to the expanded steps.

#### Supported sources

| Source | Example |
|--------|---------|
| Local relative path | `workflow_call: ./workflows/review.yaml` |
| GitHub blob URL | `workflow_call: https://github.com/org/repo/blob/main/workflows/review.yaml` |
| GitHub raw URL | `workflow_call: https://raw.githubusercontent.com/org/repo/main/workflows/review.yaml` |

GitHub workflows are fetched via `gh api` at config-load time. Relative paths inside a GitHub-hosted workflow resolve from the remote directory, so nested references work across repositories.

#### Call-site fields

A `workflow_call` step is a pure delegation point. Only `skip`, `when`, and `next` may be set alongside `workflow_call`:

- `skip` and `when` are applied to the **first** expanded step.
- `next` is applied to the **last** expanded step (when it has no explicit `next` of its own).

All other step fields (`prompt`, `command`, `model`, `if`, `timeout`, `env`, etc.) are rejected at validation time.

#### Nesting and cycle detection

Workflow calls can be nested: a called workflow may itself contain `workflow_call` steps. Step IDs accumulate prefixes (`outer/inner/step`). Circular references (A calls B, B calls A) are detected and rejected. Groups inside called workflows are not supported.

```yaml
# parent.yaml -> nested/outer.yaml -> inner/leaf.yaml
# Results in step IDs: outer-call/leaf-call/leaf
steps:
  outer-call:
    workflow_call: ./nested/outer.yaml
```

### Variable Reference

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from CLI argument or stdin |
| `{prev.output}` | LLM output from the previous step |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Stderr captured from the previous command step |
| `{prev.success}` | Exit status of the previous command step (`true`/`false`) |
| `{plan}` | Session plan file path (set automatically by `cruise run`) |
| `{pr.number}` | Pull request number, available after a PR has been created |
| `{pr.url}` | Pull request URL, available after a PR has been created |
| `{pr.language}` | Language used for PR title/body generation (from `pr_language`) |

> **Note:** `{model}` is **not** a template variable -- it is a special placeholder resolved only within the top-level `command` array. It is not available inside `prompt`, `instruction`, or `command` step fields.

## Workspace Mode

When `cruise run` starts a new session, it prompts you to choose a workspace mode:

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| Mode | Description |
|------|-------------|
| **Worktree** (default) | Creates an isolated git worktree at `$XDG_DATA_HOME/cruise/worktrees/<session-id>/` (default: `~/.local/share/cruise/worktrees/<session-id>/`). A new branch `cruise/<session-id>-<sanitized-input>` is checked out. Requires `gh` CLI for PR creation. |
| **Current branch** | Executes directly in the current repository on the active branch. No worktree is created, and no PR is created automatically. |

In non-interactive environments (piped stdin) and with `--all`, worktree mode is used automatically. Sessions created with `--repo` (or the GUI repository picker) are always pinned to Worktree mode — the prompt is skipped and current-branch mode is not available, since a PR is the only way the work leaves the temporary clone.

### Current-branch mode constraints

- Requires a clean working tree (no uncommitted changes) for a fresh run.
- Requires an attached branch (not detached HEAD).
- On resume, the active branch must match the branch recorded at the start of the session.

### Worktree isolation

- The worktree is retained until the PR is closed or merged; run `cruise clean` to delete it.
- Set `cleanup_after_pr: true` in the config (or pass `--cleanup-after-pr` at runtime) to automatically delete the local worktree and branch immediately after the PR is created. Use `--no-cleanup-after-pr` to override the config setting and keep them.

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

steps:
  implement:
    prompt: "{input}"

  test:
    command: cargo test

  fix:
    prompt: |
      The following test errors occurred. Please fix them:
      ---
      {prev.stderr}
    if:
      file-changed: test    # after fix, if it modified files, jump back to test

  commit:
    command: git add -A && git commit -m "feat: {input}"
```

## Config Hot-Reload

During `cruise run`, the config file is checked for changes between each step. If the file has been modified (detected via mtime), the updated config is reloaded automatically -- no restart required. This allows you to adjust prompts, add steps, or tweak settings while a session is running.

> **Note:** Hot-reload only applies when the session was started from an external config file (not the built-in default). The current step must still exist in the new config for the reload to take effect.

## Rate Limit Retry

When a rate-limit error (HTTP 429) is detected in a prompt or command step, cruise retries with exponential backoff:

- Initial delay: 2 seconds
- Maximum delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)

## Stale Session Detection

When `cruise list` (or the desktop GUI) loads sessions, any session in the `Running` phase is checked for liveness. If the runner process (identified by PID and start time) is no longer alive, the session is automatically transitioned to the `Suspended` phase. This prevents sessions from being stuck in `Running` indefinitely after a crash or forced termination.

Suspended sessions can be resumed from `cruise list` or reset to Planned. The `run --all` command also picks up Suspended sessions alongside Planned ones.

## Parallel Session Execution

The desktop GUI supports running multiple sessions concurrently during `run --all`. The parallelism level is controlled by `run_all_parallelism` in `$XDG_CONFIG_HOME/cruise/config.json` (configurable via `cruise config --set-parallelism <N>`, default: `1`).

The batch scheduler:
- Seeds from Planned and Suspended sessions.
- Launches up to `N` sessions concurrently.
- Re-scans for newly added Planned sessions every 200ms while worker slots are available, so sessions created while a batch is running are picked up automatically.
- Results are returned in scheduling order regardless of completion order.

The CLI `cruise run --all` always runs sessions sequentially regardless of the parallelism setting.

## New Session Form Persistence

The desktop GUI persists two pieces of state across sessions:

- **Draft** (`$XDG_STATE_HOME/cruise/new_session_draft.json`): The current contents of the New Session form (task description, config path, working directory, repository, skipped steps). Automatically saved on changes and restored when the form is reopened, so unsent input is not lost.
- **History** (`$XDG_STATE_HOME/cruise/history.json`): A log of past New Session selections. Used to pre-populate the step skip selector with the most recent choices for each config file and to recall previous working directory / config combinations.

## GitHub Actions

Mention `@cruise` on a GitHub Issue to drive cruise inside GitHub Actions, always through the `sdk: pi` backend (no `claude` CLI install). There is no PR mode -- comments on pull requests are ignored. The word right after the mention picks a command: `run` (default) plans-and-implements and opens a draft PR, `exec` pushes straight to the default branch (no PR, advanced/opt-in), `plan` posts an LLM-generated plan as a tracking comment, and `fix <feedback>` revises that comment in place. See [`docs/github-actions.md`](docs/github-actions.md) for the full command reference.

Setup: (1) install the [`cruise-agent` GitHub App](https://github.com/apps/cruise-agent/installations/new) on your repository, (2) add an `ANTHROPIC_API_KEY` and/or `OPENAI_API_KEY` secret (pi needs at least one), (3) copy the workflow below to `.github/workflows/cruise.yml`. The App lets the action authenticate as `cruise-agent[bot]` via a short-lived, repository-scoped token instead of the default `GITHUB_TOKEN` -- see [`docs/github-actions.md`](docs/github-actions.md#how-authentication-works) for how that works and how to opt out.

```yaml
# .github/workflows/cruise.yml
on:
  issue_comment:
    types: [created]
  issues:
    types: [opened]

jobs:
  cruise:
    # See examples/cruise.yml for the full per-event trigger-phrase filter.
    if: |
      (github.event_name == 'issue_comment' && !github.event.issue.pull_request && contains(github.event.comment.body, '@cruise')) ||
      (github.event_name == 'issues' && (contains(github.event.issue.title, '@cruise') || contains(github.event.issue.body, '@cruise')))
    runs-on: ubuntu-latest
    timeout-minutes: 30
    permissions:
      contents: write
      pull-requests: write
      issues: write
      id-token: write # for the cruise-agent App token exchange; optional
    steps:
      - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7
        with:
          fetch-depth: 0
      - uses: smartcrabai/cruise@v1
        with:
          anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
```

See [`examples/cruise.yml`](examples/cruise.yml) for the full trigger filter and [`docs/github-actions.md`](docs/github-actions.md) for the command reference, inputs/outputs, security notes, and how to point it at your own workflow config.

## License

MIT
