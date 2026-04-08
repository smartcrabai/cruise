# cruise

A CLI tool that orchestrates coding agent workflows defined in a YAML config file.

Cruise wraps CLI coding agents such as `claude -p` and drives them through a declarative workflow: plan → approve → write tests → implement → test → review → open PR → post-PR automation. It handles variable passing between steps, conditional branching, and loop control.

> **Note:** This project has been developed and tested on macOS only. It has not been verified on Linux or Windows.

## Prerequisites

- [`gh` CLI](https://cli.github.com/) — required for worktree mode (PR creation and cleanup). Not needed when using current-branch mode.

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
| Windows (x86_64) | `.msi`, `.exe` |

#### macOS GUI Installation

After downloading the DMG and copying `cruise.app` to `/Applications`, run the following in Terminal before the first launch:

```sh
xattr -cr /Applications/cruise.app
```

This removes the Gatekeeper quarantine attribute, allowing the app to launch.

## Usage

```sh
# Create a session (plan → approve)
cruise plan "implement the feature"

# Create a session and generate the plan in the background
cruise --plan "implement the feature"

# Background planning from stdin
echo "implement the feature" | cruise --plan stdin

# Execute the approved session
cruise run

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
  run          Execute a planned session
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
      --rate-limit-retries <N>     Maximum number of rate-limit retries per LLM call [default: 5]
```

#### `cruise run`

```
cruise run [OPTIONS] [SESSION]

Arguments:
  [SESSION]  Session ID to execute (if omitted, picks from pending sessions)

Options:
      --all                        Run all planned sessions sequentially
      --max-retries <N>            Maximum number of times a single loop edge may be traversed [default: 10]
      --rate-limit-retries <N>     Maximum number of rate-limit retries per step [default: 5]
      --dry-run                    Print the workflow flow without executing it
```

`--all` runs every Planned session in sequence. Worktree mode is always forced (even if the session was originally started in current-branch mode). After all sessions finish, a summary table is printed showing the outcome and PR link for each session. `--all` and `[SESSION]` are mutually exclusive.

#### `cruise --plan`

```
cruise --plan <INPUT|stdin>
```

Creates the session immediately, starts plan generation in a detached worker, and returns the new session ID. While the worker is still running, `cruise list` shows the session as `Planning`. If generation fails, the session remains in `AwaitingApproval` phase internally but `cruise list` shows `Plan Failed`, and approval stays disabled until planning succeeds.

#### `cruise clean`

```
cruise clean
```

Checks each Completed session's PR status via `gh pr view`. Sessions whose PR is closed or merged are deleted along with their worktrees. Sessions without a PR URL or with an open PR are skipped.

> **Note:** A session may lack a PR URL if `gh pr create` failed or was not reached (e.g. the workflow failed before completion, or PR creation returned an error). If a session is unexpectedly skipped by `cruise clean`, check the session logs or re-run PR creation manually with `gh pr create`.

## Session Management

Cruise uses a session-based workflow stored in `~/.cruise/sessions/`.

### Session Lifecycle

1. **`cruise plan "task"`** — Runs the built-in plan step to generate an implementation plan, then presents an approve-plan menu.
2. **`cruise --plan "task"`** — Creates the session immediately and generates the plan in the background. Review it later from `cruise list`.
3. **Approve-plan menu** — Choose one of:
   - **Approve** — Mark the session as ready to run.
   - **Fix** — Provide feedback; the plan step reruns with your input.
   - **Ask** — Ask a question; the answer is shown before the menu reappears.
   - **Execute now** — Skip approval and run immediately.
4. **`cruise run`** — Picks up the approved session, creates a git worktree under `~/.cruise/worktrees/<session-id>/`, executes the workflow steps, automatically creates a PR with `gh pr create`, then runs any configured `after-pr` steps.

Sessions remain in `~/.cruise/sessions/` until their PR is closed or merged, after which `cruise clean` will remove them.

### `cruise list` Actions

The interactive session list shows a menu of actions depending on the session's phase:

| Phase | Available Actions |
|-------|-------------------|
| **AwaitingApproval** | Approve, Delete, Back |
| **Planned** | Run, Replan, Delete, Back |
| **Running** | Resume, Reset to Planned, Delete, Back |
| **Suspended** | Resume, Reset to Planned, Delete, Back |
| **Failed** | Run, Reset to Planned, Delete, Back |
| **Completed** | Open PR*, Reset to Planned, Delete, Back |

\* Open PR is shown only when the session has a PR URL.

`cruise list` may also show `Planning` while `--plan` is still running, or `Plan Failed` when background planning wrote a durable `plan_error`. Those states only offer `Delete` and `Back`; `Approve` appears only after a non-empty `plan.md` is available.

- **Approve** — Approve the plan and transition the session to the Planned phase.
- **Run / Resume** — Execute (or continue) the session.
- **Replan** — Provide feedback to re-generate the plan; the session stays in the Planned phase.
- **Open PR** — Open the session's pull request in the browser via `gh pr view --web`.
- **Reset to Planned** — Reset the session back to the Planned phase, clearing the current step and allowing it to be re-run from the beginning.
- **Delete** — Permanently remove the session.
- **Back** — Return to the session list.

## Config File Resolution

When `-c` is not specified, cruise searches for a config in this order:

1. `-c/--config` flag — the specified file must exist or cruise exits with an error.
2. `CRUISE_CONFIG` environment variable — error if file does not exist.
3. `./cruise.yaml` → `./cruise.yml` → `./.cruise.yaml` → `./.cruise.yml` — in the current directory.
4. `~/.cruise/*.yaml` / `*.yml` — auto-selected if exactly one file exists, or prompted if multiple.
5. Built-in default — a 2-step test-first workflow (`write-tests` → `implement`); no config file required.

## Config File Reference

### Basic Structure

```yaml
command:
  - claude
  - --model
  - "{model}"
  - -p

model: sonnet             # default model for all prompt steps (optional)
plan_model: opus          # model used for the built-in plan step (optional)
pr_language: English      # language for auto-generated PR title/body (optional, default: English)

llm:                      # OpenAI-compatible API for session title generation (optional)
  api_key: sk-...         # API key (or set CRUISE_LLM_API_KEY env var)
  endpoint: https://api.openai.com/v1   # default endpoint
  model: gpt-4o-mini      # model to use for title generation

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

### PR Language

The `pr_language` field controls the language used for the auto-generated PR title and body. Defaults to `"English"` when omitted.

```yaml
pr_language: Japanese     # PR title/body will be generated in Japanese
```

### Session Title Generation

When an API key is configured, cruise calls an OpenAI-compatible API after plan approval to generate a concise session title (up to 80 characters). This title is shown in `cruise list` and the GUI sidebar instead of the raw task input.

Configure via the `llm:` block in the config file, or with environment variables:

| Setting | Config field | Environment variable | Default |
|---------|-------------|----------------------|---------|
| API key | `llm.api_key` | `CRUISE_LLM_API_KEY` | *(required)* |
| Endpoint | `llm.endpoint` | `CRUISE_LLM_ENDPOINT` | `https://api.openai.com/v1` |
| Model | `llm.model` | `CRUISE_LLM_MODEL` | `gpt-4o-mini` |

Environment variables take precedence over config file values.

```yaml
llm:
  api_key: sk-...
  endpoint: https://api.openai.com/v1
  model: gpt-4o-mini
```

If no API key is configured, the title is derived automatically from the first heading or first non-empty line in the generated `plan.md`.

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
    env:                          # environment variables for this step (optional)
      ANTHROPIC_MODEL: claude-opus-4-5
```

#### Command Step (shell execution)

```yaml
steps:
  run_tests:
    command: cargo test           # single command (required)
    env:                          # environment variables for this step (optional)
      RUST_LOG: debug

  lint_and_test:
    command:                      # list of commands: run sequentially, stop on first failure
      - cargo fmt --all
      - cargo clippy -- -D warnings
      - cargo test
```

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

#### Conditional execution (file-changed detection)

When a step has `if: file-changed: <target>`, a snapshot of the working directory is taken **before** the step runs. After the step executes, if any files changed during its execution, the workflow jumps to `<target>`. If no files changed, the workflow continues to the next step normally.

This is designed for loop-back patterns — for example, re-running tests whenever a review step modifies code:

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

- **`fail: true`** — Abort the workflow with an error and transition the session to the `Failed` state. This is useful for detecting cases where an LLM claims to have implemented something but did not actually modify any files.
- **`retry: true`** — Re-execute the current step. This is useful for retrying a step until it produces meaningful file changes.

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
- `fail` and `retry` are mutually exclusive — exactly one must be true.
- Cannot be used in `after-pr` steps (rejected at validation time).
- Cannot be used at the group level (`if` in group definitions).
- Cannot be combined with the legacy `fail-if-no-file-changes: true` on the same step.
- Can be combined with `if: file-changed` on the same step, but when both are present, `no-file-changes` takes priority for change detection.

The legacy `fail-if-no-file-changes: true` syntax is still supported and is equivalent to `if: { no-file-changes: { fail: true } }`.

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
- Steps inside a group definition cannot have nested `group:` references or individual `if:` conditions — the group-level `if:` applies to the entire group.
- When the group's `if: file-changed` condition triggers, execution jumps back to the **first step of the group** and all group steps re-run.
- A call-site step (e.g. `review-pass: group: review`) cannot have its own `if:` condition.

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

> **Note:** `{model}` is **not** a template variable — it is a special placeholder resolved only within the top-level `command` array. It is not available inside `prompt`, `instruction`, or `command` step fields.

## Workspace Mode

When `cruise run` starts a new session, it prompts you to choose a workspace mode:

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| Mode | Description |
|------|-------------|
| **Worktree** (default) | Creates an isolated git worktree at `~/.cruise/worktrees/<session-id>/`. A new branch `cruise/<session-id>-<sanitized-input>` is checked out. Requires `gh` CLI for PR creation. |
| **Current branch** | Executes directly in the current repository on the active branch. No worktree is created, and no PR is created automatically. |

In non-interactive environments (piped stdin) and with `--all`, worktree mode is used automatically.

### Current-branch mode constraints

- Requires a clean working tree (no uncommitted changes) for a fresh run.
- Requires an attached branch (not detached HEAD).
- On resume, the active branch must match the branch recorded at the start of the session.

### Worktree isolation

- The worktree is retained until the PR is closed or merged; run `cruise clean` to delete it.

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

During `cruise run`, the config file is checked for changes between each step. If the file has been modified (detected via mtime), the updated config is reloaded automatically — no restart required. This allows you to adjust prompts, add steps, or tweak settings while a session is running.

> **Note:** Hot-reload only applies when the session was started from an external config file (not the built-in default). The current step must still exist in the new config for the reload to take effect.

## Rate Limit Retry

When a rate-limit error (HTTP 429) is detected in a prompt or command step, cruise retries with exponential backoff:

- Initial delay: 2 seconds
- Maximum delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)

## License

MIT
