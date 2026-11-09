# Changelog

## Unreleased

### Added

- Prompt steps can declare ordered `quality_gates` commands that verify successful work and retry the step with sanitized failure feedback within the existing edge budget.
- Add locale selection and translations for English, Simplified Chinese, Spanish, French, Brazilian Portuguese, Japanese, and Korean across the CLI, TUI, and WebUI.

### Fixed

- Retain quality-gate output when sanitizing large streams runs past the post-exit pipe-drain grace.
- Prevent detached Windows WebUI daemons from inheriting captured CLI output handles and blocking command completion.
- Retry transient SDK transport failures and OpenCode setup timeouts before prompt submission through the existing model fallback policy.
