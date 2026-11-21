// Real production WebUI ownership boundary with disposable persisted sessions.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import { loginWebUi, waitForWebUiLogin } from './auth.mjs';

const root = await mkdtemp(join(tmpdir(), 'cruise-managed-browser-'));
const repo = join(root, 'repo');
const output = resolve(process.env.WEBUI_MANAGED_REPORT_DIR || 'target/formal-reports/managed-child-browser');
const child = '20261120000000001', ordinary = '20261120000000002';
const sessionDir = id => join(root, 'data/cruise/sessions', id);
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('managed-child browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(repo, { recursive: true });
  await mkdir(output, { recursive: true });
  for (const [id, managed] of [[child, true], [ordinary, false]]) {
    const dir = sessionDir(id);
    await mkdir(dir, { recursive: true });
    const state = { id, base_dir: repo, phase: 'Planned', config: { kind: 'inline_snapshot' },
      input: managed ? 'Owned fixture' : 'Ordinary fixture', title: managed ? 'Owned fixture' : 'Ordinary fixture',
      current_step: null, created_at: '2026-11-20T00:00:00Z', completed_at: null,
      worktree_path: null, worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main', flags: {} };
    if (managed) state.managed_child = { goal_id: 'goal', task_id: 'task', attempt: 1, revision: 1, base_sha: 'base' };
    await writeFile(join(dir, 'state.json'), JSON.stringify(state));
    await writeFile(join(dir, 'config.yaml'), "steps:\n  harmless: {command: 'true'}\n");
    await writeFile(join(dir, 'plan.md'), '# Saved plan\n');
  }
  const before = await readFile(join(sessionDir(child), 'state.json'), 'utf8');
  const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
    XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
    CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
    GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['daemon', 'serve', '--port', '0', '--webui-dir', resolve('webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', data => { logs += data; });
  const { base, loginUrl } = await waitForWebUiLogin(server, () => logs);
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  console.log(`Chromium ${browser.version()}`);
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
  await loginWebUi(page, loginUrl);
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  assert.equal((await page.goto(`${base}/sessions/${child}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  const header = page.locator(`#session-header-${child}`);
  assert.match(await header.innerText(), /Owned fixture \(Manager child, read-only\)/);
  assert.match(await page.locator('aside').innerText(), /Owned fixture \(Manager child, read-only\)/);
  for (const suffix of ['run', 'reset', 'delete', 'discard', 'verify', 'approve', 'cancel', 'editor/tell']) {
    assert.equal(await header.locator(`[hx-post$="/${suffix}"], [hx-get$="/${suffix}"]`).count(), 0, `${suffix} hidden`);
  }
  assert.equal(await header.getByRole('button', { name: 'Settings', exact: true }).count(), 0, 'managed settings hidden');
  const runAll = page.getByRole('button', { name: 'Run all pending sessions' });
  assert.equal(await runAll.isEnabled(), true);
  assert.match(await runAll.getAttribute('hx-confirm'), /Run 1 pending session/);
  await page.screenshot({ path: join(output, 'managed-child.png'), fullPage: true });
  // Exercise authenticated production endpoints from the browser, not a mocked service.
  for (const [action, form] of [['reset', ''], ['delete', ''], ['tell', 'instruction=change']]) {
    const result = await page.evaluate(async ({ url, form }) => {
      const r = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: form });
      return { status: r.status, body: await r.text() };
    }, { url: `${base}/webui/sessions/${child}/${action}`, form });
    assert.equal(result.status, 400, `${action} rejects managed child`);
    assert.match(result.body, /manager|managed/i, `${action} explains ownership`);
    assert.equal(await readFile(join(sessionDir(child), 'state.json'), 'utf8'), before, `${action} preserves child state`);
  }
  assert.equal(await readFile(join(sessionDir(child), 'plan.md'), 'utf8'), '# Saved plan\n');
  assert.equal((await page.goto(`${base}/sessions/${ordinary}`)).status(), 200);
  const ordinaryHeader = page.locator(`#session-header-${ordinary}`);
  assert.doesNotMatch(await ordinaryHeader.innerText(), /Manager child/);
  assert.equal(await ordinaryHeader.getByRole('button', { name: 'Settings', exact: true }).count(), 1, 'ordinary settings remain available');
  assert.equal(await ordinaryHeader.locator(`[hx-post$="/run"]`).count() > 0, true, 'ordinary session remains runnable');
  await page.screenshot({ path: join(output, 'ordinary-session.png'), fullPage: true });
  assert.deepEqual(errors, [], 'no browser exceptions');
  console.log(`Managed-child browser boundary passed. Screenshots: ${output}`);
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
