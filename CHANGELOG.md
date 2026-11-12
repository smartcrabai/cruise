# Changelog

## Unreleased

### Changed

- Stop prompting for workflow flags on `cruise run`; saved session values and explicit `-f/--flag` assignments now control execution. `exec` and `force_exec` still prompt before creating a transient session.

### Added

- Prompt steps can declare ordered `quality_gates` commands that verify successful work and retry the step with sanitized failure feedback within the existing edge budget.
- Add locale selection and translations for English, Simplified Chinese, Spanish, French, Brazilian Portuguese, Japanese, and Korean across the CLI, TUI, and WebUI.
- Add the bounded `plan_revision` Quint/Alloy specification with production Gherkin and negative controls (`scripts/check_plan_revision_specs.sh`) covering planning-input delivery, client action parity, and phase preservation.

### Fixed

- Resume fatal after-PR failures from their separate graph checkpoint, preserving retry counts and avoiding repeated main-workflow and PR-publication effects. Legacy sessions without the stage marker retain their previous after-PR restart behavior.
- Prevent stack overflow while reporting early CLI workflow failures, including missing-model diagnostics, when after-PR steps are configured.
- Native GitHub squash merges check current-base ancestry and route confirmed behind/diverged or conflicting pull requests through bounded base-sync recovery. Only native `ResyncRequired` routing starts a fresh after-PR pass budget: reset `budgeted_traversals` for every edge except `squash-merge` → `if.fail` target and clear group retry counts, preserving edge traversal history. The resync edge remains capped at `max_retries`, while generic `if.fail` retains shared budgets. Worst case: up to `max_retries` resyncs, each pass allowing up to `max_retries - 1` CI-fix iterations; zero retry budget permits no resync. Other readiness failures and GitLab behavior are unchanged.
- WebUI and TUI offer Fix, Ask, and Verify for Planned sessions instead of a Replan action that silently ignored its feedback. `CruiseApplication::replan` now rejects feedback or questions, and planning requests reject user text their prompt cannot render. `cruise list` Replan is unchanged.

- Retain quality-gate output when sanitizing large streams runs past the post-exit pipe-drain grace.
- Prevent detached Windows WebUI daemons from inheriting captured CLI output handles and blocking command completion.
- Route pending CI without a known failure away from repair, preserve failed rollups at the wait deadline, wait for Actions suites before collecting complete failure details, and stop no-change fix loops.
- Retry transient SDK transport failures and OpenCode setup timeouts before prompt submission through the existing model fallback policy, while retaining barriers for local OpenCode API rate-limit, client, and server-class failures.
