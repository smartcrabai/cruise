// Real browser regression for the optional PR field. No network or forge stubs.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-pr-form-'));
const output = resolve(process.env.WEBUI_PR_REPORT_DIR || 'target/formal-reports/pr-form-browser');
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_TERMINAL_PROMPT: '0', GH_PROMPT_DISABLED: '1' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('PR form browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(output, { recursive: true });
  const repoDir = join(root, 'repo');
  await mkdir(repoDir);
  for (const args of [['init', '-b', 'main'], ['config', 'user.name', 'PR form test'],
    ['config', 'user.email', 'pr-form@example.invalid'], ['commit', '--allow-empty', '-m', 'fixture']]) {
    const result = spawnSync('git', args, { cwd: repoDir, env, encoding: 'utf8' });
    assert.equal(result.status, 0, `isolated git fixture: ${result.stderr}`);
  }
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['webui', '--port', '0', '--webui-dir', resolve('webui')], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
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
  console.log(`Chromium ${browser.version()}`);
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  assert.equal((await page.goto(`${base}/new`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  const pr = page.locator('#pr-number-input');
  const repo = page.getByRole('radio', { name: 'Repository (GitHub or GitLab)' });
  const dir = page.getByRole('radio', { name: 'Directory' });
  await dir.check();
  assert.equal(await pr.isVisible(), false, 'PR input hidden for directory');
  await repo.check();
  assert.equal(await pr.isVisible(), true, 'PR input visible for repository');
  assert.equal(await pr.getAttribute('min'), '1');
  assert.equal(await pr.getAttribute('step'), '1');
  await page.locator('#repo-input').fill('example/nonexistent');
  await page.locator('#task-input').fill('Browser validation fixture');
  await page.screenshot({ path: join(output, 'repository-form.png'), fullPage: true });
  let posts = 0;
  page.on('request', request => { if (request.method() === 'POST' && new URL(request.url()).pathname === '/webui/sessions') posts++; });
  for (const value of ['0', '-1', '1.5']) {
    await pr.fill(value);
    assert.equal(await pr.evaluate(element => element.validity.valid), false, `${value} must be invalid in browser`);
    await page.getByRole('button', { name: 'Create & plan' }).click();
    assert.equal(posts, 0, `${value} must not submit`);
  }
  await page.screenshot({ path: join(output, 'invalid-number.png'), fullPage: true });
  for (const value of ['', '1']) {
    await pr.fill(value);
    assert.equal(await pr.evaluate(element => element.validity.valid), true, `${value || 'empty'} is allowed`);
  }
  // Create draft is a separate real HTMX endpoint and bypasses native form
  // validation, so it exercises the server's u64 boundary without forge access.
  for (const value of ['0', '18446744073709551616']) {
    await pr.fill(value);
    const response = page.waitForResponse(r => r.url().endsWith('/webui/sessions/draft') && r.request().method() === 'POST');
    await page.getByRole('button', { name: 'Create draft' }).click();
    const result = await response;
    assert.equal(result.status(), 400, `server rejects ${value}`);
    assert.match(await result.text(), /Pull request number must be a positive integer/i);
    await page.getByText('Pull request number must be a positive integer').last().waitFor();
  }
  await page.screenshot({ path: join(output, 'server-error.png'), fullPage: true });
  await pr.fill('0');
  await dir.check();
  assert.equal(await pr.isVisible(), false);
  assert.equal(await pr.isDisabled(), true);
  await repo.check();
  assert.equal(await pr.isEnabled(), true);
  assert.equal(await pr.evaluate(element => element.validity.valid), false);
  await dir.check();
  await page.locator('#base-dir-input').fill(repoDir);
  await page.locator('#workspace-mode').selectOption('CurrentBranch');
  await page.locator('input[name="skipPlanning"]').check();
  await page.screenshot({ path: join(output, 'directory-after-invalid-pr.png'), fullPage: true });
  const valid = await page.locator('#new-session-form').evaluate(form => form.checkValidity());
  await page.getByRole('button', { name: 'Create & plan' }).click();
  assert.equal(valid, true,
    'hidden invalid PR input must not block a directory session');
  await page.waitForURL(/\/sessions\//);
  await page.screenshot({ path: join(output, 'directory-created.png'), fullPage: true });
  assert.deepEqual(errors, [], 'page must not throw');
  console.log(`PR form checks passed; screenshots: ${output}`);
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
