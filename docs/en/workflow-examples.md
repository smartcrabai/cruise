# Workflow examples

**Languages:** [English](../en/workflow-examples.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/workflow-examples.md) · [简体中文](../zh/workflow-examples.md) · [Index](README.md)

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
max_retries: 4

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

groups:
  fix-loop:
    if:
      file-changed: test    # if the fix modified files, rerun the tests
    max_retries: 2          # retries exhausted -> continue to commit
    steps:
      apply-fix:
        prompt: |
          The following test errors occurred. Please fix them:
          ---
          {prev.stderr}

steps:
  implement:
    prompt: "{input}"

  test:
    command: cargo test

  fix:
    group: fix-loop

  commit:
    command: "git add -A && git commit -m 'feat: {input}'"
```

Conditional cycles are allowed when a normal exit is possible. Execution preflight rejects a reachable cycle only when no normal exit is reachable from the actual start position, considering configured and user-selected skips. A branch that may enter an exitless cycle produces a warning, not a blanket rejection. Runtime edge budgets still apply.
