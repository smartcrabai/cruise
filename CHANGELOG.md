# Changelog

## Unreleased

### Breaking

- Replace the `cruise manager` slash-command prompt with one-shot subcommands. Run `cruise manager --help` for the command list or `cruise manager status --json` for scriptable status. Non-interactive approval requires both `--yes` and a matching `--revision N`.

- Remove the `claude` and `opencode` SDK backends; configurations must use `sdk: jcode` or external `command:` mode. Workflow MCP servers now support only `stdio`; HTTP/SSE entries are no longer supported.

- Require a fresh `cruise webui` login URL: it carries a single-use five-minute nonce, not the API token; successful login sets a separate in-memory browser session cookie, while bare-origin bookmarks still show a 401 page.
- Replace separate CLI and WebUI execution with a per-user resident daemon. Session commands autostart it; workflow overrides and working directory apply per request, while process-level environment comes from the daemon.
- Remove the terminal keyboard client. Bare `cruise` on a TTY opens `cruise list`; `cruise ssh <host>` without forwarded arguments runs remote `cruise list`.
- Remove `cruise webui start|stop|restart|status`; use `cruise daemon start|stop|restart|status|serve`.
- Refuse daemon startup while the v0.3.0 detached WebUI is live; `cruise daemon stop` can stop both servers and cancels legacy work, leaving its sessions `Suspended`.
- Refuse to run or plan a session while another cruise process recorded as its live runner (for example a v0.3.0 `cruise run` still in progress during an upgrade) is executing it; the daemon answers 409 and the CLI exits 1 without changing session state. v0.3.0 background planning workers record no runner, so let background plans started before the upgrade finish before acting on those sessions.
- Replace the `android` UI E2E policy ID with `jetpack-compose`. Android Views targets without Compose follow the common policy for unlisted targets.
- Remove after-PR automation and its `after_pr` and `merge` flags from the built-in `refactor`, `add-tests`, `add-formal-specs`, and `security` workflows. `exec` still does not create PRs; `--no-force-exec` still allows regular worktree PR creation without after-PR automation.

### Changed

- Stop prompting for workflow flags on `cruise run`; saved session values and explicit `-f/--flag` assignments now control execution. `exec` and `force_exec` still prompt before creating a transient session.
- Relax Bubblewrap isolation requests: `require_bubblewrap: true` and `CRUISE_JCODE_BUBBLEWRAP=1` use isolation when available; otherwise Cruise warns and runs unconfined. CI repair commits use the host verifier when Bubblewrap and `CRUISE_CI_VERIFY_SCRIPT` are available; otherwise Cruise warns, commits and pushes without host verification, and relies on CI for the pushed head. A configured verifier failure still blocks the commit.
- Breaking change: the built-in `browser_e2e` and `app_e2e` flags are removed without aliases and replaced by `e2e`. TDD's former `browser-e2e` / `app-e2e` steps become one `e2e` step; refactor's browser/app E2E steps are consolidated into `prepare-e2e` and `e2e-final-check`. Saved sessions that enabled `browser_e2e` or `app_e2e` now run without E2E; re-enable it with `cruise run <id> -f e2e`.
- Split the README into English, Japanese, and Simplified Chinese documentation under `docs/en/`, `docs/ja/`, and `docs/zh/`.
- Resolve managed provider installs to each provider's latest stable release, skipping healthy matches and reporting recorded installed versions in offline status.
- Isolate GitHub Action config, data, state, and provisioned jcode credentials per workflow job under `$RUNNER_TEMP`, and stop the job's daemon when the action finishes.
- Shorten built-in workflow prompts by about 24% without changing step structure, and stop TDD consolidate/apply and security triage from inlining the previous step output twice.
- Update SwiftUI and Slint UI E2E policies with faster feedback loops that keep target-runtime verification separate.
- Move the built-in TDD ledger snapshot and post-PR Git operations to the host. Keep Git metadata read-only in isolated jcode turns; stop on an unexpected command status rather than advancing to publication.
- Leave OpenTelemetry span status unset and omit `error.type` for `paused` and `cancelled` operations; `cruise.outcome` and operation metrics still carry these outcomes. Failures and timeouts remain errors.

### Added

