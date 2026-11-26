// Approve the updated repository model from the WebUI review panel and capture it.
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
const [root, id, out] = process.argv.slice(2);
const repo = resolve('.');
const { chromium } = createRequire(resolve(repo, 'tests/webui/package.json'))('playwright');
const { loginWebUi } = await import(resolve(repo, 'tests/webui/auth.mjs'));
const inSh = join(repo, '.agents/skills/verify-cruise/scripts/in.sh');
const cruise = (...args) => execFileSync(inSh, [root, 'cruise', ...args], { encoding: 'utf8' });
const phase = () => JSON.parse(cruise('list', '--json')).find(s => s.id === id)?.phase;
const loginUrl = cruise('webui').trim();
const base = new URL(loginUrl).origin;
const browser = await chromium.launch({ headless: true, channel: process.env.VERIFY_CRUISE_BROWSER_CHANNEL || undefined });
try {
  const page = await (await browser.newContext({ viewport: { width: 1280, height: 900 }, colorScheme: 'light' })).newPage();
  page.setDefaultTimeout(120000);
  await loginWebUi(page, loginUrl);
  await page.goto(`${base}/sessions/${id}`);
  const approve = page.getByRole('button', { name: 'Approve updated Quint model' });
  await approve.waitFor();
  await page.locator('img[src*="prompt"], img').first().waitFor();
  await page.screenshot({ path: join(out, '01-updated-review.png') });
  await approve.click();
  const deadline = Date.now() + 120000;
  while (phase() !== 'Completed') {
    if (Date.now() > deadline) throw new Error(`phase ${phase()}`);
    await new Promise(ok => setTimeout(ok, 500));
  }
  await page.reload();
  await page.screenshot({ path: join(out, '02-completed.png') });
  console.log(JSON.stringify({ id, phase: phase() }));
} finally { await browser.close(); }
