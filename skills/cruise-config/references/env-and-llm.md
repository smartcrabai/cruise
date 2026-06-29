# Environment variables and LLM API config

## Env-var merge rules

`env:` has two levels: top-level and per-step. Values are subject to template variable resolution.

- Top-level `env:` applies to every step.
- Per-step `env:` is merged on top of the top-level map; identical keys are overridden.

```yaml
env:                        # applied to every step
  ANTHROPIC_API_KEY: sk-...
  TARGET_ENV: production

steps:
  deploy:
    command: ./deploy.sh
    env:                    # merged on top of top-level env
      TARGET_ENV: staging   # overrides production
      LOG_LEVEL: debug
```

Template variables (e.g. `{input}`) can be used inside `env:` values.

**Secrets caveat**: avoid writing real API keys into `env:` values — config files tend to get committed. Prefer exporting secrets in the shell environment and keeping only non-secret values in `env:`.

**SDK mode caveat**: in `sdk: seher` mode, prompt steps receive `env:` through the selected seher backend. Claude subprocess backends pass the values to the child process; the in-process pi backend applies them through process environment mutation inside seher (see [sdk.md](sdk.md)). Command steps still spawn a shell and receive `env:` as usual.

## LLM API config (session-title generation)

After plan approval, cruise can call an OpenAI-compatible API to generate a concise session title (up to 80 characters). The title is shown in `cruise list` and the GUI sidebar.

```yaml
llm:
  api_key: sk-...
  endpoint: https://api.openai.com/v1
  model: gpt-4o-mini
```

### Precedence and environment variables

| Setting | Config field | Environment variable | Default |
|---------|--------------|----------------------|---------|
| API key | `llm.api_key` | `CRUISE_LLM_API_KEY` | (required) |
| Endpoint | `llm.endpoint` | `CRUISE_LLM_ENDPOINT` | `https://api.openai.com/v1` |
| Model | `llm.model` | `CRUISE_LLM_MODEL` | `gpt-4o` |

Environment variables take precedence over the YAML config. To avoid leaking secrets, prefer the `CRUISE_LLM_API_KEY` environment variable.

### Fallback when unset

When `api_key` is not set, the title is derived automatically from the first heading (or the first non-empty line) of the generated `plan.md`.
