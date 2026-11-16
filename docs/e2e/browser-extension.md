# Browser extension UI policy

Applies to browser extensions (for example Manifest V3 extensions built with Extension.js, WXT, or Plasmo), not ordinary web pages. Apply the [common UI E2E policy](common.md).

- **Build first:** test the built unpacked extension produced by the project's own build command for the target browser, not the source tree.
- **Persistent E2E:** only when suitable saved extension tests are absent, store Playwright Test source with a fixture that launches `chromium.launchPersistentContext` using `channel: 'chromium'` and `--disable-extensions-except=<dir>` plus `--load-extension=<dir>`. Read the extension ID from the Manifest V3 service worker URL, then assert popup and options pages (`chrome-extension://<id>/…`), content-script effects on local fixture pages, and service-worker behavior. Provide the repeatable `npx playwright test` command and pin compatible versions.
- **Visual interaction:** only when suitable visual tooling is absent, use the same persistent Chromium context to interact with the loaded extension and save screenshots of its pages and affected fixture pages, then inspect the pixels.
- Extensions load only in Chromium persistent contexts; Google Chrome and Microsoft Edge removed the side-load flags, so use Playwright's bundled Chromium. Use a fresh user data directory per test and local fixture pages, never live third-party sites or real accounts.
- Opening `popup.html` directly does not exercise a toolbar-icon click; report that limit. MV3 service workers suspend after about 30 seconds idle, and an `evaluate()` in flight at that moment throws.
- A Chromium run is not coverage of Firefox or Safari builds; for those, reuse existing tooling and report missing capability.
