# Responsive WebUI executable specifications

## Tell browser regression

From the repository root, install the pinned dependencies and browser, build the production runtime, and run the Tell contract:

```sh
npm ci --prefix tests/webui --ignore-scripts
tests/webui/node_modules/.bin/playwright install chromium
cargo +1.99.1 build --locked --all-features --bin cruise-runtime
node tests/webui/tell-dialog.mjs
node tests/webui/tell-negative.mjs
```

On Linux, Chromium's system libraries must also be installed (the existing CI setup uses `playwright install --with-deps chromium`). The test uses an isolated temporary HOME, XDG data directory and Git repository. It exercises the actual WebUI and a real running workflow, without an external agent or credentials. Screenshots of the planned boundary, rejected blank instruction, editor, accepted submission and Info audit are saved under `target/formal-reports/tell-browser/`. The negative control removes the Tell button from a temporary asset copy and requires the same browser contract to fail on that missing button. The WebUI CI workflow runs both and uploads `target/formal-reports/`.

## Per-event notifications browser regression

Run `cargo +1.99.1 build --locked --all-features --bin cruise-runtime` and `node tests/webui/notifications.mjs` from the repository root after installing the pinned Playwright dependencies and Chromium. The existing responsive WebUI CI job runs this test and uploads screenshots under `target/formal-reports/notification-browser/`. `WEBUI_BDD_BIN` selects a different runtime. Seven real workflows run through the browser in an isolated temporary Git repository, including option dialogs and deterministic no-file-changes failures. A browser EventSource observes production SSE and a local HTTP server receives real webhook POSTs. The test checks completed, action-required, and failed event opt-out and opt-in, plus a disabled master with completed-event opt-in, alongside the corresponding visible toasts. The option scenarios also answer the real dialog and complete their workflows. Negative SSE assertions observe a bounded 500 ms window after the toast. This does not exercise OS desktop notification display, sound, or plan-ready events.

## Workflow category browser regression

From the repository root, install the locked dependencies and browser, build the production runtime, and run the isolated browser test:

```sh
npm ci --prefix tests/webui --ignore-scripts
npx --prefix tests/webui playwright install chromium
cargo +1.99.1 build --locked --all-features --bin cruise-runtime
node tests/webui/workflow-categories.mjs
```

On Linux, use `playwright install --with-deps chromium` if system libraries are missing. Screenshots are written to `target/formal-reports/workflow-categories/` and uploaded by the existing responsive WebUI CI job. The test creates and removes its own HOME, XDG directories, workflow files, and session fixtures. It uses the production server, templates, and HTMX endpoints, without external credentials.

## Local merge browser contract

After installing the pinned dependencies and Chromium as described below, build the runtime with `cargo +1.99.1 build --locked --all-features --bin cruise-runtime`, then run `node tests/webui/local-merge.mjs` from the repository root. Set `WEBUI_BDD_BIN` to use a different prebuilt runtime. This runs the production server and real Chromium against disposable Git repositories and isolated HOME/XDG directories. It covers confirmation cancellation, staging committed and uncommitted work, dirty-base rejection, and squash merge with cleanup, checking browser feedback and Git/session state. It never modifies the checkout running the test.

The test saves stage success, dirty-base error, and merge success screenshots under `target/formal-reports/local-merge-browser/` (override with `WEBUI_LOCAL_MERGE_REPORT_DIR`). The existing WebUI CI workflow runs this contract and uploads the report directory.

## Auto-requeue browser coverage

With Node 26.12.1 and Rust 1.99.1, from the repository root run:

```sh
npm ci --prefix tests/webui --ignore-scripts
tests/webui/node_modules/.bin/playwright install chromium
cargo +1.99.1 build --locked --all-features --bin cruise-runtime
node tests/webui/auto-requeue.mjs all
node tests/webui/auto-requeue.mjs header
```

On Linux install Chromium's system dependencies as described below. The test isolates its HOME and session data and saves screenshots to `target/formal-reports/auto-requeue-browser/`. It checks settings persistence and validation, counter display and a Run All retry. The retry uses a scripted command provider, so it does not verify a real external provider. The existing responsive WebUI CI workflow runs both scenarios and uploads the reports.

## PR retry browser boundaries

With Node 26.12.1, Rust 1.99.1, Git and the GitHub CLI installed, run from the repository root:

