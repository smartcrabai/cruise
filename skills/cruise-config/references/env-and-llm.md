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

**SDK mode caveat**: in `sdk: seher` mode, prompt steps receive `env:` through the selected seher backend. Claude subprocess backends and the external pi CLI backend pass values to child processes; RPC backends (`pi` and `omp`) ignore workflow `PATH`/`PATHEXT` overrides so those variables cannot change helper resolution. The in-process `pi-rust` backend applies values through process environment mutation inside seher. Ambient variables are inherited by RPC child processes, and configured/request values override them except for `PATH`/`PATHEXT`. In `sdk: pi` mode (pi runs in-process directly, no seher involved), `env:` is applied the same way, via process environment mutation, before each pi call (see [sdk.md](sdk.md)). Command steps still spawn a shell and receive `env:` as usual.

## Process-level config overrides

The CLI and desktop GUI apply these environment variables when loading a
workflow config: `CRUISE_MODEL`, `CRUISE_PLAN_MODEL`, `CRUISE_SDK`,
`CRUISE_LANGUAGE_PR`, `CRUISE_LANGUAGE_PLAN`, `CRUISE_CLEANUP_AFTER_PR`,
`CRUISE_INTERACTIVE_PLANNING`, and `CRUISE_FORCE_EXEC`. String values are
trimmed and blank values are ignored. Boolean values must be `true`, `false`,
`1`, or `0`; any other value is an error naming the offending variable.

`CRUISE_LANGUAGE_PR` and `CRUISE_LANGUAGE_PLAN` override the corresponding
`languages.pr` and `languages.plan` settings. When these variables are unset,
nested language fields take precedence over the deprecated top-level fields,
then the first supported locale from `LC_ALL`, `LC_MESSAGES`, `LANG`, or
`LANGUAGE`, then the default is `English`. Unsupported or language-neutral
locales use `English`.

## Prompt language environment variables

`CRUISE_LANGUAGE_PR` and `CRUISE_LANGUAGE_PLAN` override the corresponding
`languages.pr` and `languages.plan` settings. Blank values are ignored. When
these variables are unset, nested language fields take precedence over the
deprecated top-level fields, then locale inference, then the default is
`English`.

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
