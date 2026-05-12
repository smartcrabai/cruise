# Flow control

Mechanisms for controlling transitions between steps. By default, steps run in YAML declaration order; `next` / `skip` / `if` express branches, loops, and skips.

## `next:` — explicit jump

```yaml
steps:
  step_a:
    command: echo "hello"
    next: step_c              # skip step_b and jump to step_c
  step_b:
    command: echo "skipped"
  step_c:
    command: echo "world"
```

For option steps, each option's `next:` is effectively required (`next: ~` ends the workflow).

## `skip:` — skip a step

Accepts either a static boolean or a variable reference.

```yaml
steps:
  always_skipped:
    command: cargo fmt
    skip: true                # static boolean

  conditional:
    command: cargo fix
    skip: prev.success        # variable name (step is skipped if the value resolves to "true")
```

With a variable reference, the step is skipped when the variable's current value is the string `"true"`.

## `if: file-changed:` — loop back

Before the step runs, cruise takes a snapshot of the workspace. If files change during execution, the workflow jumps to the named target. A common pattern is rerunning tests whenever a review step modifies code.

```yaml
steps:
  test:
    command: cargo test
  review:
    prompt: "Review and fix issues."
    if:
      file-changed: test      # if review modified files, jump back to test
```

When no files change, the workflow proceeds to the next step normally (or follows `next:` if set).

## `if: no-file-changes:` — detect no-change

Specifies behavior when a step completes without modifying any workspace files. `fail` and `retry` are mutually exclusive.

```yaml
steps:
  implement:
    prompt: "Implement {plan}"
    if:
      no-file-changes:
        fail: true            # abort with Failed if no files changed

  fix:
    prompt: "Fix the issue"
    if:
      no-file-changes:
        retry: true           # re-execute the same step until files change
```

### Constraints on `if.no-file-changes`

- Exactly one of `fail` / `retry` must be `true`. Both true or both false is a validation error.
- Cannot be used inside `after-pr` steps.
- Cannot be used in a group-level `if:`.
- Cannot be combined with the legacy `fail-if-no-file-changes: true` on the same step.
- Can coexist with `if: file-changed` on the same step, but `no-file-changes` takes priority for change detection.

## `fail-if-no-file-changes` (legacy)

Equivalent to `if.no-file-changes.fail: true`.

```yaml
steps:
  implement:
    prompt: "{input}"
    fail-if-no-file-changes: true
```

Cannot be used inside `after-pr` (validation error). Prefer the new `if.no-file-changes` syntax.

## Transition rules summary

1. If `skip` resolves to true → skip and go to next.
2. Execute the step.
3. `if.no-file-changes.retry: true` and no files changed → re-execute the same step.
4. `if.no-file-changes.fail: true` and no files changed → stop with Failed.
5. `if.file-changed` and files changed → jump to target.
6. If `next:` is set → go to that step.
7. Otherwise → go to the next step in YAML declaration order.
8. If there is none → end the workflow.
