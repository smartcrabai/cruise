# UI end-to-end testing policy

This policy applies only to applicable browser-facing, mobile, and desktop UI flows. Use the matching framework policy for target-specific recommendations; a framework policy does not exclude other repository targets.

## Scope and evidence

- Inventory each real in-scope UI target and classify its actual framework from manifests, source, and app entry points before choosing tools. React Native/Expo remains React Native/Expo when generated `ios/` files exist; Electron means its real desktop application, not an ordinary browser renderer; Tauri means its native WebView plus Rust/IPC application. Inspect existing tests, CI, and repository instructions. Preserve suitable established tooling, versions, commands, setup, and CI; do not add unrelated suites.
- This is UI E2E policy only. Keep ordinary CLI, HTTP API, Testcontainers real-service, Pact consumer/provider contract, and other default test-stack coverage in ordinary workflows; do not move it behind a UI-E2E flag.
- Use available relevant skills and connected tools when useful, but they are optional. Do not guess tool or API names, require a skill/MCP loader, create skills, edit global MCP/config, or block solely because optional skills or MCP are unavailable. Follow repository instructions and never let optional guidance override repository or user requirements.
- Reuse suitable existing tools independently for each capability. Outside the `e2e` workflow flag, consider a framework recommendation only when its corresponding capability is genuinely missing. Verify compatibility for the actual app, host, runtime, and client; pin compatible versions and revisions when introducing a tool. Do not silently switch runners when a selected setup is unsupported.

## Tooling boundary under the `e2e` workflow flag

Built-in `e2e` flag steps (TDD `e2e`; refactor `prepare-e2e` and `e2e-final-check`) use only tooling the repository and environment already provide. They do not install, download, add, upgrade, or downgrade dependencies, toolchains, or test tools; edit dependency manifests, lockfiles, toolchain pins, or CI configuration; or introduce an E2E framework the repository does not already use. They never substitute another target, platform build, or runner (for example, a web export for a native app) for one that cannot run. Framework recommendations in these policies are the preparation such a step reports for a missing capability, not tools it adds. A host guard compares the working tree with a baseline recorded before each step and makes the step undo such changes.

Each run report lists every blocked capability under `## Blockers` with what is missing, the command and result proving it, and the exact preparation needed, then ends with exactly one `E2E_STATUS: passed`, `E2E_STATUS: blocked`, or `E2E_STATUS: not-applicable` line. `blocked` stops the session before later steps, with the blockers as its failure reason. Prepare the prerequisites and resume, or resume with `-f e2e=off` to continue without UI E2E. Workflows whose approved plan adds UI E2E tooling, such as `add-tests`, may still introduce framework recommendations.

## Two separate capabilities

For each applicable UI flow, assess and deliver these separately whenever available:

1. **Real-app visual interaction:** launch and interact with the actual app in a browser or representative simulator, emulator, or supported desktop runtime. Save screenshots of meaningful success and failure/boundary states, then inspect the actual image pixels. DOM/accessibility output, a screenshot that was not pixel-inspected, and SwiftUI Preview alone are not visual evidence.
2. **Persistent deterministic E2E:** store assertion-bearing E2E source in the repository, use isolated/reset deterministic test data, assert consumer-visible behavior and relevant failure/boundary behavior through real production paths, and provide repeatable build/run commands. Execute independently of prior exploration sessions and LLM/MCP decisions.

One actual E2E run can satisfy both capabilities when it produces screenshots that are saved and pixel-inspected; do not duplicate a run solely to satisfy both, and no pixel-diff/golden test is required. Unit tests, DOM-only tests, mocks, parser-only runs, and screenshot capture without pixel inspection do not substitute for the capabilities above.

If only one capability is available, complete reachable work and report the missing capability or exact blocker; do not claim both. Work each target and capability independently so one blocker does not stop feasible work elsewhere. Never claim a fake pass or call a target not applicable merely because tooling is absent. Report unavailable capability, exact prerequisite, completed reachable work, and any unverified scope honestly.
Run prepared E2E suites and checks needed to diagnose failures. Fix genuine application defects without weakening assertions, then rerun affected checks. If no in-scope UI target exists, report not applicable with repository evidence and do not add a fake suite. Never use live external credentials or production services.

## Report

For each target, record its framework and actual app/runtime; visual tool and version, saved screenshot paths, and evidence that screenshot pixels were inspected; persistent E2E source and substantive assertions; reset/isolation; exact setup, build, and test commands and results; and CI changes or, under the `e2e` flag, CI wiring the suite still needs. Keep UI E2E results separate from ordinary CLI/API/default test coverage. State genuine failures and fixes, missing capabilities and exact blockers, work completed, and unverified scope.

Outside the `e2e` flag, wire a prepared suite into the existing provider-native CI only when it is reproducible on a supported runner and available without secrets. Make failures fail CI. Preserve applicable browser/Electron checks on supported hosts and Linux web CI; do not disable them to avoid an unsupported native target.

## No-new-tool policy: targets without a listed framework recommendation

Windows/Linux native applications and other unlisted targets or capabilities are not excluded or unsupported. Reuse appropriate existing tooling for each capability when it is present. If no suitable existing tooling exists, do not recommend or introduce new platform tooling or a suite for an unlisted target; do reachable work and report the limitation honestly. Do not force a new platform matrix. Tauri is explicitly listed and may use its framework recommendations on macOS, Windows, or Linux only after confirming compatibility. Do not add new Maestro, Appium WinCore, or dogtail/AT-SPI defaults; reuse those only when already established and appropriate. Do not generalize unsupported Apple OS targets.

## Retrieving policy while offline

Policy text is embedded in the Cruise executable; reading it does not need a repository checkout, network, user configuration, telemetry, or tool installation. Ask the available executable with `cruise docs e2e` for this common policy and framework IDs, or `cruise docs e2e <id>` for one framework policy. If `cruise` is not on `PATH` but the installed executable's path is already known, invoke that same executable by its path (for example, `/known/path/cruise-runtime docs e2e`). If no usable executable path is known or reachable, report that exact blocker and do not auto-install Cruise, alter `PATH`, or mutate configuration.
