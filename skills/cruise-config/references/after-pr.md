# `after-pr`: steps that run after PR creation

Steps that run automatically after `cruise run` creates a pull request via `gh pr create`. The format is identical to top-level `steps:` — inline or file-backed prompt steps (`prompt` / `prompt_file`), command steps, option steps, `workflow_call` call sites, and group calls are all supported.

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


## Constraints

- **Errors are downgraded to warnings**: if an `after-pr` step fails, the workflow continues (no fail-fast). The model fits side effects like pushing labels, posting notifications, etc.
- **`if.no-file-changes` is forbidden**: rejected for the same reason.
- **`if.fail` is forbidden**: rejected for the same reason.

Regular transition rules (`next` / `skip` / `when.exists` / `if.file-changed`) work as usual.

## Related: `cleanup_after_pr`

To automatically delete the local git worktree and branch after the PR is created, set `cleanup_after_pr: true` at the top level. This runs after all `after-pr` steps complete. See [top-level.md](top-level.md#cleanup_after_pr) for details.
