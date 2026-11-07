// Real WebUI auto-requeue contracts. Run after building cruise-runtime:
// node tests/webui/auto-requeue.mjs [settings|validation|header|runall|all]
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const scenario = process.argv[2] || 'all';
assert(['settings', 'validation', 'header', 'runall', 'all'].includes(scenario));
const root = await mkdtemp(join(tmpdir(), 'cruise-auto-requeue-browser-'));
const repo = join(root, 'repo');
const id = '20261025000000001';
const dir = join(root, 'data/cruise/sessions', id);
const marker = join(root, 'provider-attempts');
const reportDir = resolve(process.env.WEBUI_AUTO_REQUEUE_REPORT_DIR || 'target/formal-reports/auto-requeue-browser');
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('auto-requeue browser timed out', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(repo, { recursive: true });
  await mkdir(dir, { recursive: true });
  await mkdir(reportDir, { recursive: true });
  const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
    XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
    CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
    GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
  for (const args of [['init', '-b', 'main'], ['-c', 'user.name=Browser Test', '-c', 'user.email=browser@example.invalid', 'commit', '--allow-empty', '-m', 'fixture']]) {
    const result = spawnSync('git', args, { cwd: repo, env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
  }
  const state = { id, base_dir: repo, phase: 'Planned', config: { kind: 'inline_snapshot' },
    input: 'Auto-requeue browser fixture', title: 'Auto-requeue fixture', current_step: null,
    created_at: '2026-10-25T00:00:00Z', completed_at: null, worktree_path: null,
    worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main', allow_dirty_working_tree: true };
  // A scripted provider fails its first invocation, then succeeds. Its marker
  // is outside the repository so retry detection never dirties the worktree.
  const provider = join(root, 'provider.sh');
  await writeFile(provider, `#!/bin/sh\nprintf 'attempt\\n' >> '${marker}'\nif [ "$(wc -l < '${marker}')" -eq 1 ]; then echo 'provider failed' >&2; exit 1; fi\ncat >/dev/null\nprintf 'completed\\n'\n`, { mode: 0o755 });
  await writeFile(join(dir, 'state.json'), JSON.stringify(state));
  await writeFile(join(dir, 'config.yaml'), `command: [${JSON.stringify(provider)}]\ncreate_pr: false\nsteps:\n  work:\n    prompt: Perform the fixture task\n`);
  await writeFile(join(dir, 'plan.md'), '# Fixture plan\n');
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
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  assert.equal(await page.locator(`#session-header-${id}`).getByText(/Auto-requeued/).count(), 0, 'zero counter is hidden');
  const openSettings = async () => {
    await page.locator('#sidebar [aria-label="Settings"]').click();
    await page.getByRole('dialog').waitFor();
  };
  const input = () => page.locator('[name="runAllAutoRequeueMaxAttempts"]');
  if (scenario === 'settings' || scenario === 'all' || scenario === 'runall') {
    await openSettings();
    assert.equal(await input().getAttribute('min'), '0');
    assert.equal(await input().inputValue(), '0', 'fresh settings default to disabled');
    await page.getByRole('dialog').screenshot({ path: join(reportDir, 'settings-zero.png') });
    await input().fill('1');
    const saved = page.waitForResponse(r => r.url().endsWith('/webui/settings') && r.request().method() === 'POST');
    await page.getByRole('dialog').getByRole('button', { name: 'Save', exact: true }).click();
    assert.equal((await saved).status(), 200);
    await page.getByRole('dialog').waitFor({ state: 'detached' });
    await openSettings();
    assert.equal(await input().inputValue(), '1', 'saved value must survive modal reload');
    await page.getByRole('dialog').screenshot({ path: join(reportDir, 'settings-saved.png') });
    await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  }
  if (['settings', 'validation', 'all', 'runall'].includes(scenario)) {
    // Exercise server validation directly, bypassing native number-input validation.
    for (const [value, expected] of [['0', 200], ['-1', 400], ['abc', 400]]) {
      const response = await page.request.post(`${base}/webui/settings`, { form: { runAllParallelism: '1', runAllAutoRequeueMaxAttempts: value } });
      if (expected === 200) assert.equal(response.status(), 200, `accept ${value}`);
      else assert(response.status() >= 400 && response.status() < 500, `reject ${value}: ${response.status()}`);
    }
    // Restore the enabled budget for the live Run All scenario.
    assert.equal((await page.request.post(`${base}/webui/settings`, { form: { runAllParallelism: '1', runAllAutoRequeueMaxAttempts: '1' } })).status(), 200);
    console.log('settings save/reopen and validation passed');
  }
  if (scenario === 'header') {
    state.auto_requeue_count = 2;
    await writeFile(join(dir, 'state.json'), JSON.stringify(state));
    await page.reload();
    await page.getByText('Auto-requeued 2', { exact: true }).first().waitFor();
    await page.getByRole('tab', { name: 'Info', exact: true }).click();
    const info = page.locator(`#tab-info-${id}`);
    await info.getByText('Auto-requeued', { exact: true }).waitFor();
    assert.equal(await info.locator('dd').filter({ hasText: /^2$/ }).count(), 1);
    await page.screenshot({ path: join(reportDir, 'counter-two.png') });
    console.log('session counter display passed');
  }
  if (scenario === 'runall' || scenario === 'all') {
    page.once('dialog', dialog => dialog.accept());
    await page.locator('#run-all-control button').click();
    await page.locator('#run-all-view').waitFor();
    await page.locator('#run-all-log').getByText(/Auto-requeued.*1\/1/).waitFor({ timeout: 30000 });
    await page.locator('#run-all-results').getByText('Completed', { exact: true }).waitFor({ timeout: 30000 });
    await page.screenshot({ path: join(reportDir, 'runall-retried.png') });
    assert.equal((await readFile(marker, 'utf8')).trim().split('\n').length, 2, 'one failed attempt and one retry');
    assert.equal(JSON.parse(await readFile(join(dir, 'state.json'), 'utf8')).auto_requeue_count, 1);
    await page.goto(`${base}/sessions/${id}`);
    await page.getByText('Auto-requeued 1', { exact: true }).first().waitFor();
    console.log('scripted-provider batch retry announcement and header passed');
  }
  assert.deepEqual(errors, [], 'production page must not throw');
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
