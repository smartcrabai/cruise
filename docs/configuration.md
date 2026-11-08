# Application configuration

Cruise stores application settings separately from workflow YAML in `$XDG_CONFIG_HOME/cruise/config.json` or `config.yaml`. If both exist, JSON wins. See the [README configuration reference](../README.md#cruise-config) for file selection and other settings.

## UI language

Set `uiLanguage` in application configuration, or use `cruise config --set-language ja`. The alias `ui_language` is accepted on input. Supported explicit tags are `en`, `zh-CN`, `es`, `fr`, `pt-BR`, `ja`, and `ko`, with case and `_`/`-` variations accepted. `auto` means unspecified; `cruise config --set-language auto` clears the saved setting. Invalid values are rejected when setting or validating application configuration.

For CLI and TUI, `CRUISE_UI_LANGUAGE` takes precedence over `uiLanguage`, then system locale, then English. A blank or `auto` environment value defers to the next layer. Invalid environment values produce an English warning and are ignored. System locale detection uses the first nonempty `LC_ALL`, `LC_MESSAGES`, `LANG`, or `LANGUAGE`, without trying later variables when that value is unsupported. Windows also consults the user locale if environment variables do not determine one. macOS and Linux use environment variables only, so a GUI launch without them defaults to English.

The WebUI uses its browser's `cruise_ui_language` cookie when valid, otherwise the server process default. Selecting a language in the WebUI saves the cookie and reloads the page, without changing application configuration. UI language is independent of workflow `languages.pr` and `languages.plan`, which control generated prompts.
