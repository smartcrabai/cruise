# Changelog

## Unreleased

### Breaking

- Require a fresh `cruise webui` login URL: it carries a single-use five-minute nonce, not the API token; successful login sets a separate in-memory browser session cookie, while bare-origin bookmarks still show a 401 page.
- Replace separate CLI and WebUI execution with a per-user resident daemon. Session commands autostart it; workflow overrides and working directory apply per request, while process-level environment comes from the daemon.
- Remove the terminal keyboard client. Bare `cruise` on a TTY opens `cruise list`; `cruise ssh <host>` without forwarded arguments runs remote `cruise list`.
- Remove `cruise webui start|stop|restart|status`; use `cruise daemon start|stop|restart|status|serve`.
- Refuse daemon startup while the v0.3.0 detached WebUI is live; `cruise daemon stop` can stop both servers and cancels legacy work, leaving its sessions `Suspended`.
- Refuse to run or plan a session while another cruise process recorded as its live runner (for example a v0.3.0 `cruise run` still in progress during an upgrade) is executing it; the daemon answers 409 and the CLI exits 1 without changing session state. v0.3.0 background planning workers record no runner, so let background plans started before the upgrade finish before acting on those sessions.
- Replace the `android` UI E2E policy ID with `jetpack-compose`. Android Views targets without Compose follow the common policy for unlisted targets.

### Changed

- Stop prompting for workflow flags on `cruise run`; saved session values and explicit `-f/--flag` assignments now control execution. `exec` and `force_exec` still prompt before creating a transient session.
- Breaking change: the built-in `browser_e2e` and `app_e2e` flags are removed without aliases and replaced by `e2e`. TDD's former `browser-e2e` / `app-e2e` steps become one `e2e` step; refactor's browser/app E2E steps are consolidated into `prepare-e2e` and `e2e-final-check`. Saved sessions that enabled `browser_e2e` or `app_e2e` now run without E2E; re-enable it with `cruise run <id> -f e2e`.
- Split the README into English, Japanese, and Simplified Chinese documentation under `docs/en/`, `docs/ja/`, and `docs/zh/`.
- Resolve managed provider installs to each provider's latest stable release, skipping healthy matches and reporting recorded installed versions in offline status.
- Isolate GitHub Action config, data, state, and provisioned jcode credentials per workflow job under `$RUNNER_TEMP`, and stop the job's daemon when the action finishes.
- Shorten built-in workflow prompts by about 24% without changing step structure, and stop TDD consolidate/apply and security triage from inlining the previous step output twice.
- Update SwiftUI and Slint UI E2E policies with faster feedback loops that keep target-runtime verification separate.

### Added

- Check GitHub and GitLab base-branch CI before fresh workflow runs by default; opt out with `check_base_ci: false` in `default.yaml`.
- Prompt steps can declare ordered `quality_gates` commands that verify successful work and retry the step with sanitized failure feedback within the existing edge budget.
- Add the `if.head-changed` step condition, which routes to a target when a step completes successfully with a different Git `HEAD` OID; untracked, ignored, and uncommitted changes do not count.
- Add `cruise docs e2e [<framework>]` to read the common E2E policy and available framework IDs or one embedded policy offline. New policies include `browser-extension`, `slint`, `flutter`, and `fyne`.
- Add `cruise daemon start|stop|restart|status|serve` and `CRUISE_DAEMON_PORT` for resident daemon lifecycle and port configuration.
- Start Docker images as a foreground daemon on port 8484 by default, and document long-lived service-container use.
- Rotate detached `daemon.log` files above 5 MiB into a single `daemon.log.1` generation.
- Add locale selection and translations for English, Simplified Chinese, Spanish, French, Brazilian Portuguese, Japanese, and Korean across the CLI and WebUI.
- Add the bounded `plan_revision` Quint/Alloy specification with production Gherkin and negative controls (`scripts/check_plan_revision_specs.sh`) covering planning-input delivery, client action parity, and phase preservation.
- Add best-effort client-side Herdr lifecycle reporting over OSC 7501 for CLI `plan`/`run`/`exec` (including Run/Resume from `cruise list`), with fully configured classic Herdr taking precedence. Only `cruise list` browsing/action-selection menus and WebUI daemon sessions are excluded.

### Fixed

- Recheck CI after no-commit built-in after-PR repairs instead of proceeding to merge, within the existing retry budget. Unavailable local platforms no longer block independent evidence-backed fixes; platform-specific repairs remain incomplete until their actual CI jobs pass.
- Resume fatal after-PR failures from their separate graph checkpoint, preserving retry counts and avoiding repeated main-workflow and PR-publication effects. Legacy sessions without the stage marker retain their previous after-PR restart behavior.
- Prevent stack overflow while reporting early CLI workflow failures, including missing-model diagnostics, when after-PR steps are configured.
- Native GitHub squash merges check current-base ancestry and route confirmed behind/diverged or conflicting pull requests through bounded base-sync recovery. Only native `ResyncRequired` routing starts a fresh after-PR pass budget: reset `budgeted_traversals` for every edge except `squash-merge` → `if.fail` target and clear group retry counts, preserving edge traversal history. The resync edge remains capped at `max_retries`, while generic `if.fail` retains shared budgets. Worst case: up to `max_retries` resyncs, each pass allowing up to `max_retries - 1` CI-fix iterations; zero retry budget permits no resync. Other readiness failures and GitLab behavior are unchanged.
- WebUI offers Fix, Ask, and Verify for Planned sessions instead of a Replan action that silently ignored its feedback. `CruiseApplication::replan` now rejects feedback or questions, and planning requests reject user text their prompt cannot render. `cruise list` Replan is unchanged.

- Retain quality-gate output when sanitizing large streams runs past the post-exit pipe-drain grace.
- Prevent detached Windows WebUI daemons from inheriting captured CLI output handles and blocking command completion.
- Route pending CI without a known failure away from repair, preserve failed rollups at the wait deadline, wait for Actions suites before collecting complete failure details, and stop no-change fix loops.
- Retry transient SDK transport failures and OpenCode setup timeouts before prompt submission through the existing model fallback policy, while retaining barriers for local OpenCode API rate-limit, client, and server-class failures.
- The shared after-PR CI repair step now loops back to base sync only when the repair creates a commit (`if.head-changed`), so untracked or ignored build artifacts from reproducing CI no longer rerun sync/push/wait-ci against the same failed run.
- Render daemon-run output once with the existing step and parallel-child formatting, and leave sessions runnable when base-CI preflight rejects a run.
- Keep daemon terminal waits alive through transient session 404s while the event stream can still deliver terminal events; dispose exec sessions only after releasing terminal events.
- Run approval-title SDK requests from the session workspace.
