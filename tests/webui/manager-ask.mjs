// Invoked by the Rust Manager router test with a live, owned Ask in flight.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const [detail, promptId, answer] = process.argv.slice(2);
assert(detail && promptId && answer, 'the Rust fixture must supply the live prompt');
const url = new URL(detail);
const action = new URL(`/webui${url.pathname}/answer`, url.origin).href;
const report = resolve(process.env.WEBUI_MANAGER_REPORT_DIR || 'target/formal-reports/manager-browser');
await mkdir(report, { recursive: true });
const env = { ...process.env };
for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete env[key];
const browser = await chromium.launch({ env, headless: true });
try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, reducedMotion: 'reduce' });
  await context.addCookies([{ name: 'cruise_webui', value: 'test-session', url: url.origin }]);
  const page = await context.newPage();
  page.setDefaultTimeout(10000);
  try {
    assert.equal((await page.goto(detail)).status(), 200, 'authenticated Manager detail loads');
    await page.getByText('Which local answer should the managed task use?', { exact: true }).waitFor();
    const form = page.locator('form').filter({ has: page.locator('textarea[name="answer"]') });
    assert.equal(await form.locator('[name="prompt_id"]').inputValue(), promptId);
    const goalId = await form.locator('[name="goal_id"]').inputValue();
    assert(goalId, 'the rendered Ask binds its Goal');
    const input = form.getByRole('textbox');
    const submit = form.getByRole('button', { name: /answer|send/i });
    const waitForAnswer = () => page.waitForResponse(r => r.url() === action && r.request().method() === 'POST');
    await input.fill(' ');
    const [empty] = await Promise.all([waitForAnswer(), submit.click()]);
    assert.equal(empty.status(), 400, 'blank reply is rejected');
    await page.getByText(/must not be empty/i).waitFor();
    await form.waitFor();
    await page.screenshot({ path: join(report, 'ask-blank-rejected.png'), fullPage: true });
    await input.fill(answer);
    await page.screenshot({ path: join(report, 'ask-answer.png'), fullPage: true });
    const [response] = await Promise.all([
      waitForAnswer(), submit.click(),
    ]);
    assert(response.ok(), 'answer accepted');
    await form.waitFor({ state: 'detached' });
    assert.equal(await page.getByText(/must not be empty/i).count(), 0, 'successful submission clears the error');
  } catch (error) {
    await page.screenshot({ path: join(report, 'ask-failure.png'), fullPage: true }).catch(() => {});
    throw error;
  }
} finally {
  await browser.close();
}
