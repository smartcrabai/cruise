// Real browser contract for explicit exec and force_exec using disposable local workflows.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import { loginWebUi, waitForWebUiLogin } from './auth.mjs';

const root = await mkdtemp(join(tmpdir(), 'cruise-exec-browser-'));
const repo = join(root, 'repo');
const workflows = join(root, 'config/cruise/workflows');
const output = resolve(process.env.WEBUI_EXEC_REPORT_DIR || 'target/formal-reports/exec-browser');
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'), XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'), CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', CRUISE_UI_LANGUAGE: 'en', NO_COLOR: '1', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('exec browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(repo, { recursive: true });
  await mkdir(workflows, { recursive: true });
  await mkdir(output, { recursive: true });
  for (const args of [['init', '-b', 'main'], ['config', 'user.name', 'Exec E2E'], ['config', 'user.email', 'exec@example.invalid'], ['commit', '--allow-empty', '-m', 'fixture']]) {
    const result = spawnSync('git', args, { cwd: repo, env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
  }
  const normal = join(workflows, 'browser-normal.yaml');
  const forced = join(workflows, 'browser-forced.yaml');
  await writeFile(normal, "command: [cat]\nsteps:\n  marker:\n    command: 'printf normal > normal-marker'\n");
  await writeFile(forced, "command: [cat]\nforce_exec: true\nsteps:\n  marker:\n    command: 'printf forced > forced-marker'\n");
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'), ['daemon', 'serve', '--port', '0', '--webui-dir', resolve('webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', data => { logs += data; });
  const { base, loginUrl } = await waitForWebUiLogin(server, () => logs);
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  console.log(`Chromium ${browser.version()}`);
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await loginWebUi(page, loginUrl);
  async function setup(config) {
    assert.equal((await page.goto(`${base}/new`)).status(), 200);
    await page.locator('#base-dir-input').fill(repo);
    await page.locator('#base-dir-input').dispatchEvent('change');
    await page.locator(`#config-path option[value="${config}"]`).waitFor({ state: 'attached' });
    await page.locator('#config-path').selectOption(config);
    await page.locator('#task-input').fill('Run deterministic local workflow');
  }
  async function marker(name, expected) {
    const deadline = Date.now() + 20000;
    while (true) {
      try { assert.equal(await readFile(join(repo, name), 'utf8'), expected); break; }
      catch (error) { if (Date.now() >= deadline) throw error; await new Promise(ok => setTimeout(ok, 50)); }
    }
  }
  await setup(normal);
  await page.getByRole('button', { name: 'Exec', exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Create & plan', exact: true }).count(), 1);
  await page.screenshot({ path: join(output, 'explicit-exec.png'), fullPage: true });
  await page.locator('#task-input').fill('#12');
  const rejected = page.waitForResponse(r => r.url().endsWith('/webui/sessions/exec') && r.request().method() === 'POST');
  await page.getByRole('button', { name: 'Exec', exact: true }).click();
  assert.equal((await rejected).status(), 400, 'issue-only input is rejected');
  await page.getByText('exec does not resolve #N issue input', { exact: false }).waitFor();
  await page.screenshot({ path: join(output, 'rejected-exec.png'), fullPage: true });
  await page.locator('#task-input').fill('Run deterministic local workflow');
  const explicitResponse = page.waitForResponse(r => r.url().endsWith('/webui/sessions/exec') && r.request().method() === 'POST');
  await page.getByRole('button', { name: 'Exec', exact: true }).click();
  assert.equal((await explicitResponse).status(), 200);
  await marker('normal-marker', 'normal');
  await setup(forced);
  await page.getByRole('button', { name: 'Execute', exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Exec', exact: true }).count(), 0);
  assert.match(await page.locator('#new-session-actions').innerText(), /without planning, a worktree, or a PR/);
  await page.screenshot({ path: join(output, 'forced-exec.png'), fullPage: true });
  const forcedResponse = page.waitForResponse(r => r.url().endsWith('/webui/sessions') && r.request().method() === 'POST');
  await page.getByRole('button', { name: 'Execute', exact: true }).click();
  assert.equal((await forcedResponse).status(), 200);
  await marker('forced-marker', 'forced');
  await page.screenshot({ path: join(output, 'executed.png'), fullPage: true });
  const git = spawnSync('git', ['worktree', 'list', '--porcelain'], { cwd: repo, env, encoding: 'utf8' });
  assert.equal(git.status, 0, git.stderr);
  assert.equal((git.stdout.match(/^worktree /gm) || []).length, 1, 'exec does not create worktrees');
  assert.deepEqual(errors, [], 'no browser JavaScript errors');
  console.log('WEBUI_EXEC_BROWSER_PASSED');
} catch (error) {
  console.error(error, '\nProduction server stderr:', logs);
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
  try { await browser?.close(); } finally {
    try { if (server) { server.kill('SIGKILL'); await closed; } } finally { await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); }
  }
}
