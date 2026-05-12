# Step groups

A way to bundle multiple steps together and form a group-level retry loop. Group steps are defined inline; the main `steps:` section invokes them with `group: <name>`.

## Basics

```yaml
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

A call-site step (one with `group: <name>`) must stay a pure invocation. Adding `prompt` / `command` alongside is a validation error.

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
- The legacy style of writing `prompt` / `command` alongside `group:` on a call-site step is rejected.
- Empty groups (`steps: {}`) are a validation error.
- References to undefined groups are a validation error.
- A group-level `if:` cannot contain `no-file-changes` (see [flow-control.md](flow-control.md)).

## Group execution behavior

- When `if: file-changed` targets a step and group execution modifies files, execution jumps back to the **first step of the group** and the whole group re-runs.
- `max_retries` caps the number of group-level loop iterations. When the cap is reached, the workflow continues normally (to the next step).
