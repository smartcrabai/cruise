# Changelog

## Unreleased

### Added

- Prompt steps can declare ordered `quality_gates` commands that verify successful work and retry the step with sanitized failure feedback within the existing edge budget.
- Add locale selection and translations for English, Simplified Chinese, Spanish, French, Brazilian Portuguese, Japanese, and Korean across the CLI, TUI, and WebUI.

### Fixed

- Native GitHub squash merges now check the pull request against the current base immediately before merging. Confirmed behind/diverged or conflicting pull requests enter the bounded built-in base-sync recovery loop; other failures and GitLab behavior are unchanged.

- Retain quality-gate output when sanitizing large streams runs past the post-exit pipe-drain grace.
- Prevent detached Windows WebUI daemons from inheriting captured CLI output handles and blocking command completion.
- Route pending CI without a known failure away from repair, preserve failed rollups at the wait deadline, wait for Actions suites before collecting complete failure details, and stop no-change fix loops.
- Retry transient SDK transport failures and OpenCode setup timeouts before prompt submission through the existing model fallback policy, while retaining barriers for local OpenCode API rate-limit, client, and server-class failures.
