// Exercise production Diff HTMX swaps against a real, isolated Git repository.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-diff-browser-'));
const repo = join(root, 'repo'), worktree = join(root, 'worktree');
const id = '20261025000000004', session = join(root, 'data/cruise/sessions', id);
const output = resolve(process.env.WEBUI_DIFF_REPORT_DIR || 'target/formal-reports/diff-browser');
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('diff browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(repo, { recursive: true });
  await mkdir(session, { recursive: true });
  await mkdir(output, { recursive: true });
  for (const args of [
    ['init', '-b', 'main'],
    ['-c', 'user.name=Browser Fixture', '-c', 'user.email=browser@example.invalid', 'commit', '--allow-empty', '-m', 'fixture base'],
    ['worktree', 'add', '-b', 'diff-fixture', worktree],
  ]) {
    const result = spawnSync('git', args, { cwd: repo, env, encoding: 'utf8' });
    assert.equal(result.status, 0, `isolated git fixture: ${result.stderr}`);
  }
  const state = { id, base_dir: repo, phase: 'Planned', config: { kind: 'inline_snapshot' },
    input: 'Diff browser fixture', title: 'Diff browser fixture', current_step: null,
    created_at: '2026-10-25T00:00:00Z', completed_at: null, worktree_path: worktree,
    worktree_branch: 'diff-fixture', target_branch: 'main' };
  const save = () => writeFile(join(session, 'state.json'), JSON.stringify(state));
  await save();
  await writeFile(join(session, 'config.yaml'), "steps:\n  first: {command: 'true'}\n");
  await writeFile(join(session, 'plan.md'), '# Diff fixture\n');
  const fixture = join(worktree, 'diff-fixture.txt');
  await writeFile(fixture, 'initial diff fixture line\n<script>window.diffFixtureExecuted = true</script>\n');
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
  console.log(`Chromium ${browser.version()}`);
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  await page.locator('[role="tab"][hx-get][data-htmx-powered="true"]').first().waitFor();
  const panel = page.locator(`#tab-diff-${id}`);
  const responseForDiff = () => page.waitForResponse(r => r.url().endsWith(`/webui/sessions/${id}/tab/diff`));
  let response = responseForDiff();
  await page.getByRole('tab', { name: 'Diff', exact: true }).click();
  assert.equal((await response).status(), 200);
  await panel.getByText('+initial diff fixture line', { exact: true }).waitFor();
  assert.match(await panel.innerText(), /diff --git/);
  const scriptLine = panel.getByText('+<script>window.diffFixtureExecuted = true</script>', { exact: true });
  await scriptLine.waitFor();
  assert(await scriptLine.evaluate(el => el.classList.contains('text-green-600')), 'added line uses addition color');
  assert.equal(await panel.locator('script').count(), 0, 'patch markup must remain text');
  assert.equal(await page.evaluate(() => window.diffFixtureExecuted), undefined, 'patch script must not execute');
  await page.screenshot({ path: join(output, 'changed.png'), fullPage: true });

  assert.equal((await page.goto(`${base}/sessions/${id}?tab=diff`)).status(), 200);
  await panel.getByText('+initial diff fixture line', { exact: true }).waitFor();
  assert.equal(await page.getByRole('tab', { name: 'Diff', exact: true }).getAttribute('aria-selected'), 'true');
  assert.equal((await page.reload()).status(), 200);
  await panel.getByText('+initial diff fixture line', { exact: true }).waitFor();
  assert.equal(await page.getByRole('tab', { name: 'Diff', exact: true }).getAttribute('aria-selected'), 'true');
  await page.screenshot({ path: join(output, 'direct-reload.png'), fullPage: true });

  async function refresh(expected, name) {
    const previous = await panel.elementHandle();
    const response = responseForDiff();
    await panel.getByRole('button', { name: 'Refresh', exact: true }).click();
    assert.equal((await response).status(), 200, `${name} response`);
    await previous.waitForElementState('hidden');
    await panel.getByText(expected, { exact: false }).waitFor();
    await page.screenshot({ path: join(output, `${name}.png`), fullPage: true });
  }
  await writeFile(fixture, 'refreshed diff fixture line\n');
  await refresh('+refreshed diff fixture line', 'refresh');
  assert.doesNotMatch(await panel.innerText(), /initial diff fixture line/);
  await rm(fixture);
  await refresh('No changes on diff-fixture', 'empty');
  assert.doesNotMatch(await panel.innerText(), /refreshed diff fixture line/);
  state.worktree_branch = null;
  await save();
  await refresh('No session branch available for diff', 'unavailable');
  state.worktree_branch = 'missing-fixture-branch';
  state.worktree_path = null;
  await save();
  await refresh('branch missing-fixture-branch no longer exists', 'error');
  assert.doesNotMatch(await panel.innerText(), /No session branch available/);
  assert.deepEqual(errors, [], 'production Diff page must not throw');
  console.log(`Diff changed, refresh, empty, unavailable and error passed; screenshots: ${output}`);
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
