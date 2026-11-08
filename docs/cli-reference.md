# CLI reference

See the [README command reference](../README.md#usage) for the complete command and option overview.

## `cruise config`

`cruise config` displays application settings, including `ui_language` and its effective source (`env`, `config`, `system`, or `default`). Use `cruise config --set-language <LANG>` to save a UI language, for example:

```sh
cruise config --set-language ja
cruise config --set-language auto
```

The second command clears the saved language so the next available preference applies. Supported explicit tags are `en`, `zh-CN`, `es`, `fr`, `pt-BR`, `ja`, and `ko`. `--set-language` rejects unsupported values. The TUI also offers a language picker on `L`. A WebUI selection is instead saved in a browser cookie.

See [application configuration](configuration.md#ui-language) for locale precedence and the distinction between UI and prompt languages.
