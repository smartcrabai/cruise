# SDK backends: `sdk: seher` / `sdk: pi`

Setting the top-level `sdk` field runs prompt steps in-process instead of spawning an external `command`. Two values are accepted: `"seher"` and `"pi"`. Any other value is a validation error.

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
- `sdk` set to anything other than `seher` / `pi` → validation error.

## `sdk: seher` — routed through seher's provider resolution

Before each prompt run, seher resolves a non-rate-limited provider using its own `~/.config/seher/config.yaml`. `model` / `plan_model` / per-step `model` are **reinterpreted as seher mode keys** (which provider/model to use is resolved from that seher configuration, not from a model name):

| Context | Resolution order | Default |
|---------|------------------|---------|
| Ordinary prompt step | step `model` → top-level `model` | `build` |
| Built-in plan step | `plan_model` → top-level `model` | `plan` |

### Provider resolution and rate limits

When every provider is rate-limited, cruise waits and re-polls every 60 seconds until one becomes available (the wait is cancellable with Ctrl-C).

### Differences from command mode

- **`env` applies to prompt steps**: top-level and per-step `env:` values are forwarded to the selected seher SDK backend. Backends that spawn Claude pass them to the child process; the in-process pi backend applies them through process environment mutation inside seher.
- **`{model}` placeholder is irrelevant**: it only exists for the `command` array.
- **Interactive planning**: during `cruise plan`, the SDK agent gets custom planning tools — `ask_user` (ask the user a clarifying question), `submit_plan` (write the plan markdown), and `update_plan` (find/replace a section of the existing plan). Custom tools require a tool-capable seher SDK (`pi` or `claude`), so this pins planning to those providers. In non-interactive runs (no TTY), `ask_user` is not registered — the prompt instead tells the agent to decide on explicitly stated assumptions — but `submit_plan` and `update_plan` remain available, and a turn that ends without a successful `submit_plan`/`update_plan` call fails instead of falling back to the agent's final message as the plan. The interview-style `cruise plan --grill` mode is built on `ask_user` and therefore requires both an `sdk:` config and interactive planning enabled (`interactive_planning: true`, the default, and no `--no-interactive-planning` flag); cruise errors out otherwise.
- **Run steps execute autonomously**: ordinary prompt steps get no custom tools; the agent's built-in tools do the file editing.

Command and option steps behave identically in both modes.

## `sdk: pi` — pi_agent_rust directly, no seher involved

`sdk: pi` drives `pi_agent_rust` in-process **directly**, bypassing seher's provider-resolution layer entirely. There is no `~/.config/seher/config.yaml` lookup and no seher configuration of any kind is required.

```yaml
sdk: pi

model: anthropic/claude-sonnet-4-6   # plain model reference, not a mode key
plan_model: openai/gpt-5.5:high      # "provider/model[:thinking]"

steps:
  implement:
    prompt: "{input}"
```

### Model fields are plain model references, not mode keys

Unlike `sdk: seher`, `model` / `plan_model` / per-step `model` are **not** reinterpreted — they carry the same precedence as command mode (step `model` > top-level `model` / `plan_model`) but the value itself is a model reference in one of these forms:

| Form | Example | Behavior |
|------|---------|----------|
| `provider/model[:thinking]` | `openai-codex/gpt-5.5:xhigh` | Explicit provider + model; `:thinking` is recognized only when it parses as a pi thinking level (`off`/`low`/`medium`/`high`/`xhigh`/`max`/aliases) — any other `:` suffix (e.g. an OpenRouter `:free` variant) stays part of the model id. |
| `model` (no `/`) | `claude-sonnet-4-6` | Provider left unset; pi searches its own model registry for a model with this id — the same as running the `pi` CLI with `--model` but no `--provider`. |
| unset | *(both `model` and `plan_model` omitted)* | pi auto-selects: it tries its built-in provider/model preference order (Codex, then OpenAI, ... down to Anthropic and others) and picks the first entry with usable credentials. |

### Authentication

Resolved entirely by pi, in this precedence order:

1. An explicit key (not exposed through cruise config).
2. pi's stored `~/.pi/agent/auth.json` OAuth/Bearer credentials.
3. Ambient environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and similar provider-specific vars — pi recognizes about a dozen).

**Stored credentials win over environment variables** — a successful `pi login` is not silently overridden by a stale shell env var. Workflow `env:` (top-level and per-step) is still applied to the process environment before each pi call and participates in step 3.

### Rate limits

A rate limit retries the **same** provider/model with exponential backoff (2s, doubling up to a 60s cap — identical schedule to command mode), up to the configured `--rate-limit-retries` attempts. There is no other provider to fall back to (unlike `sdk: seher`, which re-resolves a different provider on each retry). Each retry starts a **fresh pi session** (the rate-limited attempt's partial session is abandoned on disk, and the step prompt is re-sent from scratch) — resuming a partially-answered session would duplicate context, so a clean re-run is deliberately preferred.

Known limitation (shared with the seher-resolved pi engine): a step `timeout:` or Ctrl-C makes cruise stop waiting, but the in-flight pi call keeps running on its detached worker thread until it completes on its own — there is no cancellation hook in the underlying runner. If the step sets `env:` overrides, that orphaned call also keeps holding a process-wide env lock, so the next `sdk: pi` step with `env:` may block until the abandoned call finishes. Prefer generous `timeout:` values (or none) for `sdk: pi` steps that set `env:`.

### Differences from `sdk: seher`

- No seher configuration file, no provider-resolution polling, no rate-limit-driven provider hopping.
- Interactive planning tools (`ask_user` / `submit_plan` / `update_plan`) and `cruise plan --grill` work the same way — pi always supports custom tools, so there is no tool-incapable pi provider to worry about (unlike seher's `claude-terminal` / `claude-headless`).
- `env` is applied the same way (in-process environment mutation) since pi is in-process either way.

### Endpoint overrides

Custom endpoints / model catalogs are configured through pi's own mechanism: set `PI_CODING_AGENT_DIR` to point pi at an alternate config directory containing a `models.json`. Cruise does not add any endpoint-override fields of its own for `sdk: pi`.
