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

For `sdk: opencode`, workflow and step environment overrides are merged with the inherited OS environment before setting the V2 shell environment, preserving `PATH` and the commit guard. OpenCode's server authentication secret is not exposed to shell tools. Existing OpenCode configuration/authentication is retained; malformed JSONC in `OPENCODE_CONFIG_CONTENT` is an error, not silently discarded. No new `CRUISE_OPENCODE_*` connection setting is needed. `CRUISE_SDK=opencode` selects this backend through the normal override API.

## OpenAI priority processing with jcode

For `sdk: jcode`, `JCODE_OPENAI_SERVICE_TIER` controls the OpenAI service tier. Cruise defaults it to `off` when neither workflow `env:` nor the process environment defines the key. Set it to `"priority"` or `"flex"` in workflow `env:` to opt in; step-level `env:` overrides workflow-level `env:` for that step.

## Process-level config overrides

Process-level configuration overrides apply after workflow and defaults layers. See the [CLI skill's full table](../../cruise-cli/SKILL.md#process-environment-overrides) for all supported variables and their keys. Names normally derive from `CRUISE_` plus the uppercase key path with dots and hyphens replaced by underscores; the language child names below are exceptions, not aliases.

Values are trimmed and blanks ignored. Booleans accept only `true`, `false`, `1`, or `0`; integers use unsigned decimal digits. Models are literal single models unless they begin with `[` or `{`, in which case they are JSON in the field's normal shape. Model arrays enable fallback as in YAML. Composite fields require JSON, not YAML flow syntax; `null` clears an optional field and `[]` clears a list where supported. Parent `CRUISE_RETRY` and `CRUISE_LANGUAGES` apply before their child overrides. `CRUISE_SDK` clears `command`, `CRUISE_COMMAND` clears `sdk`, and setting both nonblank is an error. Invalid values identify the variable and undergo normal validation.

CLI flags and saved session cleanup overrides win where applicable. `CRUISE_RUN_ALL_PARALLELISM` changes the effective app setting for TUI/WebUI run-all without persisting it, not CLI `run --all` (`--parallelism`). File-backed sessions reapply overrides when reloaded; snapshot sessions retain their creation-time configuration. Named or secret-bearing `env`, `mcp_servers`, and `pre_steps`, workflow structure (`steps`, `groups`, `after-pr`, `inputs`, `returns`, `description`, `flags`), `create_pr`, and deprecated `pr_language` / `plan_language` are excluded. Process runtime switches are separate from configuration overrides.

`CRUISE_UI_LANGUAGE` is separate from workflow configuration and prompt language: it selects the CLI/TUI display language ahead of application `uiLanguage`, system locale, and English. It accepts `en`, `zh-CN`, `es`, `fr`, `pt-BR`, `ja`, `ko`, or `auto`; blank and `auto` defer to the next layer. Set the application default with `cruise config --set-language <tag>` or clear it with `--set-language auto`. A WebUI browser's language cookie overrides the server default without changing application config.

`CRUISE_LANGUAGE_PR` and `CRUISE_LANGUAGE_PLAN` override the corresponding
`languages.pr` and `languages.plan` settings. When these variables are unset,
nested language fields take precedence over the deprecated top-level fields,
then the first non-empty variable among `LC_ALL`, `LC_MESSAGES`, `LANG`, and `LANGUAGE` is mapped once; if that value is not a supported locale, no other variable is consulted and the default is `English`. Unsupported or language-neutral locales use `English`.

## Session title generation

After plan approval, cruise sets a concise session title of at most 80 characters for `cruise list` and the WebUI sidebar:

- **SDK mode** (`sdk: jcode`, `sdk: claude`, `sdk: opencode`, or the default jcode backend) makes a separate title-generation request only on the foreground `cruise plan` approval path (interactive **Approve/Execute now** and non-TTY auto-approval), using `plan_model` then `model`. If jcode has no effective model, approval fails and is not saved. Other title-generation failures fall back to `plan.md` metadata: the first heading, or the first content line with list markers stripped.
- **Command mode** derives the title from the first heading or first non-empty line of `plan.md`; it makes no separate title-generation call.
- `cruise list` **Approve**, WebUI/TUI approval, and background `cruise --plan` planning completion use the `plan.md` metadata fallback directly: first heading, else first content line with list markers stripped. Background `cruise --plan … --skip-planning` also derives the title directly from `plan.md` and makes no model call.

No `llm:` workflow field or `CRUISE_LLM_*` override exists. Title generation uses the configured execution backend and model resolution above.
