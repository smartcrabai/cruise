# Browser UI policy

Apply the [common UI E2E policy](common.md), then use these recommendations only for missing capabilities in an actual browser application. Browser extensions use the `browser-extension` policy; Exact apps use the `exact` policy on every host, including the web.

- **Visual interaction:** only when suitable browser visual tooling is absent, use Playwright CLI (`playwright-cli`) for final real-browser interaction and screenshot capture. Save meaningful success and failure/boundary states and inspect the actual screenshot pixels.
- **Persistent E2E:** only when suitable saved E2E tooling is absent, store assertion-bearing Playwright Test source (`*.spec.ts`) and provide its repeatable `npx playwright test` command. CLI history or generated action code without assertion-bearing test source is not persistent E2E.
- Confirm current compatibility in official documentation before introducing tools and pin compatible runner and browser versions. Use the real application path; a parser-only run or DOM-only check is not browser E2E.
