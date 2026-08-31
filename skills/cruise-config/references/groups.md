# Step groups

A way to bundle multiple steps together and form a group-level retry loop. Group steps are defined inline; the main `steps:` section invokes them with `group: <name>`.

## Basics

```yaml
max_retries: 4

groups:
  review:
    if:
      file-changed: test    # if any group step changes files, jump back to the first group step
    max_retries: 3          # max number of group-level loop iterations (optional)
    steps:                  # inline definition of group steps
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  test:
    command: cargo test
  review-pass:
    group: review           # run all steps of the "review" group here
```

## Multiple call sites for the same group

The same group can be invoked from different positions in the workflow.

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

## Group-call steps

A call-site step (one with `group: <name>`) must stay a pure invocation. Adding `prompt` / `prompt_file` / `command` alongside is a validation error.

```yaml
# OK
steps:
  call:
    group: review

# NG — validation error
steps:
  call:
    group: review
    prompt: /something      # cannot coexist with group
```

## Validation rules

- Steps inside a group definition cannot have nested `group:` references.
- Steps inside a group definition cannot have an individual `if:` (the group's `if:` applies to the whole group).
- Call-site steps cannot have an individual `if:`.
- `allow_commit: true` is rejected on a group call site because the call site cannot override the expanded prompt steps; set it on the inner prompt instead.
- Empty groups (`steps: {}`) are a validation error.
- References to undefined groups are a validation error.
- A group-level `if:` cannot contain `no-file-changes` or `fail` (see [flow-control.md](flow-control.md)); only `file-changed` is allowed at the group level.
- If `if.file-changed` targets a step outside the group, the group's `max_retries` requires one additional unit of the global loop-protection ceiling.
- A group `if.file-changed` back-edge that closes a top-level step cycle is rejected at startup unless the group sets `max_retries` — without it the jump has no graceful skip and counts as an unsafe conditional edge (see [Loop protection](flow-control.md#loop-protection)).

## Group execution behavior

- When `if: file-changed` targets a step and group execution modifies files, execution jumps back to the **first step of the group** and the whole group re-runs.
- `max_retries` caps the number of group-level loop iterations. When the cap is reached, the workflow continues normally (to the next step).
