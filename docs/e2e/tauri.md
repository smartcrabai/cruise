# Tauri desktop UI policy

Classify and test an actual native Tauri app (WebView plus Rust/IPC), not an ordinary browser or Electron frontend. Apply the [common UI E2E policy](common.md).

- **Visual interaction:** only when suitable visual tooling is absent, use [tauri-plugin-mcp](https://github.com/P3GLEG/tauri-plugin-mcp) for real-app input and screenshot capture, then inspect actual screenshot pixels.
- **Persistent E2E:** only when suitable saved E2E tooling is absent, use [tauri-webdriver](https://github.com/Choochmeque/tauri-webdriver) with companion [tauri-plugin-webdriver](https://github.com/Choochmeque/tauri-plugin-webdriver) and a real WebDriver client/test runner. Reuse an existing client or select a stack-compatible one such as WebdriverIO or Selenium; the server alone is not a suite.
- Pin compatible versions/Git revisions and confirm compatibility for the host, Tauri, WebView, and client. Keep both plugins dev/test-only and exclude automation endpoints from production builds. Use local/isolated test sessions; do not globally enable MCP.
- Include substantive assertions for relevant real frontend-to-Rust IPC/native behavior. Report limitations from mocks or synthetic/untrusted events. The plugins' default stubs for JavaScript `alert`, `confirm`, and `prompt` do not establish real native-dialog evidence.
- If the selected tools/runtime are unsupported or unavailable, report the exact blocker and complete any other reachable capability without silently switching tools or claiming a pass.
