# Top-level structure

```yaml
command:                  # Required: LLM invocation command (array)
  - claude
  - --model
  - "{model}"
  - -p

model: sonnet             # Optional: default model for prompt steps
plan_model: opus          # Optional: model for the built-in plan step
pr_language: English      # Optional: language for auto-generated PR title/body (default: English)

llm:                      # Optional: OpenAI-compatible API for session-title generation
  api_key: sk-...
  endpoint: https://api.openai.com/v1
  model: gpt-4o-mini

env:                      # Optional: environment variables applied to every step
  API_KEY: sk-...

groups:                   # Optional: step group definitions (see references/groups.md)
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify

steps:                    # Required: workflow steps (declaration order = execution order)
  step_name:
    # ...

after-pr:                 # Optional: steps that run after PR creation (see references/after-pr.md)
  step_name:
    # ...
```

Only `command` and `steps` are required. `steps` is held as an `IndexMap`, so declaration order is the execution order.

## `command` and the `{model}` placeholder

`{model}` inside the `command` array is a special placeholder resolved at runtime. It is **not** a template variable and cannot be used inside `prompt` / `instruction` / `command` step fields.

- When an effective model is set: `{model}` is replaced with the model name.
- When no model is set: both `{model}` and its immediately preceding `--model` flag are removed automatically.

A step-level `model:` overrides the top-level `model:` for that step only.

```yaml
command:
  - claude
  - --model
  - "{model}"      # resolved at runtime; `--model {model}` is stripped if no model is set
  - -p

model: sonnet      # default

steps:
  planning:
    model: opus    # this step uses opus
    prompt: "Plan: {input}"
```

## `plan_model`

Model used by the built-in plan step (driven by `cruise plan`). Falls back to `model` if unset.

## `pr_language`

Language used for the auto-generated PR title and body. Defaults to `English`.

```yaml
pr_language: Japanese     # PR title/body generated in Japanese
```

## Hot-reload

During `cruise run`, the config file's mtime is checked between steps and the file is reloaded automatically when changed.

- Does not apply to sessions started from the built-in default.
- The current step must still exist in the new config.

## Rate-limit retry

When an HTTP 429 is detected, cruise retries with exponential backoff:

- Initial delay: 2 seconds
- Max delay: 60 seconds
- Default retry count: 5 (override with `--rate-limit-retries`)
