// Real production SSE fan-out must render each connected client's locale independently.
// Run from the repository root with the Playwright dependencies and runtime built.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-sse-locale-'));
const repo = join(root, 'repo');
const id = '20261025000000005';
const session = join(root, 'data/cruise/sessions', id);
const config = join(root, 'config/cruise');
const output = resolve(process.env.WEBUI_LANGUAGE_REPORT_DIR || 'target/formal-reports/language-browser');
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  LC_ALL: 'C', LANG: 'C', CRUISE_UI_LANGUAGE: 'en', NO_COLOR: '1',
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('SSE locale watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(output, { recursive: true });
  await mkdir(repo, { recursive: true });
  for (const args of [['init', '-b', 'main'], ['-c', 'user.name=SSE test', '-c', 'user.email=sse@example.invalid', 'commit', '--allow-empty', '-m', 'fixture']]) {
    const result = spawnSync('git', args, { cwd: repo, env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
  }
  await mkdir(session, { recursive: true });
  await mkdir(config, { recursive: true });
  await writeFile(join(session, 'state.json'), JSON.stringify({ id, base_dir: repo, phase: 'Planned',
    config: { kind: 'inline_snapshot' }, input: 'SSE fixture', title: 'SSE fixture',
    current_step: null, created_at: '2026-10-25T00:00:00Z', completed_at: null,
    worktree_path: null, worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main', allow_dirty_working_tree: true }));
  await writeFile(join(session, 'config.yaml'), "steps:\n  first: {command: 'true'}\n");
  await writeFile(join(session, 'plan.md'), '# SSE fixture\n');
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
  const pages = [];
  for (const [locale, expected] of [['en', 'Run All'], ['ja', 'すべて実行']]) {
    const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
    await context.addCookies([{ name: 'cruise_ui_language', value: locale, url: base }]);
    const page = await context.newPage();
    // Observe a clone of the production HTMX SSE response without changing it.
    await page.addInitScript(() => {
      const nativeFetch = window.fetch;
      window.__sseOpen = false;
      window.__sseText = '';
      window.fetch = async (...args) => {
        const response = await nativeFetch(...args);
        if (response.headers.get('content-type')?.includes('text/event-stream')) {
          window.__sseOpen = true;
          const reader = response.clone().body.getReader();
          const decoder = new TextDecoder();
          (async () => {
            for (;;) {
              const { value, done } = await reader.read();
              if (done) break;
              window.__sseText += decoder.decode(value, { stream: true });
            }
          })().catch(() => {});
        }
        return response;
      };
    });
    assert.equal((await page.goto(`${base}/`)).status(), 200);
    await page.waitForFunction(() => window.__sseOpen, null, { timeout: 25000 });
    assert.equal(await page.locator('html').getAttribute('lang'), locale);
    assert.equal((await page.locator('#run-all-control').innerText()).trim(), expected);
    await page.evaluate(() => { window.__originalRunAllControl = document.querySelector('#run-all-control'); });
    pages.push({ context, page, locale, expected });
  }
  // Run the real isolated workflow after both subscriptions are established.
  const response = await pages[0].page.evaluate(() => fetch('/webui/run-all', { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: '' }).then(r => r.status));
  assert.equal(response, 200);
  for (const { page, locale, expected } of pages) {
    // Batch completion publishes a real sidebar control partial to both streams.
    await page.waitForFunction(expected => window.__sseText.includes('hx-target="#run-all-control"') && window.__sseText.includes(expected), expected, { timeout: 30000 });
    const stream = await page.evaluate(() => window.__sseText);
    const control = stream.match(/<hx-partial hx-target="#run-all-control"[^]*?<\/hx-partial>/g) || [];
    assert(control.some(partial => partial.includes(`>${expected}</button>`)), `${locale}: localized SSE control: ${control.join('\n')}`);
    await page.waitForFunction(() => document.querySelector('#run-all-control') !== window.__originalRunAllControl);
    assert.equal((await page.locator('#run-all-control').innerText()).trim(), expected);
    await page.screenshot({ path: join(output, `sse-${locale}.png`) });
    if (locale === 'ja') {
      await page.locator('#toasts > *').first().waitFor();
      assert.doesNotMatch(await page.locator('#toasts').innerText(), /\b(?:Completed|Run All finished)\b/, 'Japanese SSE notifications must be localized');
    }
  }
  console.log('WEBUI_SSE_LOCALE_CONTRACT_PASSED');
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
