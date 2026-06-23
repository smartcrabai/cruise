# `after-pr`: steps that run after PR creation

Steps that run automatically after `cruise run` creates a pull request via `gh pr create`. The format is identical to top-level `steps:` — prompt, command, option, and group calls are all supported.

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

## Constraints

- **Errors are downgraded to warnings**: if an `after-pr` step fails, the workflow continues (no fail-fast). The model fits side effects like pushing labels, posting notifications, etc.
- **`fail-if-no-file-changes` is forbidden**: because failures are downgraded, a fail directive is meaningless and is explicitly rejected.
- **`if.no-file-changes` is forbidden**: rejected for the same reason.
- **`if.fail` is forbidden**: rejected for the same reason.

Regular transition rules (`next` / `skip` / `when.exists` / `if.file-changed`) work as usual.

## Related: `cleanup_after_pr`

To automatically delete the local git worktree and branch after the PR is created, set `cleanup_after_pr: true` at the top level. This runs after all `after-pr` steps complete. See [top-level.md](top-level.md#cleanup_after_pr) for details.
