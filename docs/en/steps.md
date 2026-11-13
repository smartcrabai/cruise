# Workflow step reference

**Languages:** [English](../en/steps.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/steps.md) · [简体中文](../zh/steps.md) · [Index](README.md)

## Step Types

### Prompt Step (LLM call)

Prompt steps can declare `quality_gates` to verify a successful attempt before advancing; see [Quality gates](workflow-control.md#quality-gates-quality_gates).

```yaml
steps:
  planning:
    model: claude-opus-4-5        # model to use (optional; overrides top-level model)
    instruction: |                # message displayed before the step runs (optional)
      Before planning, add any constraints or context:
    prompt: |                     # prompt body (use either prompt or prompt_file)
      Create an implementation plan for:
      {input}
    timeout: 10m                  # per-step timeout (optional; see Step Timeout)
    output_file: initial-state.md # save the final response as a session artifact (optional)
    allow_commit: false           # default: guard this prompt from moving Git HEAD in this repository
    computer_use: true            # optional; jcode only; overrides top-level computer_use
    env:                          # environment variables for this step (optional)
      ANTHROPIC_MODEL: claude-opus-4-5
```

When `output_file` is set, cruise saves the completed backend response outside the
workspace at the session's `artifacts/` directory. The file is written atomically,
keeps the exact response text, and is not included in workspace change detection.
Later prompt bodies can read it with `{file:initial-state.md}`. File references
are resolved immediately before a prompt runs, including prompts loaded through
`prompt_file`; the inserted text is not recursively expanded. Artifact names must
be relative and stay within the session directory, and saves and reads are limited
to 1 MiB. Names are rejected when empty, absolute, or containing a `.` or `..`
path component; a subdirectory such as `reports/initial-state.md` is allowed and
its directories are created on write. Symbolic links are refused rather than
followed: a symlink anywhere in the artifact path -- including the session's
`artifacts/` root or its managed parent directories -- fails both reads and
writes, as does a final path that exists but is not a regular file.
See [`examples/file-artifacts.yaml`](../../examples/file-artifacts.yaml) for a
baseline-and-regression comparison flow. Artifacts belong to the session: deleting
the session removes them. Terminal `cruise exec` sessions are cleaned up after
completion, while interrupted exec sessions remain resumable by ID and `cruise run`
sessions retain artifacts for resume.

An `instruction` is a message shown to the user immediately before a step runs, not a system prompt. If `{input}` is empty, the message is used as a multiline input prompt and the submitted text becomes `{input}`; otherwise it is displayed without being sent to the LLM.

Prompt steps are commit-guarded by default. SDK prompts also receive the implicit Git shell denials described under [Tool restrictions](sdk.md#tool-restrictions); `allow_commit: true` removes these and the guard, not explicit `disallowed_tools`, and never grants broader permissions. Set `allow_commit: true` only when a prompt intentionally needs to create a commit or otherwise move Git `HEAD`; omitted and `false` values keep the guard enabled. The guard covers classic `command:` mode and all SDK backends (`jcode`, `claude`, and `opencode`), while command and option steps remain unaffected. Ref rejection is scoped to the repository the step runs in, identified by its git common dir: `HEAD` and `refs/heads/*` updates are rejected in every worktree of that repository, including the main checkout and any linked worktree. The repository-scoped commit guard permits ref updates in throwaway repositories created below the step (temporary repositories used by test suites, other clones, nested repositories), but SDK shell prefix denials still block matching Git commands targeting those repositories. Commits in the guarded repository's own checkout or any of its linked worktrees remain rejected. A detected movement fails the step and cruise may restore the original branch reference without resetting the index or worktree.

When `allow_commit` is omitted or `false`, cruise also appends an instruction telling the agent to leave edits uncommitted, not move `HEAD` or local branch refs in the working repository or its linked worktrees, and not bypass the commit guard even if the task requests it. This notice supplements Git enforcement; `allow_commit: true` omits this commit-guard notice, but explicit `disallowed_tools` still adds its own restriction notice.

Set `allow_commit: true` on the actual prompt, not a `group:` or `workflow_call:` invocation. The guard is implemented by injecting `core.hooksPath` and `CRUISE_GUARD_COMMON_DIR` through the step's child environment, which installs a `reference-transaction` hook that rejects ref updates only in the repository named by that variable. Only that rejection is repository-scoped: the injected hooks path applies to every git invocation below the step, in every repository, so a repository's own hooks (`pre-commit`, `commit-msg`, `pre-push`, ...) do not run under a guarded step, not even in an unrelated repository whose ref updates are allowed. The hook identifies the current repository from the `GIT_DIR` that git exports to hooks, following the `commondir` file for linked worktrees, and falls back to `git rev-parse --git-common-dir`. The guard is fail-closed: if the repository of a transaction cannot be resolved the transaction is rejected and the step fails with a commit guard violation, while a missing working directory or a directory outside a Git worktree is still a graceful no-op. It remains a best-effort same-user policy, not a security boundary: other repositories are not post-checked.

For longer prompts, load the prompt body from a file with `prompt_file`:

```yaml
steps:
  implement:
    prompt_file: prompts/implement.md
```

`prompt_file` may be an absolute path, a `~/` path, a path relative to the
configuration file, or a GitHub blob/raw URL. A bare file name is resolved next
to the configuration file. GitHub URLs are fetched via `gh api` at config-load
time. Files referenced inside a `workflow_call` are
resolved relative to the called workflow's directory. File contents are kept
verbatim and use the same variable expansion as `prompt`; `prompt` and
`prompt_file` are mutually exclusive. For repo-backed sessions whose config lives in the temporary clone, the session
snapshot stores the resolved prompt contents, so they remain usable after the
clone is removed.
Resolution follows these rules:

| `prompt_file` value | Resolution |
| --- | --- |
| Absolute path | Used as-is |
| `~/...`, `~`, or `~user/...` | Expanded from the current user's or named user's home directory |
| `./...`, `../...`, or a bare file name | Relative to the directory containing the config file |
| GitHub blob/raw URL | Fetched from GitHub |
| Relative path inside a called workflow | Relative to the called workflow's directory (or remote directory) |

Absolute and `~` paths refer to the local filesystem. In a GitHub-hosted
workflow, relative non-URL values are resolved as paths in the remote directory; local
`~` paths are rejected. A direct GitHub blob/raw URL can be used explicitly.

### Command Step (shell execution)

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

### Parallel Steps

Use `parallel:` to run named prompt or command steps concurrently within one
session. The next step starts after every child finishes.

```yaml
steps:
  checks:
    parallel:
      lint:
        command: cargo clippy -- -D warnings
      tests:
        command: cargo test
      review:
        prompt: Review the changes and report findings without editing files.
        timeout: 10m
    timeout: 15m
  summarize:
    prompt: "Summarize the check results: {prev.output}"
```

- All children start together in the session's existing working directory.
  Use independent commands or file edits; cruise does not isolate or merge child
  changes. Concurrent edits to the same files or Git state can conflict.
- Each child receives the same input and `{prev.*}` values from before the
  block. Environment precedence is workflow < parallel block < child. Children
  can use `model`, `env`, `skip`, `when`, and `timeout`; prompt children may also
  set `instruction`, `computer_use`, `permission`, and `disallowed_tools` (the parent cannot), and `output_file` with distinct artifact names within the block. Parallel instructions are noninteractive and require nonempty runtime input. Supply input or use a preceding sequential instruction step, otherwise execution returns an error without opening an input editor. `instruction` is rejected on command children. `prompt_file`
  resolves relative to its config as usual. Command arrays remain sequential per
  child.
  A child prompt that reads an artifact produced by a sibling in the same block
  (`{file:...}` pointing at another child's `output_file`) is rejected at config
  validation time, as are an empty `parallel: {}` block and a child whose
  `command:` array is empty.
- After joining, `{prev.output}` is a JSON object keyed by child name in YAML
  declaration order. Each entry has `output` (prompt text, or `null` for commands),
  `stderr` (including execution errors), `success`, and `skipped`.
  `{prev.success}` is true when every child succeeded or was skipped;
  `{prev.stderr}` combines child errors with their names. Logs include
  `[parent/child]` prefixes. Command children have no interactive stdin.
- A child failure does not cancel siblings. The block counts as one step and
  one failure at most, following normal step failure semantics. Put `if.fail`
  or `next` on the parent; a retry reruns the entire block. Unhandled prompt or
  execution errors stop the workflow after siblings finish. Nonzero command
  exits and timeouts follow the ordinary step failure path.
  Two child errors ignore that aggregation: a commit-guard violation or a
  missing-model error from a jcode prompt is always fatal and stops the workflow
  even when the parent has `if.fail`, while a child timeout is never fatal on its
  own and always reports through `{prev.*}` and the ordinary step failure path.
- A child timeout affects only that child. A parent timeout cancels unfinished
  children and waits for them to stop before taking the failure path. Ctrl+C
  stops all children. Resume restarts the whole interrupted block, including
  children that already finished. Graph display and step selection treat the
  block as one step.
- Parallel blocks also work inside `groups`, `after-pr`, and called workflows.
  Children must be prompt/command steps; child `next`, `if`, `option`,
  `plan`, `group`, `workflow_call`, nested `parallel`, `flag`, and
  `allow_commit: true` are rejected. Configure flow control on the parent (including `flag`) and
  perform commits in a subsequent sequential step.

### Step Timeout

Prompt, command, and parallel steps may set `timeout:` to abort the step if it runs too long (option steps ignore it; in a command array each command gets the timeout; on a parallel block it limits the whole block). Accepted formats:

| Suffix | Meaning | Example |
|--------|---------|---------|
| (none) | Seconds | `timeout: "30"` |
| `m` | Minutes | `timeout: 5m` |
| `h` | Hours | `timeout: 1h` |

When a timeout fires:

- **Command steps**: the child process is killed and the step is treated as a failure (non-zero exit). `{prev.success}` is `false` and the workflow follows the normal failure path (see `if.fail` below).
- **Prompt steps**: the LLM call is aborted and the step is treated as a failure.

Invalid timeout strings are rejected at config validation time. Timeouts are also honoured for steps defined inside groups and `after-pr`.

### Option Step (interactive selection)

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
        next: ~                          # null/omitted next falls through to the
                                         # next declared step; the last step ends the workflow
```

## Post-PR Automation (`after-pr`)

For host-created commit and PR body formatting, see [Commit and PR Templates](configuration.md#commit-and-pr-templates).

Prompt steps here, including `github-review` prompts, inherit workflow
`disallowed_tools` and may add step restrictions. Native `squash-merge` is not
a prompt and cannot carry a nonempty `disallowed_tools` list.

On GitLab, the built-in `after-pr` steps detect `/-/merge_requests/` in `{pr.url}` and use `glab` (`glab mr update --ready`, `glab ci get --merge-request`, `glab ci trace`) instead of `gh`.

Use `after-pr` for steps that should run automatically after `cruise run` successfully creates a pull request. `after-pr` uses the same step format as `steps`, so you can define inline or file-backed prompt steps (`prompt` / `prompt_file`), command steps, and grouped steps there as well.

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

Prompt steps in `after-pr` are guarded by the same default, with the same repository scope and the same fail-closed behaviour. The built-in `resolve-conflict` and `fix-ci-error` prompts explicitly set `allow_commit: true` because they are intentionally expected to commit fixes to the PR branch. Other after-PR prompts must opt in individually when they need to advance `HEAD` in the session repository; command and option steps are unaffected.

### Review loop (`github-review`)

An explicit `after-pr` step can handle review bots such as CodeRabbit. The general-purpose built-ins do not add it; `builtin:pr-review-fix` uses it for an existing PR.

```yaml
after-pr:
  coderabbit:
    github-review:
      bots: ["coderabbitai[bot]"]   # any one of these reviewing the current PR head ends the wait
      max-iterations: 3             # default 3
    prompt: |
      Fix the review comments and commit the changes.
      Reply with the requested JSON describing each thread.
    allow_commit: true
    timeout: 30m                    # required; bounds the whole loop
    output_file: review-report.json # optional iteration report in the session artifacts
```

In bot mode, each iteration waits for a review of the current head commit by a listed bot, then fetches unresolved threads started by those bots (all pages). In all-reviewer mode, it immediately fetches eligible unresolved threads without waiting for a review. Either mode runs the prompt with the head OID and thread data appended. The prompt must answer with JSON only: `{"actions":[{"thread_id":"...","reply":"optional","resolve":true}]}`. Only listed thread IDs are accepted, each at most once, with a non-empty `reply` or `resolve: true`, and only where `viewerCanReply` / `viewerCanResolve` allow it. Cruise itself posts the replies and resolves the threads through `gh api graphql` (requires an authenticated `gh`). Commits made by the prompt are pushed; uncommitted changes fail the step. The loop ends when no eligible unresolved threads remain and fails when eligible threads remain after `max-iterations`, on timeout, or on any API or response error. Unlike other `after-pr` errors, which are warning-only, this `github-review` failure marks the run `Failed`; cancellation and missing-model errors from jcode after-pr prompts also propagate, with missing-model errors failing the run/session. Rerunning resumes safely because replies carry an idempotency marker. The step is valid only directly in `after-pr`.

The default `reviewers: bots` preserves that behavior and requires a nonempty, unique `bots` list. To process existing human and bot feedback instead, use `github-review: { reviewers: all, max-iterations: 3 }` and omit `bots` or specify an empty list. This mode does not wait for a new review. Before passing thread text to the agent, Cruise checks each author's repository collaborator permission: only `write`, `maintain`, or `admin` authors are eligible. A 404 means no permission; other permission lookup failures fail the step. Excluded unresolved thread IDs are recorded in the report and do not block success. The loop succeeds when no eligible unresolved threads remain, fails if an iteration leaves the eligible ID set unchanged, and fails if the iteration limit is reached with eligible threads remaining. The builtin PR review workflow uses this mode with a 60-minute timeout, three iterations, and `allow_commit: true`.

## Variable Reference

Commit and PR templates use a separate [allowed vocabulary](configuration.md#commit-and-pr-templates), which does not include `{plan}`. The `{plan}` below remains a session plan file path.

| Variable | Description |
|----------|-------------|
| `{input}` | Initial input from CLI argument or stdin; for `--skip-planning` / input-as-plan sessions (`input_as_plan: true`) it resolves to the current `plan.md` content |
| `{prev.output}` | Previous prompt output, or a parallel block's JSON results keyed by child name |
| `{prev.input}` | User text input from the previous option step |
| `{prev.stderr}` | Previous command/prompt stderr, or a parallel block's combined stderr and execution errors prefixed by child name |
| `{prev.success}` | Previous command success, or parallel block success (`true`/`false`; skipped children count as successful) |
| `{plan}` | Session plan file path (set automatically by `cruise run`) |
| `{file:name}` | UTF-8 contents of a relative session artifact named `name`; prompt fields only, loaded immediately before execution |
| `{plan.language}` | Effective language used for built-in planning prompts (from `CRUISE_LANGUAGE_PLAN`, `languages.plan`, the legacy field, locale inference, or the default) |
| `{pr.number}` | Pull request number, available after a PR has been created |
| `{pr.url}` | Pull request URL, available after a PR has been created |
| `{pr.language}` | Effective language used for PR title/body generation (from `CRUISE_LANGUAGE_PR`, `languages.pr`, the legacy field, locale inference, or the default) |

> **Note:** `{model}` is **not** a template variable -- it is a special placeholder resolved only within the top-level `command` array. It is not available inside `prompt`, `prompt_file`, `instruction`, or `command` step fields.

Literal braces are escaped Rust-`format!`-style: `{{` -> `{` and `}}` -> `}` (e.g. `"{{input}}"` resolves to the literal string `"{input}"`, not a lookup of `input`). An unclosed `{`, a lone `}`, or an empty `{}` is a template syntax error, as is referencing an undefined variable.
