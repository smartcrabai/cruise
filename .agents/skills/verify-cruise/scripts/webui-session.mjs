// Drive one Planned session through the WebUI: open it, run it on the current
// branch, answer its Option prompt in the browser, and capture each state.
// Usage: node .agents/skills/verify-cruise/scripts/webui-session.mjs ROOT SESSION_ID CHOICE OUTDIR
// CHOICE is the visible selector label, for example "Fast path".
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';

const [root, id, choice, outArg] = process.argv.slice(2);
if (!root || !id || !choice || !outArg) {
  console.error('usage: webui-session.mjs ROOT SESSION_ID CHOICE OUTDIR');
  process.exit(2);
}
const scripts = resolve(fileURLToPath(import.meta.url), '..');
const repo = resolve(scripts, '../../../..');
const { chromium } = createRequire(resolve(repo, 'tests/webui/package.json'))('playwright');
const { loginWebUi } = await import(resolve(repo, 'tests/webui/auth.mjs'));
const out = resolve(outArg);
mkdirSync(out, { recursive: true });
const cruise = (...args) => execFileSync(join(scripts, 'in.sh'), [root, 'cruise', ...args], { encoding: 'utf8' });
const phase = () => JSON.parse(cruise('list', '--json')).find(session => session.id === id)?.phase;
const loginUrl = cruise('webui').trim();
const base = new URL(loginUrl).origin;
const steps = [];
const browser = await chromium.launch({ headless: true, channel: process.env.VERIFY_CRUISE_BROWSER_CHANNEL || undefined });
try {
  const page = await (await browser.newContext({ viewport: { width: 1280, height: 800 }, colorScheme: 'light' })).newPage();
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await loginWebUi(page, loginUrl);
  const capture = async name => {
    await page.screenshot({ path: join(out, `${name}.png`) });
    writeFileSync(join(out, `${name}.aria.txt`), await page.locator('body').ariaSnapshot());
    steps.push({ name, url: new URL(page.url()).pathname, phase: phase() });
  };
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.locator(`#session-detail[data-session-id="${id}"]`).waitFor();
  await capture('01-planned');
  await page.getByRole('button', { name: 'Run on current branch', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.waitFor();
  await capture('02-option');
  await dialog.getByRole('button', { name: new RegExp(choice.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).click();
  const submit = dialog.getByRole('button', { name: 'Submit', exact: true });
  if (await submit.isVisible()) await submit.click();
  await dialog.waitFor({ state: 'detached' });
  const deadline = Date.now() + 30000;
  while (phase() !== 'Completed') {
    assert(Date.now() < deadline, `session did not complete; phase ${phase()}`);
    await new Promise(ok => setTimeout(ok, 250));
  }
  await page.reload();
  await page.locator(`#session-detail[data-session-id="${id}"]`).waitFor();
  await capture('03-completed');
  assert.deepEqual(errors, [], 'WebUI JavaScript errors');
  const chosen = readFileSync(join(root, 'repo', 'chosen.txt'), 'utf8');
  writeFileSync(join(out, 'result.json'), `${JSON.stringify({ id, choice, chosen, steps }, null, 2)}\n`);
  console.log(JSON.stringify({ id, choice, chosen, phase: phase() }));
} finally {
  await browser.close();
}
