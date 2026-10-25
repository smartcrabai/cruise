# Responsive WebUI verification, October 25, 2026

## Environment and pins

Local execution: macOS arm64, Node 22.22.0, Rust 1.99.1 (`58cf8f9c3`, 2026-10-13), Quint 0.33.0, Apalache 0.62.1, Alloy 6.2.0 with SAT4J, Temurin 25.0.5+8-LTS. The repository's shared checksum-pinned formal installer was executed, not substituted with the host's Java 27 or Node 26. All changes remain uncommitted. No HEAD, branch ref, commit guard or other linked worktree's source was modified by this task.

## Commands and model results

Exact reproducible aggregate command: `bash scripts/check_responsive_webui_specs.sh`. Expanded model and regression commands are documented in `RESPONSIVE_WEBUI.md`; each invocation also logs exact process argv in its fresh report directory. The actual installed Quint subcommand help and Alloy `help exec` were inspected before selecting options. Alloy 6.2.0 has no `--expect` CLI switch, so its actual supported `expect 0` / `expect 1` model command syntax is used and every receipt is independently validated.

- `quint typecheck tests/formal/responsive_webui.qnt`: passed.
- `quint test tests/formal/responsive_webui.qnt --backend typescript --seed 20261025`: **9 passed**, zero failed.
- `quint run --seed 20261025 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety tests/formal/responsive_webui.qnt`: passed, no safety violation. **Simulation is not proof**.
- `quint verify --invariant safety --max-steps 8 --main responsive_webui --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint 127.0.0.1:PORT ... tests/formal/responsive_webui.qnt`: passed, Apalache outcome `NoError`. This is bounded verification through **8 transitions**, not unbounded proof. The helper obtains a fresh port and prints actual argv.
- `java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output REPORT/alloy tests/formal/responsive_webui.als`: all commands executed with **9 ordered states / 8 transitions**.

Inspected Alloy receipt results, including the strengthened consecutive-event witnesses:

| Command | Kind | Receipt result | Expected |
| --- | --- | --- | --- |
| Accessibility | check | UNSAT | 0 counterexamples |
| DesktopAndBounds | check | UNSAT | 0 counterexamples |
| SelectiveClose | check | UNSAT | 0 counterexamples |
| Persistence | check | UNSAT | 0 counterexamples |
| CancelThenNavigate | run | SAT, concrete instance | 1 result |
| EscapeRestoresFocus | run | SAT, concrete instance | 1 result |
| ResizeSurvivesCrossing | run | SAT, concrete instance | 1 result |
| PollAndSettingsPreserve | run | SAT, concrete instance | 1 result |

`node scripts/formal/check_alloy_receipt.cjs MODEL RECEIPT` validated **4 UNSAT assertions and 4 SAT witnesses**, rejecting missing, extra or unexpected results. `node --test scripts/formal/check_alloy_receipt.test.cjs` also passed. Alloy emits a Java restricted-native-access warning during solver enumeration but executes SAT4J successfully on the pinned runtime.

## Production regressions and discovered issues

`cargo +1.99.1 test --locked --all-features --bin cruise-runtime webui::` passed **89 tests**. `cargo +1.99.1 clippy --locked --all-targets --all-features` passed with existing `clippy::large_futures` warnings, not suppressed by this change. `cargo +1.99.1 fmt --check`, `git diff --check`, ShellCheck of the aggregate script, Actionlint of the new workflow, and Taplo manifest checks passed during implementation.

The plan's literal `cargo test webui::` command first ran all 89 WebUI tests successfully but subsequently failed because the repository's unrelated harness-free `checkpoint_bdd` does not accept Rust's positional test filter. The reproducible runner fixes that invocation defect by explicitly selecting `--bin cruise-runtime`, preserving all WebUI tests rather than hiding the failing command or relaxing an assertion.

## CI wiring

Added `.github/workflows/responsive-webui-specs.yml`, a dedicated GitHub Actions job for PRs targeting main and pushes to main. It calls `scripts/check_responsive_webui_specs.sh`, installs exact Node/Rust versions and runs formal checks, Alloy receipt validation, real browser Gherkin, affected Rust regressions, Clippy and formatting. There are no secret-dependent PR steps or path exclusions. Reports are uploaded with `if: always()` and retained for 14 days. Local checks do not claim a remotely executed GitHub Actions run.

## Production browser results

`bash scripts/check_responsive_webui_bdd.sh` executes Rust Cucumber **0.23.0** through the actual compiled `cruise-runtime` HTTP server, generated CSS, production TSX, HTMX and `app.js`. Playwright **1.62.1** launches Chromium **151.0.7922.34**, revision **1234**, and the wrapper captures the runtime version. The locked npm install audited successfully with no reported vulnerabilities. No HTTP interception, synthetic template markup, dry run or fake DOM is used.

The full suite passes **9 scenarios / 27 steps**:

