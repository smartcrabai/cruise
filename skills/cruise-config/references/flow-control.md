# Flow control

Mechanisms for controlling transitions between steps. By default, steps run in YAML declaration order; `next` / `skip` / `when` / `if` / `timeout` express branches, loops, skips, and failure handling.

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

For option steps, each option's `next:` selects the jump target. An option with `next: ~` (null) does **not** end the workflow — it falls through to the next step in YAML declaration order (the workflow ends only when the option step is the last one).

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

## `when: exists:` — skip unless a file matches

A pre-execution condition: the step is skipped when **no** file matches the glob. Relative globs are evaluated against the workflow working directory; absolute globs are used as-is. Variable references inside the glob are resolved first.

```yaml
steps:
  migrate:
    when:
      exists: "migrations/*.sql"   # skipped when no .sql file exists
    command: ./run-migrations.sh
```

- Usable on regular steps, `after-pr` steps, and steps inside group definitions.
- An empty glob, or a syntactically invalid glob, is a validation error. Globs containing `{...}` variable references skip static validation and are checked at runtime instead.
- I/O errors during glob evaluation are treated as "matched" (the step runs) with a warning.
- `skip:` is evaluated first; `when.exists` glob I/O only happens when the step is not already skipped.

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

## `if.no-file-changes` — detect no-change

Specifies behavior when a step completes without modifying any workspace files. The value is either `retry` or `failed`.

```yaml
steps:
  implement:
    prompt: "Implement {plan}"
    if:
      no-file-changes: failed        # abort with Failed if no files changed

  fix:
    prompt: "Fix the issue"
    if:
      no-file-changes: retry         # re-execute the same step until files change
```

### Constraints on `if.no-file-changes`

- The value must be `retry` or `failed`; any other value (including the removed object form `{ fail: true }` / `{ retry: true }`) is a parse error.
- Cannot be used inside `after-pr` steps.
- Cannot be used in a group-level `if:`.
- Can coexist with `if: file-changed` on the same step, but then `file-changed` is **ignored entirely** — `no-file-changes` takes over change detection.

> Migration: the legacy `fail-if-no-file-changes: true` field is rejected as an unknown field. Use `no-file-changes: failed` under the step's `if:` instead.

### Declaring intentional no-changes

Sometimes a step legitimately makes no file changes — for example, the plan explicitly says "don't add tests for this step" — and the agent reaching that conclusion again on every retry is not a bug to route around, it's the correct answer being rejected. Two ways to tell cruise the no-change is deliberate, both of which disable **both** actions (`failed` and `retry`) for that attempt (a `failed` step does not abort; a `retry` step does not re-run):

1. **Output marker** — the step's raw output contains a line starting with `NO_CHANGES_INTENTIONAL:` (leading whitespace on the line is ignored; trailing text on that line is the reason). Works with every backend — `command:` mode and both `sdk:` modes — since it's plain text scanning with no tool support required. A mid-line mention (e.g. quoted in passing, or inside a code block) does **not** count — the marker must anchor the start of a line.

   ```yaml
   steps:
     write_tests:
       prompt: "Add tests per {plan}. If the plan says not to add tests here, reply with a line starting with `NO_CHANGES_INTENTIONAL: <reason>` instead."
       if:
         no-file-changes: retry
   ```

2. **`skip_step` tool** (SDK mode only) — the agent calls `skip_step(reason)` instead of relying on text matching. Schema-validated, so it can't be missed by an accidental typo in the marker. Only registered on prompt steps that carry an `if.no-file-changes` condition — see `sdk.md` for why (registering custom tools narrows SDK-mode provider resolution to tool-capable backends, so it's kept opt-in per step). Not available in classic `command:` mode: `run_command` never sees the tool list, so a `command:` step can only use the output marker.

Either path logs the declared reason (`intentional no-changes declared: <reason>`) so the decision stays visible in the run output even though it silently changes what would otherwise be a failure/retry.

## `if: fail:` — failure handler

Specifies what to do when the step **fails**: a command exits non-zero, the step times out (see `timeout` below), the prompt errors, or a `no-file-changes: failed` directive triggers. The value is either a step name (jump) or `{ retry: true }` (re-execute the same step).

```yaml
steps:
  test:
    command: cargo test
    if:
      fail: fix-test-error      # jump to fix-test-error when cargo test fails

  flaky_fetch:
    command: ./fetch.sh
    timeout: 5m
    if:
      fail:
        retry: true             # re-execute the same step on failure/timeout
```

### Constraints on `if.fail`

- Cannot be used inside `after-pr` steps.
- Cannot be used in a group-level `if:`.
- With `if.fail` set, a prompt-step error is caught and routed to the handler instead of aborting the workflow.
- Without `if.fail`, a failed command step does **not** abort the workflow — it proceeds normally and the next step can branch on `{prev.success}` / `{prev.stderr}`. A `no-file-changes` failure without `if.fail` aborts the workflow.

## `timeout:` — per-step time limit

Plain digits mean seconds; `m` / `h` suffixes mean minutes / hours. Applies to prompt and command steps.

```yaml
steps:
  long_task:
    prompt: "Refactor the module"
    timeout: 30m
```

- `"30"` = 30 seconds, `"5m"` = 5 minutes, `"1h"` = 1 hour. Empty, zero, or other suffixes are validation errors.
- A timed-out step is treated as **failed**: `if.fail` (if set) handles it; otherwise the workflow continues to the next step (like a failed command).

## Transition rules summary

1. If `skip` resolves to true, or `when.exists` matches no file → skip and go to next.
2. Execute the step (bounded by `timeout` if set).
3. Step failed (non-zero exit / prompt error / timeout / `no-file-changes: failed`) and `if.fail` is set → jump to its target, or re-execute on `retry: true`.
4. `no-file-changes: failed`, no files changed, and no `if.fail` → stop with Failed.
5. `if.file-changed` and files changed → jump to target (ignored when `if.no-file-changes` is also set).
6. `no-file-changes: retry` and no files changed → re-execute the same step.
7. An option step's selected `next:` → go to that step (`next: ~` falls through to rule 8/9).
8. If `next:` is set → go to that step.
9. Otherwise → go to the next step in YAML declaration order.
10. If there is none → end the workflow.

## Loop protection

Every transition edge (`from → to` pair) is counted. When the same edge is taken more than `--max-retries` times (default: 3), the workflow aborts with an error listing the edge counts. This bounds all loops built from `next:` / `if.file-changed` / `retry`.