- Add an experimental Manager page to the WebUI for repository conversations, explicit proposal and corrective approvals, live goal and task controls, and replies to child prompts. Manager-owned child sessions stay read-only. **Open in Manager** appears only when Cruise finds a readable Manager record containing the child’s Goal.
- Add an experimental `cruise manager` CLI with repository-scoped durable conversation and SDK-generated proposal previews. Explicit `cruise manager approve` registers a durable active Goal and authorizes the daemon coordinator to schedule eligible child sessions asynchronously on pinned Goal bases. Child results integrate serially; the combined exact SHA is verified before Cruise publishes or recovers the Goal's single PR/MR, which a human merges. Manager never merges. Goal pause, resume, and prompt-answer controls are available. `cruise manager retry` can reserve a new attempt for an eligible previously failed or suspended child of an active Goal, or a completed child of a blocked Goal with a safely established integration conflict, subject to safety checks. For an eligible definite failed check, `cruise manager corrective GOAL TASK` saves a human-authored corrective preview without authorizing it, and `cruise manager approve-corrective GOAL` requires explicit approval of that preview and integrated tip, preserving prior verification evidence and approved checks.
- Add a WebUI **Exec** button for Directory sessions. With `force_exec`, **Execute** replaces **Create & plan** for Directory workflows. Exec sessions appear in the sidebar while they exist and can be resumed there when Suspended.
- Retain failed `cruise exec` sessions, including `--instant` and `force_exec`, under the same ID until success or explicit Delete. Use `cruise run <id>` or Resume in `cruise list` to retry from the first unresolved failed step or, for a hard error with no earlier unresolved failure, the saved checkpoint. If setup failed before workflow execution started and no checkpoint exists, recovery starts at the beginning. A missing or unusable checkpoint blocks recovery for a workflow that already started. Steps at and after the retry point may repeat. Files and external effects are not rolled back. `cruise clean`, automatic run selection, `run --all`, and WebUI inventory exclude failed exec sessions. `cruise list --json` adds `"exec": true` only for failed exec rows.
- Check GitHub and GitLab base-branch CI before fresh workflow runs by default; opt out with `check_base_ci: false` in `default.yaml`.
- Prompt steps can declare ordered `quality_gates` commands that verify successful work and retry the step with sanitized failure feedback within the existing edge budget.
- Add the `if.head-changed` step condition, which routes to a target when a step completes successfully with a different Git `HEAD` OID; untracked, ignored, and uncommitted changes do not count.
- Allow an optional trusted `CRUISE_CI_VERIFY_SCRIPT` outside the worktree to verify the exact staged repair before an automatic CI fix commit and scoped push when Bubblewrap is available; otherwise Cruise warns and relies on CI for the pushed head.
- Add `cruise docs e2e [<framework>]` to read the common E2E policy and available framework IDs or one embedded policy offline. New policies include `browser-extension`, `slint`, `flutter`, and `fyne`.
- Add `cruise daemon start|stop|restart|status|serve` and `CRUISE_DAEMON_PORT` for resident daemon lifecycle and port configuration.
- Start Docker images as a foreground daemon on port 8484 by default, and document long-lived service-container use.
- Rotate detached `daemon.log` files above 5 MiB into a single `daemon.log.1` generation.
- Add locale selection and translations for English, Simplified Chinese, Spanish, French, Brazilian Portuguese, Japanese, and Korean across the CLI and WebUI.
- Add the bounded `plan_revision` Quint/Alloy specification with production Gherkin and negative controls (`scripts/check_plan_revision_specs.sh`) covering planning-input delivery, client action parity, and phase preservation.
- Add best-effort client-side Herdr lifecycle reporting over OSC 7501 for CLI `plan`/`run`/`exec` (including Run/Resume from `cruise list`), with fully configured classic Herdr taking precedence. Only `cruise list` browsing/action-selection menus and WebUI daemon sessions are excluded.
- Export the session base (managed clone with `--repo`) and resolved workspace as raw `cruise.execution`-only trace attributes; telemetry remains opt-in.

### Fixed

