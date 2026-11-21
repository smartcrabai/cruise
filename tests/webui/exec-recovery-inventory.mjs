// Exercise production inventory rendering and the HTMX sidebar refresh.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, rename, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import { loginWebUi, waitForWebUiLogin } from './auth.mjs';

const root = await mkdtemp(join(tmpdir(), 'cruise-exec-inventory-'));
const repo = join(root, 'repo');
const output = resolve('target/formal-reports/exec-recovery-browser');
const ordinary = '20261025000000001';
const exec = '20261025000000002';
const env = {
  PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', CRUISE_UI_LANGUAGE: 'en',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null',
};
async function seed(id, title, isExec) {
  const dir = join(root, 'data/cruise/sessions', id);
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, 'config.yaml'), 'steps:\n  fixture: {command: "false"}\n');
  if (!isExec) await writeFile(join(dir, 'plan.md'), '# Ordinary failure plan\n');
  await writeFile(join(dir, 'state.next'), JSON.stringify({
    id, title, input: 'Local inventory fixture', exec: isExec, base_dir: repo, phase: { Failed: 'fixture command failed' },
    config: { kind: 'inline_snapshot' }, created_at: '2026-10-25T00:00:00Z',
    current_step: 'fixture', workspace_mode: 'CurrentBranch', target_branch: 'main', flags: {},
  }));
  await rename(join(dir, 'state.next'), join(dir, 'state.json'));
}
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => {
  console.error('exec inventory browser watchdog expired', logs);
  server?.kill('SIGKILL');
  process.exit(1);
}, 90000);
try {
  await mkdir(repo);
  await mkdir(output, { recursive: true });
  await seed(ordinary, 'Ordinary failed session', false);
  await seed(exec, 'Retained failed exec', true);
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['daemon', 'serve', '--port', '0', '--webui-dir', resolve('webui')],
    { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', data => { logs += data; });
  const { loginUrl } = await waitForWebUiLogin(server, () => logs);
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  console.log(`Chromium ${browser.version()}`);
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce', colorScheme: 'light' });
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await loginWebUi(page, loginUrl);
  const sidebar = page.locator('#session-list');
  const row = sidebar.locator(`#session-row-${ordinary}`);
  const assertInventory = async () => {
    await row.waitFor({ state: 'visible' });
    assert.match(await row.innerText(), /Ordinary failed session/);
    assert.match(await row.innerText(), /Failed/);
    assert.equal(await sidebar.locator(`#session-row-${exec}`).count(), 0);
    assert.equal(await sidebar.getByText('Retained failed exec', { exact: true }).count(), 0);
    assert.equal(await page.getByRole('button', { name: 'Run all pending sessions', exact: true }).isDisabled(), true);
  };
  await assertInventory();
  await page.screenshot({ path: join(output, 'inventory.png') });

  await row.click();
  const detail = page.locator(`#session-header-${ordinary}`);
  await detail.getByRole('heading', { name: 'Ordinary failed session', exact: true }).waitFor();
  assert.match(await detail.getByRole('alert').innerText(), /fixture command failed/);
  // Wait for the real polling request and observe a distinct DOM change caused by its response.
  const refresh = page.waitForResponse(r => new URL(r.url()).pathname === '/webui/sidebar' &&
    r.request().headers()['hx-request'] === 'true');
  await seed(ordinary, 'Ordinary failed session refreshed', false);
  assert.equal((await refresh).status(), 200);
  await row.getByText('Ordinary failed session refreshed', { exact: true }).waitFor();
  await assertInventory();
  await page.screenshot({ path: join(output, 'refreshed-inventory.png') });
  assert.deepEqual(errors, [], 'production inventory and navigation must not throw');
  console.log('Ordinary failure visible and navigable; retained failed exec absent before and after HTMX refresh.');
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
