// Browser-level contract for request-local WebUI language selection.
// Run from the repository root after building cruise-runtime and installing Playwright.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-language-browser-'));
const repo = join(root, 'repo');
const id = '20261025000000004';
const session = join(root, 'data/cruise/sessions', id);
const config = join(root, 'config/cruise');
const output = resolve(process.env.WEBUI_LANGUAGE_REPORT_DIR || 'target/formal-reports/language-browser');
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  LC_ALL: 'C', LANG: 'C', CRUISE_UI_LANGUAGE: 'en',
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('language browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(output, { recursive: true });
  await mkdir(repo, { recursive: true });
  await mkdir(session, { recursive: true });
  await writeFile(join(session, 'state.json'), JSON.stringify({ id, base_dir: repo, phase: 'Planned',
    config: { kind: 'inline_snapshot' }, input: 'Browser fixture', title: 'Browser fixture',
    current_step: null, created_at: '2026-10-25T00:00:00Z', completed_at: null,
    worktree_path: null, worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main' }));
  await writeFile(join(session, 'config.yaml'), "steps:\n  first: {command: 'true'}\n");
  await writeFile(join(session, 'plan.md'), '# Browser fixture\n');
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['webui', '--port', '0', '--webui-dir', resolve('webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', data => { logs += data; });
  const base = await new Promise((ok, fail) => {
    let stdout = '';
    const timeout = setTimeout(() => fail(new Error(`server startup timeout: ${logs}`)), 20000);
    server.once('error', error => { clearTimeout(timeout); fail(error); });
    server.once('exit', code => { clearTimeout(timeout); fail(new Error(`server exited ${code}: ${logs}`)); });
    server.stdout.on('data', data => { stdout += data; const match = stdout.match(/http:\/\/127\.0\.0\.1:\d+/); if (match) { clearTimeout(timeout); ok(match[0]); } });
  });
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
  // Only the external Mermaid module is blocked. The real server, HTMX and app.js run normally.
  await context.route('https://cdn.jsdelivr.net/npm/mermaid@11/**', route => route.abort('failed'));
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  const selector = page.locator('#sidebar select:visible');
  await selector.waitFor();
  const options = await selector.locator('option').evaluateAll(nodes => nodes.map(n => [n.value, n.textContent.trim(), n.selected]));
  assert.deepEqual(options.map(o => o[0]), ['en', 'zh-CN', 'es', 'fr', 'pt-BR', 'ja', 'ko']);
  assert.deepEqual(options.map(o => o[1]), ['English', '简体中文', 'Español', 'Français', 'Português (Brasil)', '日本語', '한국어']);
  assert.deepEqual(options.filter(o => o[2]).map(o => o[0]), ['en']);
  assert.equal(await page.locator('html').getAttribute('lang'), 'en');
  assert.equal(await page.locator('#sidebar h2').innerText(), 'Sessions');
  const beforeConfig = await readFile(join(config, 'config.json'), 'utf8').catch(error => { if (error.code === 'ENOENT') return null; throw error; });
  // Wait for navigation explicitly: a reload can otherwise race the following assertions.
  let navigation = page.waitForEvent('load');
  await selector.selectOption('ja');
  await navigation;
  assert.equal(await page.locator('html').getAttribute('lang'), 'ja');
  assert.equal(await page.locator('#sidebar h2').innerText(), 'セッション');
  assert.equal(await page.locator('#sidebar select:visible').inputValue(), 'ja');
  await page.screenshot({ path: join(output, 'desktop-ja.png') });
  assert.doesNotMatch(await page.locator('#session-detail').innerText(), /\b(Replan|Publish as Issue|Create worktree|Run on current branch|Settings|Planned)\b/, 'Japanese session controls and phase must be localized');
  const cookie = (await context.cookies(base)).find(c => c.name === 'cruise_ui_language');
  assert(cookie, 'language preference must be stored in a cookie');
  assert.equal(cookie.value, 'ja');
  assert.equal(cookie.path, '/');
  assert.equal(cookie.sameSite, 'Lax');
  assert(cookie.expires > Date.now() / 1000 + 300 * 86400, 'language cookie persists for approximately a year');
  await page.reload();
  assert.equal(await page.locator('html').getAttribute('lang'), 'ja');
  assert.equal(await page.locator('#sidebar h2').innerText(), 'セッション');
  const second = await context.newPage();
  assert.equal((await second.goto(`${base}/sessions/${id}`)).status(), 200);
  assert.equal(await second.locator('html').getAttribute('lang'), 'ja');
  assert.equal(await second.locator('#sidebar h2').innerText(), 'セッション');
  // Check stable production UI surfaces, excluding fixture data and permitted proper nouns.
  const ui = await page.locator('#sidebar').innerText();
  assert.doesNotMatch(ui, /\b(Sessions|Settings|Clean|New|Run All)\b/, 'Japanese sidebar must not retain English UI labels');
  const graph = page.locator(`[role=tab][hx-get$="/tab/dag"]`);
  await graph.click();
  const pre = page.locator(`#tab-dag-${id} pre.mermaid`);
  await pre.waitFor();
  await page.waitForFunction(id => {
    const node = document.querySelector(`#tab-dag-${id} pre.mermaid`);
    const label = document.querySelector('#app')?.getAttribute('data-graph-error-label');
    return !!node && !!label && node.textContent.startsWith(label);
  }, id);
  const graphLabel = await page.locator('#app').getAttribute('data-graph-error-label');
  assert(graphLabel && !graphLabel.includes('Failed to render Graph'), 'graph error label is translated');
  assert((await pre.innerText()).startsWith(graphLabel), 'failed Mermaid import uses the server-provided graph error label');
  await page.screenshot({ path: join(output, 'graph-error-ja.png') });
  navigation = page.waitForEvent('load');
  await page.locator('#sidebar select:visible').selectOption('en');
  await navigation;
  assert.equal(await page.locator('html').getAttribute('lang'), 'en');
  assert.equal(await page.locator('#sidebar h2').innerText(), 'Sessions');
  assert.equal((await context.cookies(base)).find(c => c.name === 'cruise_ui_language')?.value, 'en');
  assert.equal(await readFile(join(config, 'config.json'), 'utf8').catch(error => { if (error.code === 'ENOENT') return null; throw error; }), beforeConfig, 'browser preference must not modify AppConfig');
  // Exercise every supported locale through the actual mobile selector and reload.
  // A separate context also proves that preferences do not leak between clients.
  const mobile = await browser.newContext({ viewport: { width: 375, height: 800 }, reducedMotion: 'reduce' });
  const mobilePage = await mobile.newPage();
  assert.equal((await mobilePage.goto(`${base}/sessions/${id}`)).status(), 200);
  assert.equal(await mobilePage.locator('html').getAttribute('lang'), 'en');
  const expectedSessions = new Map([
    ['zh-CN', '会话'], ['es', 'Sesiones'], ['fr', 'Sessions'],
    ['pt-BR', 'Sessões'], ['ja', 'セッション'], ['ko', '세션'], ['en', 'Sessions'],
  ]);
  for (const [locale, heading] of expectedSessions) {
    const loaded = mobilePage.waitForEvent('load');
    await mobilePage.locator('#ui-language-mobile').selectOption(locale);
    await loaded;
    assert.equal(await mobilePage.locator('html').getAttribute('lang'), locale);
    assert.equal(await mobilePage.locator('#ui-language-mobile').inputValue(), locale);
    assert.equal(await mobilePage.locator('#sidebar h2').textContent(), heading);
    assert.equal((await mobile.cookies(base)).find(c => c.name === 'cruise_ui_language')?.value, locale);
    if (locale === 'ja') await mobilePage.screenshot({ path: join(output, 'mobile-ja.png') });
  }
  await mobile.addCookies([{ name: 'cruise_ui_language', value: 'invalid', url: base }]);
  await mobilePage.reload();
  assert.equal(await mobilePage.locator('html').getAttribute('lang'), 'en', 'invalid cookie falls back to process locale');
  assert.equal(await mobilePage.locator('#ui-language-mobile').inputValue(), 'en');
  await mobilePage.screenshot({ path: join(output, 'mobile-invalid-cookie.png') });
  assert.equal(await page.locator('html').getAttribute('lang'), 'en', 'another context must not change this client');
  await mobile.close();
  assert.deepEqual(errors, [], 'production JavaScript must not throw');
  console.log('WEBUI_BROWSER_CONTRACT_PASSED language');
} catch (error) {
  console.error(error, '\nProduction server stderr:', logs);
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
  try { await browser?.close(); } finally {
    try { if (server) { server.kill('SIGKILL'); await closed; } } finally {
      await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}
