# `after-pr`: steps that run after PR creation

Steps that run automatically after `cruise run` creates a pull request via `gh pr create` (or `glab mr create` for GitLab; detect it with a `case "{pr.url}" in */-/merge_requests/*)` branch and use `glab` instead of `gh`, as the builtin config does), or on the existing PR in a PR review-fix session. The format is identical to top-level `steps:` — inline or file-backed prompt steps (`prompt` / `prompt_file`), command steps, option steps, parallel prompt/command blocks, `workflow_call` call sites, and group calls are all supported. Built-in `merge` opt-in also runs the configured after-PR prelude before its native final squash step.

The six general-purpose built-ins declare `after_pr` and `merge`, both off by default; `tdd` and `refactor` additionally declare `browser_e2e`, `app_e2e`, and `formal_methods`. `pr-review-fix` declares no flags. `-f after_pr` runs the after-PR prelude without merging. `-f merge` runs that same prelude once, then attempts the final native squash step. The implication does not add or change an `after_pr` entry in the saved sparse flag map.

```sh
cruise plan -c builtin:tdd -f merge "task"
cruise plan -c builtin:tdd -f browser_e2e,app_e2e,merge "task"
```

## Basics

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

## `{pr.*}` variables

PR creation info becomes available inside `after-pr` steps.

| Variable | Description |
|----------|-------------|
| `{pr.number}` | PR number |
| `{pr.url}` | PR URL |

Regular variables (`{input}`, `{plan}`, etc.) remain usable.

Prompt steps in `after-pr` are commit-guarded by default just like prompts in the main workflow. If an after-PR prompt must intentionally update the PR branch, set `allow_commit: true` on that prompt:

```yaml
after-pr:
  resolve-conflict:
    allow_commit: true
    prompt: "Resolve the conflict, stage the files, and commit"
```
This opt-out is limited to that prompt step and bypasses all commit-guard behavior for it. Guarded prompt attempts to move `HEAD` fail with a commit-guard violation; cruise may restore the original branch reference without resetting the index or worktree, but the step still fails. Command and option steps are not guarded. `allow_commit: true` cannot be placed on a group or workflow-call invocation; put it on the expanded prompt step.


## `github-review`

A prompt step in `after-pr` with `github-review:` processes eligible unresolved threads and applies the prompt's reply/resolve decisions, repeating up to `max-iterations` (default `3`) until no eligible threads remain. Bot mode waits for a listed bot to review the PR head before fetching its threads. All-reviewer mode immediately fetches eligible unresolved threads without waiting. `timeout` bounds the whole loop. Unlike other after-pr steps, a failure here fails the run.

```yaml
after-pr:
  address-review:
    github-review:
      bots: ["copilot-pull-request-reviewer"]
      max-iterations: 3
    timeout: 30m
    output_file: review-report.json # Optional iteration report
    prompt: "Address the review threads on PR #{pr.number}"
```

Rules: only directly in `after-pr` (not in `steps`, groups, or `parallel` children), exactly one of `prompt` / `prompt_file`, `timeout` required, `bots` unique, and no `command` / `option` / `parallel` / `group` / `workflow_call`.

`reviewers: bots` is the default and requires a nonempty `bots` list. Alternatively, `reviewers: all` accepts omitted or empty `bots` and immediately processes existing unresolved threads without waiting for a review. Before the prompt sees thread text, Cruise checks each author's collaborator permission; only `admin`, `maintain`, and `write` are eligible. A 404 is treated as no permission, while other lookup failures fail the step. Excluded unresolved IDs appear in the report and do not prevent success. The loop succeeds when no eligible threads remain, fails when an iteration leaves the eligible unresolved ID set unchanged, and fails if eligible threads remain at `max-iterations`. `builtin:pr-review-fix` uses this mode with `allow_commit: true`, a 60-minute timeout, three iterations, and `review-report.json` as its output.

```yaml
after-pr:
  address-review:
    github-review: { reviewers: all, max-iterations: 3 }
    timeout: 60m
    allow_commit: true
    output_file: review-report.json
    prompt: "Address the eligible review threads on PR #{pr.number}"
```

