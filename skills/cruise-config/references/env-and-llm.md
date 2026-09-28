# Environment variables and session titles

## Env-var merge rules

`env:` supports workflow and step levels, plus a child level inside `parallel` blocks. Values are subject to template variable resolution.

- Top-level `env:` applies to every step.
- Per-step `env:` is merged on top of the top-level map; identical keys are overridden.
- A parallel child merges its `env:` over the parent block: workflow < parallel block < child. Each child resolves its own values against the incoming variables. Overrides stay local to that child; already-resolved inherited values are not expanded again.

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

**SDK mode**: top-level and per-step `env:` values are passed to SDK prompt execution. Command steps receive `env:` in their child process (see [sdk.md](sdk.md)).

## OpenAI priority processing with jcode

For `sdk: jcode`, `JCODE_OPENAI_SERVICE_TIER` controls the OpenAI service tier. Cruise defaults it to `off` when neither workflow `env:` nor the process environment defines the key. Set it to `"priority"` or `"flex"` in workflow `env:` to opt in; step-level `env:` overrides workflow-level `env:` for that step.

## Process-level config overrides

The CLI and WebUI apply these environment variables when loading a
workflow config: `CRUISE_MODEL`, `CRUISE_PLAN_MODEL`, `CRUISE_SDK`,
`CRUISE_LANGUAGE_PR`, `CRUISE_LANGUAGE_PLAN`, `CRUISE_CLEANUP_AFTER_PR`,
`CRUISE_INTERACTIVE_PLANNING`, and `CRUISE_FORCE_EXEC`. String values are
trimmed and blank values are ignored. Boolean values must be `true`, `false`,
`1`, or `0`; any other value is an error naming the offending variable.

`CRUISE_LANGUAGE_PR` and `CRUISE_LANGUAGE_PLAN` override the corresponding
`languages.pr` and `languages.plan` settings. When these variables are unset,
nested language fields take precedence over the deprecated top-level fields,
then the first non-empty variable among `LC_ALL`, `LC_MESSAGES`, `LANG`, and `LANGUAGE` is mapped once; if that value is not a supported locale, no other variable is consulted and the default is `English`. Unsupported or language-neutral locales use `English`.

## Session title generation

After plan approval, cruise sets a concise session title of at most 80 characters for `cruise list` and the WebUI sidebar:

- **SDK mode** (`sdk: jcode`, `sdk: claude`, or the default jcode backend) makes a separate title-generation request only on the foreground `cruise plan` approval path (interactive **Approve/Execute now** and non-TTY auto-approval), using `plan_model`, then `model`, then the backend default. Title-generation failures fall back to `plan.md` metadata: the first heading, or the first content line with list markers stripped.
- **Command mode** derives the title from the first heading or first non-empty line of `plan.md`; it makes no separate title-generation call.
- `cruise list` **Approve**, WebUI/TUI approval, and background `cruise --plan` planning completion use the `plan.md` metadata fallback directly: first heading, else first content line with list markers stripped. Background `cruise --plan … --skip-planning` also derives the title directly from `plan.md` and makes no model call.

No `llm:` workflow field or `CRUISE_LLM_*` override exists. Title generation uses the configured execution backend and model resolution above.
