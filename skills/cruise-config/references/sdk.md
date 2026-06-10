# The seher SDK backend

Setting `sdk: seher` at the top level runs prompt steps in-process through the seher SDK instead of spawning an external `command`. `"seher"` is currently the only supported value.

```yaml
sdk: seher

model: build              # seher mode_key for prompt steps (default: build)
plan_model: plan          # seher mode_key for the built-in plan step (default: plan)

steps:
  implement:
    prompt: "{input}"
```

## Mutual exclusivity with `command`

Exactly one of `command` / `sdk` must be set:

- Both set → validation error (`sdk` and `command` are mutually exclusive).
- Neither set → validation error (nothing to run prompts with). An empty `command` array counts as "not set", so an `sdk`-only config is valid.

## Model fields become mode keys

In SDK mode, the `model` / `plan_model` / per-step `model` fields are **reinterpreted as seher mode keys** (which provider/model to use is resolved from the seher configuration, not from a model name):

| Context | Resolution order | Default |
|---------|------------------|---------|
| Ordinary prompt step | step `model` → top-level `model` | `build` |
| Built-in plan step | `plan_model` → top-level `model` | `plan` |

## Provider resolution and rate limits

Before each SDK prompt run, seher resolves a non-rate-limited provider for the mode key. When every provider is rate-limited, cruise waits and re-polls every 60 seconds until one becomes available (the wait is cancellable with Ctrl-C).

## Differences from command mode

- **`env` does not apply to prompt steps**: there is no spawned process, so top-level and per-step `env:` values are not passed to SDK prompt runs. (`command` steps still spawn a shell and receive `env` as usual.)
- **`{model}` placeholder is irrelevant**: it only exists for the `command` array.
- **Interactive planning**: during `cruise plan`, the SDK agent gets custom planning tools — `ask_user` (ask the user a clarifying question), `submit_plan` (write the plan markdown), and `update_plan` (find/replace a section of the existing plan). In non-interactive runs (no TTY), only `submit_plan` is available and the agent proceeds on assumptions. The interview-style `cruise plan --grill` mode is built on `ask_user` and therefore only works with an `sdk:` config (cruise errors out on a command-backend config).
- **Run steps execute autonomously**: ordinary prompt steps get no custom tools; the agent's built-in tools do the file editing.

Command and option steps behave identically in both modes.