```sh
npm ci --prefix tests/webui --ignore-scripts
tests/webui/node_modules/.bin/playwright install chromium
cargo +1.99.1 build --locked --all-features --bin cruise-runtime
node tests/webui/pr-failed-real.mjs
```

On Linux, use `playwright install --with-deps chromium` for system dependencies. The script runs the production server and real Git against disposable local repositories and writes screenshots to `target/formal-reports/pr-failed-browser/` (override with `WEBUI_PR_REPORT_DIR`). It checks the no-commits completion boundary and a real push failure, including persisted phase and no workflow rerun. It does not create an actual forge PR or verify successful publication or existing-PR reuse, which require an appropriate real forge and credentials. CI runs these secret-free boundaries and uploads the screenshots with the other WebUI reports.

## PR form browser regression

Build the production runtime with `cargo +1.99.1 build --locked --all-features --bin cruise-runtime`, install the pinned Playwright browser, then run `npm --prefix tests/webui run test:pr-form`. `WEBUI_BDD_BIN` selects a runtime and `WEBUI_PR_REPORT_DIR` overrides the screenshot directory (default `target/formal-reports/pr-form-browser`). The test starts the real server with an isolated temporary HOME/XDG tree, exercises source switching, browser number validation, and real draft endpoint rejection of zero and u64 overflow. It saves screenshots and cleans up its temporary data. It does not contact a forge or use fake services. A real successful PR-backed session needs a reachable GitHub repository and authentication, so this secret-free test does not claim that coverage.

## Diff browser regression

From the repository root, install the pinned Playwright package and Chromium, build the production runtime, and run the dedicated test:

```sh
npm ci --prefix tests/webui --ignore-scripts
tests/webui/node_modules/.bin/playwright install chromium
cargo +1.99.1 build --locked --all-features --bin cruise-runtime
node tests/webui/diff-browser.mjs
```

On Linux, install Chromium system dependencies as well with `tests/webui/node_modules/.bin/playwright install --with-deps chromium`. The responsive WebUI workflow runs the test and uploads `target/formal-reports/diff-browser/`, containing changed, refresh, empty, unavailable and error screenshots. The test uses a real runtime, real HTMX requests and an isolated temporary Git repository and worktree, not the project repository. It verifies a changed file, refresh after an external edit, an empty diff, an unavailable session branch and an error for a nonexistent Git branch. `WEBUI_BDD_BIN` selects an alternative runtime and `WEBUI_DIFF_REPORT_DIR` overrides the screenshot directory.

## Graph flag browser regression

After installing the pinned browser dependencies, run `cargo +1.99.1 build --locked --all-features --bin cruise-runtime` followed by `node tests/webui/graph-flags.mjs` from the repository root. The existing WebUI CI job runs this test and uploads `target/formal-reports/graph-browser/`. `WEBUI_BDD_BIN` can select another runtime and `WEBUI_GRAPH_REPORT_DIR` changes the screenshot directory. The test uses a temporary HOME/XDG tree and changes persisted flags externally, exercising real Graph tab HTMX requests for off, on, off again, and an all-disabled graph. It asserts the server's Mermaid source and projected edges, not that the externally loaded Mermaid CDN successfully rendered SVG. Screenshots capture the browser state at the swap and may show Mermaid source if the CDN has not loaded.

Run from the repository root:

```sh
bash scripts/check_responsive_webui_bdd.sh
```

Prerequisites: Rust **1.99.1** and build dependencies, Node.js **26.12.1**, npm, and Playwright's Chromium system libraries. The script runs `npm ci --prefix tests/webui --ignore-scripts`, installs the locked browser revision (including Linux system dependencies with `playwright install --with-deps chromium`), then executes `cargo +1.99.1 test --locked --all-features --test webui_responsive_bdd` and `node tests/webui/negative-controls.mjs`. Browser prerequisites are mandatory: missing Node, missing Chromium, crashes, undefined steps, pending/skipped steps, unknown bridge contracts and assertion failures all fail the suite. This explicit integration target is not silently included in ordinary `cargo test` on machines without browsers. The provider-native CI job always runs it explicitly.