## Opt-in native synchronous squash merge

The six general-purpose built-ins have an explicit, default-off `merge` flag. The built-in after-PR prelude uses `flag: [after_pr, merge]` on its `follow-up` workflow call, propagated to all six expanded steps, so either flag runs it; the native final action is gated only by `merge`. `-f merge` therefore runs the prelude even when `after_pr` is off, and the prelude runs once. `-f after_pr` alone never merges. No PR is created or merged when the workflow has `create_pr: false` (including `research`), uses `exec`/current-branch mode, or has no commits eligible for PR creation; opted-in actions are skipped with a reason. The merge flag does not override read-only or no-PR policy.

The six general-purpose built-ins share the ordered prelude `pr-ready` →
`sync-base` → `resolve-conflict` → `push-after-sync` → `wait-ci` →
`fix-ci-error`. Each caller passes its conflict prompt and task-specific scope
into the shared resource, which owns the common CI repair prompt. `wait-ci`
allows up to 60 minutes. If CI is still running at its deadline with no failed
rollup, it reports pending without invoking repair. If a GitHub rollup is
already failing, `wait-ci` keeps polling attached Actions suites until they
finish or the deadline; the deadline preserves the failure and invokes repair.

On failure, `fix-ci-error` receives the prior wait diagnostic, enumerates all
non-success checks for the current head, and reads logs (including canceled or
timed-out jobs). If diagnostics are empty or truncated, it queries the
provider directly. It reproduces the corresponding CI commands, fixes all
genuine in-scope failures in one pass, and runs relevant formatters, linters,
and tests before committing. Unrelated or flaky jobs are rerun once or
reported clearly; a repair with no file changes does not loop. Changed fixes
still route through base synchronization, push, and another CI wait. New
expanded IDs are `follow-up/pr-ready`, etc. The native `squash-merge` remains
the final direct root step, never inside the callee. Existing session
snapshots and saved IDs are not migrated.

Installed binaries resolve this internal shared resource from embedded data,
without filesystem fallback. Direct local loads of `builtin/*.yaml` use normal
relative files. Ejected workflows are self-contained and contain the expanded
steps, not shared-resource dependencies, so they work outside the source tree.

The native action is a direct final step in `after-pr`, never a prompt or command:

```yaml
flags:
  merge:
    description: Run after-PR automation and synchronously squash-merge

after-pr:
  squash-merge:
    squash-merge: true
    flag: merge
    timeout: 10m
    if:
      fail: follow-up/sync-base
```

`squash-merge: true` is valid only on the final direct step in `after-pr`, with exactly `flag: merge` (a single-name string or one-element list). It is forbidden in the main workflow, group definitions or calls, workflow calls, and parallel parents or children. Do not add `prompt`, `prompt_file`, `command`, `option`, `output_file`, `model`, `computer_use`, `permission`, or `allow_commit`; compatible common fields such as `env`, `timeout`, `skip`, and `when` may be used. The timeout bounds the synchronous operation.

`if.fail` is supported in `after-pr` only on the native `squash-merge: true` step; it remains invalid on every other after-PR step. In the built-ins, this handler routes confirmed GitHub behind/diverged ancestry or `CONFLICTING`/`DIRTY` mergeability into the existing recovery path. It does not make other readiness failures retryable.

The native operation supports GitHub and GitLab and fails closed unless fresh provider state and local state are ready: the PR/MR must be open and non-draft, the worktree clean, local `HEAD` equal to the remote source head, applicable CI checks for that source head passed, required reviews and provider policy satisfied, and mergeability current. Pending, unknown, failed, or otherwise unfulfilled requirements reject the merge, except for the confirmed GitHub base-sync cases below. The merge request includes an expected-head guard so a head race cannot merge a different revision. It requests squash synchronously; it does not enable auto-merge, bypass protections/admin rules, use a merge queue, or request source-branch deletion. GitHub uses its synchronous REST merge endpoint via `gh api`, not `gh pr merge`; queues that cannot preserve synchronous squash semantics are rejected. A same-head PR/MR already merged is idempotently accepted without another write.

