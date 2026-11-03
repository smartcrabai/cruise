# Responsive WebUI executable contract

## Authority and abstraction

Authority is the responsive WebUI plan at session `20261025083759183_537f9ed8d0a04980aa6a95b3fc8a07d6/plan.md`, especially §§0, 2-6 and 7. No behavior is inferred from a wished-for implementation. Production paths are `webui/static/app.js` (drawer events, width persistence and scrolling), `webui/templates/layout.tsx`, `sidebar.tsx`, `components/run-all-control.tsx`, `components/session-row.tsx`, affected page/dialog/form templates, `webui/static/app.css`, `src/webui/css.rs` (actual generated CSS) and production WebUI routes.

The two models describe **observable shell state**: mobile/desktop viewport class, drawer open state, accessibility expansion, backdrop availability, remembered desktop width and Escape focus restoration. The open bit describes the mobile drawer, not whether the desktop sidebar column exists: desktop always displays the column while keeping the drawer bit reset. Transitions represent user toggle, backdrop, Escape, a confirmed sidebar request targeting main, canceled confirmation, sidebar polling, settings, breakpoint crossing and completed desktop resize. They do not transcribe DOM selectors, listener wiring or implementation functions. Those are checked by real browser scenarios.

| Requirement | Quint | Alloy | Production evidence |
| --- | --- | --- | --- |
| R1 drawer and synchronized accessibility/backdrop | `accessibility`, `desktop`, toggle/backdrop/Escape tests | `Accessibility`, `DesktopAndBounds`, `EscapeRestoresFocus` | Browser drawer, keyboard and backdrop scenarios |
| Close on session/New/confirmed Run All, retain on canceled confirmation/poll/settings | `selectiveClose`, canceled/poll/settings/navigation tests | `SelectiveClose`, `CancelThenNavigate`, `PollAndSettingsPreserve` | Real htmx requests, confirmation dialog and polling |
| Breakpoint reset without losing desktop resize and persistence, R2 | `persistence`, `widthBounds`, breakpoint/clamp tests | `Persistence`, `ResizeSurvivesCrossing`, `DesktopAndBounds` | Browser media-query crossing, keyboard/drag resize and reload |
| R3-R9 dynamic height, wrapping, scroll, dialogs, toasts, form font and Markdown containment | Deliberately not pixel-modeled | Deliberately not pixel-modeled | Actual generated CSS and browser layout assertions |

Width samples in Quint cover clamping below 180 and above 600, both endpoints, default 288, keyboard-step neighbors and interior drag widths. Alloy uses six named width classes, documented in its header. Width persistence distinguishes an invalid previously stored width from the clamped rendered width: startup does not rewrite localStorage. A resize is modeled atomically at completion, not during each drag frame. Browser tests cover the actual numeric widths and intermediate rendering. Only visible user controls are actuated, so mobile toggle and desktop resizing have viewport guards. Focus is a record that Escape restored focus, not a claim that focus can never subsequently move.

These are bounded safety checks, not unbounded proof, liveness, a formal refinement proof of JavaScript/Rust, or exhaustive browser/device testing. CSS geometry is intentionally tested in the browser instead of duplicated in a symbolic CSS interpreter.

## Reproducible execution

Use Node **26.12.1**, Rust **1.99.1** and run from the repository root:

```sh
bash scripts/check_responsive_webui_specs.sh
```

The runner reuses `scripts/setup_formal.sh` and `tests/formal/package-lock.json`: Quint **0.33.0**, Apalache **0.62.1**, Alloy **6.2.0**, Temurin **25.0.5+8-LTS**. The JDK archives for macOS arm64 and Linux x64 and the released Alloy CLI jar are checksum-pinned in that setup. Alloy jar SHA256: `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`. Unsupported platforms or missing toolchains fail, never count as passes. No account credentials, external model providers or PR secrets are needed.

Production BDD reuses the existing Rust Cucumber **0.23.0** dependency and adds a locked `tests/webui` package with Playwright **1.62.1**, selecting Chromium **151.0.7922.34** / revision **1234**. Its wrapper installs the browser/system dependencies, captures the actual runtime version, runs the production Cucumber target and runs all three negative controls. See `../webui/README.md` for scenario-level production entrypoints and fixture isolation.

Actual commands executed by the runner (`Q=tests/formal/node_modules/.bin/quint`, `R` is a fresh report directory, `PORT` is an OS-allocated isolated port):

```sh
"$Q" typecheck tests/formal/responsive_webui.qnt
"$Q" test tests/formal/responsive_webui.qnt --backend typescript --seed 20261025 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261025 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety --out-itf "$R/simulation_{seq}.itf.json" tests/formal/responsive_webui.qnt
"$Q" verify --invariant safety --max-steps 8 --main responsive_webui --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/responsive_webui.qnt
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/responsive_webui.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/responsive_webui.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
bash scripts/check_responsive_webui_bdd.sh
cargo +1.99.1 test --locked --all-features --bin cruise-runtime webui::
cargo +1.99.1 clippy --locked --all-targets --all-features
cargo +1.99.1 fmt --check
```

Installed help is captured for Quint `typecheck`, `test`, `run`, `verify` and Alloy `help exec`, plus versions and solver enumeration. The pinned Alloy CLI **does not offer `--expect`**. Its supported equivalent is command-level `expect 0` on every assertion and `expect 1` on every witness, checked independently against every receipt entry. No default-command assumption is used. Receipt validation fails on missing/duplicate/extra commands, wrong expectations, absent witness instances or malformed solutions.

Quint simulation uses seed **20261025**, **1000 samples**, **20 transitions** per sample. Simulation is not proof. Bounded verification uses **Apalache 0.62.1**, **8 transitions** and aggregate `safety`, including each named safety property. Alloy checks **exactly 9 ordered states / 8 transitions**, using **SAT4J**, with 4 assertions and 4 meaningful satisfiable witnesses.

## CI and evidence

`.github/workflows/responsive-webui-specs.yml` runs the aggregate runner on every main-targeted PR and main push, without path exclusions or secrets. It installs pinned Node/Rust and retains all `target/formal-reports/` as an artifact even after failure. Each aggregate invocation allocates `responsive-webui.*`, logs commands and individual exit codes, captures CLI help, verification logs, test/simulation traces, Alloy receipt and concrete witness instances. Independent checks continue after failures, but aggregate exit remains nonzero. See `RESPONSIVE_WEBUI_VERIFICATION.md` for actual local results, defects and limitations.
