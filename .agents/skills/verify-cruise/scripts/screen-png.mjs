// Render verify_pty screen HTML files to PNG with the repository's pinned Playwright.
// Usage: node .agents/skills/verify-cruise/scripts/screen-png.mjs SCREEN.html [...]
// Writes SCREEN.png next to each input. Requires `npm ci --prefix tests/webui`
// and `tests/webui/node_modules/.bin/playwright install chromium`.
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { resolve } from 'node:path';

const repo = resolve(fileURLToPath(import.meta.url), '../../../../..');
const { chromium } = createRequire(resolve(repo, 'tests/webui/package.json'))('playwright');
const inputs = process.argv.slice(2);
if (!inputs.length || inputs.some(path => !path.endsWith('.html'))) {
  console.error('usage: screen-png.mjs SCREEN.html [...]');
  process.exit(2);
}
const browser = await chromium.launch({ headless: true, channel: process.env.VERIFY_CRUISE_BROWSER_CHANNEL || undefined });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 400 }, deviceScaleFactor: 1 });
  for (const input of inputs) {
    const path = resolve(input);
    await page.goto(pathToFileURL(path).href);
    const output = path.replace(/\.html$/, '.png');
    await page.locator('main').screenshot({ path: output });
    console.log(output);
  }
} finally {
  await browser.close();
}