On GitHub, the native merge uses the same CI readiness gate as the built-in `follow-up/wait-ci`: the PR head and an existing `statusCheckRollup` must match the expected local `HEAD`, and the rollup state must be `SUCCESS`. Every GitHub Actions check suite on that commit with a `workflowRun` must be `COMPLETED`; a completed suite with a failing conclusion fails CI. Suites without `workflowRun` (for example, from third-party apps) do not block. Unregistered rollups or pending checks are not ready. CI readiness must hold for two consecutive polls; any non-ready poll resets the streak. A PR-head mismatch stays pending for up to 90 seconds to allow API propagation; if it catches up in time, readiness can succeed, but a persistent mismatch after the grace or overall deadline fails. The built-in `wait-ci` keeps its existing `head_mismatch` diagnostic. It polls every 5 seconds for up to 29 minutes within its 30-minute step timeout; native GitHub merge polls every 2 seconds for up to 10 minutes, subject to a shorter step timeout. These GitHub-specific details do not change GitLab readiness behavior.

Immediately before a native GitHub merge, Cruise uses GitHub's compare API to check whether the PR includes the current base. A confirmed behind/diverged result, or `CONFLICTING`/`DIRTY` mergeability, is a distinct retryable outcome. The built-in native step routes it with `if.fail: follow-up/sync-base`, then through `sync-base` → `resolve-conflict` → `push-after-sync` → `wait-ci` → `fix-ci-error`. Only native `ResyncRequired` routing from `squash-merge` to its `if.fail` target starts a fresh after-PR pass: it resets `budgeted_traversals` for every after-PR edge except `squash-merge` → target, preserving traversal history. The resync edge remains capped at `max_retries`; generic `if.fail` retains shared budgets. Worst case: up to `max_retries` resyncs, each pass allowing up to `max_retries - 1` CI-fix iterations; zero retry budget permits no resync. Other readiness failures still fail, and the ancestry check/retry routing are GitHub-only; GitLab behavior is unchanged.

For private GitHub repositories, the plan limitation is recognized only when branch-rule lookup returns a structured HTTP 403 with the exact message `Upgrade to GitHub Pro or make this repository public to enable this feature.` Current PR, CI, and review checks still apply; Cruise submits only the guarded synchronous merge request, and GitHub enforces branch rules that could not be listed. Other lookup failures remain fatal.

After the request, Cruise rechecks provider state and reports success only when the expected head is confirmed merged. A failed or unknown native merge outcome, or any fatal after-PR error, keeps the session and worktree and prevents `cleanup_after_pr` cleanup; inspect or resume it manually rather than assuming a merge. When `merge` is off, warning-only failures from ordinary after-PR steps do not by themselves prevent configured cleanup.

The existing **Merge PR** action in `cruise list`, the TUI, and the WebUI remains a separate human-confirmed operation with its existing cleanup behavior and GitHub-only limitations. Native workflow merge is not a replacement or global default.

## Constraints

- **After-PR errors:** without native merge opt-in, errors from ordinary `after-pr` steps are warnings except cancellation, missing jcode models, and `github-review` failures, which propagate. With native merge opted in, actual execution errors anywhere in the after-PR run, including before the native step, are fatal and suppress post-PR cleanup. A nonzero command exit status remains command-as-data under existing semantics; the native action independently rechecks current readiness instead of trusting command output.
- **`if.no-file-changes` is forbidden**: rejected for the same reason.
- **`if.fail`:** supported only on the native `squash-merge: true` step; rejected on all other `after-pr` steps.

Regular transition rules (`next` / `skip` / `when.exists` / `if.file-changed`) work as usual.

## Related: `cleanup_after_pr`

To automatically delete the local git worktree and branch after the PR has been created, set `cleanup_after_pr: true` at the top level. This runs after all after-PR steps complete. A fatal after-PR error or unconfirmed native merge outcome retains the session and worktree and prevents cleanup; warning-only failures from ordinary after-PR steps with native merge off do not by themselves prevent cleanup. See [top-level.md](top-level.md#cleanup_after_pr) for details.