1. Pointer/keyboard drawer toggling, backdrop hit testing, closed-drawer Tab exclusion and Escape focus restoration from a real sidebar control. Reduced-motion precedence and mobile dark-mode header styling are asserted.
2. Actual sidebar polling preserves open state. Canceling the native Run All confirmation sends no POST and preserves state. Settings preserves state. Actual session/New/accepted Run All navigation closes it.
3. Desktop keyboard/pointer resizing, localStorage/reload persistence, min/max clamp and invalid-value restoration, mobile width isolation, exact 767/768 breakpoint reset and always-visible desktop column.
4-6. Real New Session, session-settings and editor controls, long title/input/path, outer session scrolling and reachable tabs, saved Log local scrolling/cap, actual Plan/Log swaps, overflowing Markdown table with actual local horizontal scrolling, Settings/Publish previews and a server-produced toast. Viewports: **360×640**, **375×667**, **767×540**.
7. Real Merge preview handler/parser/template with **50** fixture forge checks, internally scrollable at **360×320**, reachable confirmation and canceled preview. A strict temporary `gh` executable supplies the external forge boundary and rejects mutation commands. No real forge request or merge occurs. Settings/Publish also fit this short viewport.
8. At **375×320**, a real production option workflow emits an SSE dialog with **18** choices and text input. The dialog fits, scrolls internally, has computed **16px** text-entry font, accepts an actual option-answer POST, disappears, completes its workflow and writes the successor shell step's file effect. Its unrelated temporary Git repository and fixture commit are removed afterwards. No commit or ref operation touches this project or any linked worktree.
9. A live Run All batch of **21** isolated sessions executes harmless shell commands, each emitting **40** real log lines, and all 21 workflows must complete. Results remain reachable through outer mobile scrolling, while desktop retains internal results scrolling. The live log is genuinely overflowing and follows actual SSE swaps at the bottom. Execution uses an unrelated temporary Git repository, removed afterwards.

Three negative controls pass by detecting the required failure, not just any nonzero exit: undefined Cucumber step fails through `fail_on_skipped`, deleting Escape focus restoration fails its focus assertion, and forcing mobile controls to 14px fails its computed-font assertion. Missing browser/runtime prerequisites are errors, never skips or passes.

## Defects fixed and scope of edits

No additional production implementation defect was found, and this task did not alter the existing responsive production edits. Added model, runner, manifest, Gherkin/browser and CI files remain uncommitted.

- Fixed the plan's regression command integration with the unrelated harness-free Cucumber test by selecting the production binary explicitly, as described above.
- Fixed an initially non-observing Escape test: focus must first move into the drawer, otherwise removing focus restoration could pass while the toggle was already focused. The final negative control verifies this assertion detects the regression.
- Corrected the initial browser fixture's workflow YAML from array-shaped steps to the repository's real map-shaped schema. The option fixture verifies real execution and completion rather than merely successful page rendering.
- The first live-log assertion correctly rejected a fixture that produced preparation errors but no log chunks. Replaced that fixture with 21 real successful shell workflows producing 840 log lines. Kept the nonempty-overflow and sticky-scroll assertions unchanged, rather than declaring an empty log a pass.
- Removed Cargo-only dynamic-library search variables from the Node bridge environment after they caused Chromium helper crashes on macOS. This is test-process isolation, not a substitute browser or skipped platform.
- Strengthened Alloy witnesses to require consecutive cancellation/navigation and poll/settings events. No safety invariant or assertion was weakened to obtain a pass.
- Disabled Playwright cache garbage collection in the install wrapper so future runs do not remove another project's cached browser revision.

## Prerequisites and limits

All required tools and native browser prerequisites were available/installed and actually executed locally. There is **no outstanding local runner or platform blocker**. Linux CI installation is configured on **ubuntu-24.04**, including browser system libraries, but a remote GitHub Actions run is not claimed. Browser checks use Chromium, not a physical iOS device. The 16px requirement is checked as computed CSS, not as a claim of exhaustively tested Safari zoom behavior. The symbolic width abstraction and verification bounds are documented in `RESPONSIVE_WEBUI.md`. These checks are not an unbounded proof or a formal implementation-refinement proof.

## Final execution evidence

The final complete aggregate run on **October 25, 2026, 11:02:08–11:02:45 UTC** exited **0** after the live-log fixture correction. Every entry in `target/formal-reports/responsive-webui.9hJs9v/results.log` is zero, including setup, actual CLI help, all model checks, receipt validation and validator tests, browser BDD, production regressions, Clippy and formatting.

Local evidence, relative to the repository root:

- Aggregate command argv and exit statuses: `target/formal-reports/responsive-webui.9hJs9v/commands.log` and `results.log`.
- Quint tests, seeded simulation and bounded solver output: `quint-test.log`, `quint-simulation.log` and `quint-verify.log` in that directory. The latter ends with Apalache `NoError` and `[ok] No violation found` at the stated 8-transition bound.
- Actual Alloy receipt: `target/formal-reports/responsive-webui.9hJs9v/alloy/receipt.json`, inspected alongside all four witness solution JSON files. `alloy-receipt.log` reports each of the four UNSAT assertions and four SAT witnesses and confirms all expectations.
- Real browser scenarios and negative controls: `target/formal-reports/responsive-webui.9hJs9v/responsive-bdd.log`. Expanded browser command/version logs and negative-control diagnostics are in `target/formal-reports/responsive-browser.IYqLcl/`.

Reports are generated artifacts, not committed source. CI uploads its own fresh report directories. Apalache also emits a bundled protobuf-generated-code warning on the pinned JDK. It does not prevent verification, is not suppressed, and is retained in `quint-verify.log`. No claim is made that this tooling warning has been fixed by the specification work.
