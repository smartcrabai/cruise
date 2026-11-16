# Electron desktop UI policy

Apply the [common UI E2E policy](common.md) to the actual Electron desktop application, not an ordinary browser renderer.

- When suitable visual tooling is absent, use Playwright Electron API to interact with the real application and save screenshots of meaningful states for pixel inspection.
- When suitable persistent E2E tooling is absent, store Playwright Test source using Electron API and provide the repeatable `npx playwright test` command.
- Playwright Electron API is experimental: verify and pin compatible versions before use. A renderer tested in an ordinary browser is not Electron coverage. Stubbing an OS-native dialog is not coverage of the real OS dialog.
- If the supported host/runtime or required capability is unavailable, report the exact blocker and complete other reachable work without claiming that capability ran.
