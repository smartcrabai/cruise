# Responsive WebUI executable specifications

## Verify browser tests

The secret-free boundary suite runs `node tests/webui/verify-boundary.mjs` after the runtime is built and Chromium is installed. It uses real production rendering with isolated HOME/XDG and persisted sessions. It checks that Verify is offered for an approval session with a plan, and hidden for a missing plan, Draft, and Planned, plus the mobile approval view. With a restricted system-only executable PATH and no credentials it clicks Verify, asserts the production POST and the missing Claude CLI error in the UI, and checks the saved plan is preserved. Screenshots, including the error, go under `target/formal-reports/verify-boundary-browser/` (override with `WEBUI_VERIFY_BOUNDARY_REPORT_DIR`). Responsive WebUI CI runs it and uploads the screenshots. No provider is mocked or executed and no specification is verified by this boundary test.

The separate paid real-provider test can be run locally:

From the repository root, install the pinned Node and Rust versions and browser prerequisites above, install Claude Code 2.1.250, then run:

```sh
npm ci --prefix tests/webui --ignore-scripts
npm ci --prefix tests/formal --ignore-scripts
npx --prefix tests/webui playwright install chromium
cargo +1.99.1 build --locked --all-features --bin cruise-runtime
ANTHROPIC_API_KEY=... node tests/webui/verify-real.mjs
```

This invokes the real Claude provider and can incur charges. It creates isolated HOME/XDG and session data, runs the production runtime, and uses real Quint from `tests/formal`; Alloy and Java must also be installed and configured if Alloy checking is desired. It asserts a Draft boundary, the Verify POST, a real formal report and checker stage, and preservation of the saved plan. Screenshots go to `target/formal-reports/verify-browser/` or `WEBUI_VERIFY_REPORT_DIR`. `WEBUI_BDD_BIN` selects a runtime and `WEBUI_VERIFY_MODEL` selects a Claude model. The stochastic provider may generate invalid specs or fail, and the test does not claim a passing formal verdict or complete correctness of the generated model. Screenshots may contain provider output and should be handled accordingly.

The paid test is not run in CI. The separate pre-existing `verify.mjs` uses a fake provider and is not evidence of real-provider success.

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