The runner follows the existing `tests/*_bdd.rs` Rust Cucumber **0.23.0** convention. Rust invokes a bounded Node Playwright bridge once per scenario. Node owns the production `cruise-runtime webui --port 0 --webui-dir <repo>/webui` child, an isolated HOME/XDG tree and persisted session/plan/log fixture inputs, and closes Chromium and the server after assertions. Cargo builds the real binary. Nothing intercepts HTTP, replaces HTMX, inserts synthetic template markup, or substitutes a fake DOM. The generated CSS endpoint, production TSX renderer and production JavaScript execute together. Local Playwright dependency pins and lockfile are independent of `tests/formal`.

The merge preview uses a strict temporary `gh` executable at the external forge boundary, returning 50 checks to the **real** production PR inspection/parser/HTTP handler/template. It rejects mutation commands and never contacts GitHub. Publish is previewed and canceled, never submitted. The option scenario executes a real map-shaped workflow through the production run endpoint, receives the actual SSE option dialog, submits text to the real option-answer handler and checks the successor shell step's file effect. The option and Run All scenarios create and remove unrelated temporary Git repositories with fixture commits, never commit in this repository or any linked worktree. Run All scrolling uses 21 isolated persisted sessions executing harmless shell commands, each emitting 40 real log lines. All 21 workflows must complete, their real log must overflow and follow actual SSE updates, and their results must remain reachable. No result or log markup is mocked.

`WEBUI_BDD_BIN` selects a prebuilt runtime for direct bridge debugging, for example `node tests/webui/browser.mjs layout 360 640`. `WEBUI_BDD_ASSETS` selects an asset directory for negative controls. Normal Cargo execution always selects its own freshly built runtime. The bridge has a 90-second watchdog and a 20-second server startup deadline.

## Requirement mapping

- R1: toggle via pointer and keyboard, backdrop hit testing, Escape focus restoration, closed-drawer Tab exclusion, real session/New/Run All navigation, cancelled native confirmation with zero POSTs, actual sidebar poll, settings overlay.
- R2: keyboard + pointer desktop resizing, localStorage and reload persistence, invalid stored width defaulting, min/max clamping, mobile width overriding saved width, exact 767/768 boundary reset in both directions. The desktop column remains visible.
- R3/R4: viewport-height app, real New Session controls, long session title/input/path, document/main bounds at 360, 375 and 767 pixels.
- R5: overflowing session header scrolls main while tabs remain reachable, real Plan/Log swaps, saved log internal scrolling with 70dvh cap. A separate live Run All scenario keeps all 21 results reachable in main on mobile and checks desktop internal scrolling.
- R6: all four production dialog variants. Settings and Publish previews fit the viewport. A 50-check Merge preview and an 18-choice live Option dialog overflow internally while keeping buttons/text entry reachable, including at 320px viewport height.
- R7: server-produced error toast after invalid settings submission fits viewport.
- R8: computed New Session, session-settings, editor, Settings modal and live Option text-control sizes are at least 16px on mobile. This checks the CSS prerequisite, not physical iOS auto-zoom behavior.
- R9: persisted Markdown containing long words, a wide table and code reaches the real Markdown renderer. Document does not overflow and the table actually scrolls horizontally.

The browser package is pinned to Playwright **1.62.1** and its lockfile records integrity hashes. That release selects Chromium **151.0.7922.34**, revision **1234**. The wrapper logs the actual launched browser version. The Rust bridge removes Cargo-only dynamic-library search variables before starting Node because leaking them into Chromium helpers causes renderer crashes on macOS.

Negative controls first select `undefined-control.feature` through the test-only `WEBUI_BDD_FEATURE` override and require the real Rust runner to fail its undefined/skipped step. That deliberately invalid fixture is outside the normal feature path. They then copy production assets to a temporary directory and run the same browser contracts against the real server. Removing Escape focus restoration must fail its focus assertion. Forcing mobile text controls to 14px must fail the computed-font assertion. A crash or unrelated failure does not count as detecting a mutant.

The browser uses reduced motion for deterministic observation, not arbitrary transition sleeps. The drawer scenario explicitly asserts that reduced-motion styling wins even while open and that the mobile header responds to dark mode. Assertions use computed layout, hit testing, native dialogs, network requests, localStorage, keyboard focus and real HTMX swaps rather than checking CSS source strings. The suite does not claim physical iOS behavior or exhaustive browser coverage.