- Stop reporting a just-submitted Manager conversation turn as `interrupted` when a concurrent status request reads the record before the turn is published as running.
- Clean up session worktrees from their shared Git directory when the recorded base directory has been removed, and allow session deletion when both paths are gone.
- Refresh jcode `openai` and `claude` OAuth logins in the user's jcode home before launching a private runtime once less than two hours remain, serialized across Cruise processes, and copy back any refresh a private runtime still performed. jcode replaced the shared login link with a private copy on refresh, so the rotated refresh token was lost and later attempts failed with `refresh_token_reused`. An expired login that cannot be refreshed now fails before launch with a `jcode login` hint.
- Allow host Git operations such as after-PR base sync when global or system configuration has exactly the `filter.lfs.*` settings `git lfs install` writes. Other filters, merge drivers, and any filter from repository configuration are still rejected.
- Run built-in `cruise internal` steps, such as after-PR base sync and the TDD ledger gate, with the running Cruise executable through the new `{cruise.bin}` variable instead of the `cruise` found on `PATH`. A daemon running a newer build than the `cruise` on its `PATH` previously failed after-PR base sync with `could not rule out external Git filters or merge drivers`.
- Apply the same to built-in session snapshots saved before `{cruise.bin}` existed: their `cruise internal ...` commands now run with the running executable when the session is loaded, so resuming such a session no longer runs a different `cruise` from `PATH`. File, repository, and inline workflows are not rewritten.
- Recheck CI on the current head when a session stopped by a CI repair `wait` decision is run again. The resumed `finish-ci-fix` step previously replayed the saved `wait` and failed with `CI remains unresolved; refusing to continue to native merge` even after CI had passed.
- Collect GitHub Actions job logs for CI repair evidence as the plain-text download GitHub serves, passing `--allow-escape-sequences` to current `gh` releases. The host previously parsed the response as a ZIP archive, so every failed check reported an incomplete log and CI repair could neither commit nor retry.
- Keep the end of each CI job log in repair evidence, splitting the evidence budget evenly across failed jobs, and stop treating a shortened log as incomplete evidence. Large logs, such as Windows test runs, previously blocked every automatic CI repair commit and retry; only a job log the host cannot fetch still does.
- Keep a reported cancelled Actions or GitLab job, such as a matrix job stopped by fail-fast or a job-level timeout, in CI repair evidence instead of marking the evidence incomplete. Retries still select only failed jobs.
- Accept CI repair evidence when the `wait-ci` diagnostic lists only its first ten failing checks, provided every failing check run on the head is an Actions job the host enumerated and no commit status failed. More than ten failing checks previously always blocked automatic CI repair.
- Retry the built-in `wait-ci` GitHub and GitLab CI status query up to five times, 15 seconds apart and within the wait deadline, after a network error, HTTP 408, 429, or 5xx. One transient failure previously ended the after-PR workflow with `api_error`; other HTTP errors still fail immediately.
- Fix the built-in GitHub `wait-ci` details GraphQL query, which had an unbalanced brace and failed on every CI failure, so failed check names reach the CI diagnostic, span attributes, and CI repair evidence again instead of an empty truncated list.
- Stop reporting a step failure as unresolved when the next `skip: prev.success` recovery step runs because of it, so recovered after-PR workflows (base-sync conflict or CI repair) no longer end as `failed` with `error.type=step_failed`. The failed step span still records the failure.
- Stop stale built-in CI repairs before publication and block a repair when a configured host verifier fails. When no host verifier can run, publication proceeds with a warning and relies on CI for the pushed head.
- Resume fatal after-PR failures from their separate graph checkpoint, preserving retry counts and avoiding repeated main-workflow and PR-publication effects. Legacy sessions without the stage marker retain their previous after-PR restart behavior.
- Prevent stack overflow while reporting early CLI workflow failures, including missing-model diagnostics, when after-PR steps are configured.
- Native GitHub squash merges check current-base ancestry and route confirmed behind/diverged or conflicting pull requests through bounded base-sync recovery. Only native `ResyncRequired` routing starts a fresh after-PR pass budget: reset `budgeted_traversals` for every edge except `squash-merge` → `if.fail` target and clear group retry counts, preserving edge traversal history. The resync edge remains capped at `max_retries`, while generic `if.fail` retains shared budgets. Worst case: up to `max_retries` resyncs, each pass allowing up to `max_retries - 1` CI-fix iterations; zero retry budget permits no resync. Other readiness failures and GitLab behavior are unchanged.
- WebUI offers Fix, Ask, and Verify for Planned sessions instead of a Replan action that silently ignored its feedback. `CruiseApplication::replan` now rejects feedback or questions, and planning requests reject user text their prompt cannot render. `cruise list` Replan is unchanged.

- Retain quality-gate output when sanitizing large streams runs past the post-exit pipe-drain grace.
- Speed up long-line output handling by gating redaction scans on candidate bytes and reusing sanitized output for feedback and artifacts.
- Prevent detached Windows WebUI daemons from inheriting captured CLI output handles and blocking command completion.
- Route pending CI without a known failure away from repair, preserve failed rollups at the wait deadline, wait for Actions suites before collecting complete failure details, and stop no-change fix loops.
- Retry transient SDK transport failures and OpenCode setup timeouts before prompt submission through the existing model fallback policy, while retaining barriers for local OpenCode API rate-limit, client, and server-class failures.
- The shared after-PR CI repair step now loops back to base sync only when the repair creates a commit (`if.head-changed`), so untracked or ignored build artifacts from reproducing CI no longer rerun sync/push/wait-ci against the same failed run.
- Render daemon-run output once with the existing step and parallel-child formatting, and leave sessions runnable when base-CI preflight rejects a run.
- Keep daemon terminal waits alive through transient session 404s while the event stream can still deliver terminal events; dispose exec sessions only after releasing terminal events.
- Run approval-title SDK requests from the session workspace.
- Keep the final stderr lines in OpenCode CLI startup failure messages when the CLI exits before announcing its server.
- Ctrl+C in `cruise run` (including `--all`), `exec`, `plan`, and **Create PR** in `cruise list` is no longer ignored while the CLI handles daemon events. Outside herdr Core panes, a Ctrl+C right after the work starts now cancels it instead of closing only the CLI.
