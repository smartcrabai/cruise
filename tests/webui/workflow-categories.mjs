// Production workflow picker with real HTMX and an isolated HOME/XDG tree.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-categories-browser-'));
const repo = join(root, 'repo');
const config = join(root, 'config/cruise');
const workflows = join(config, 'workflows');
const id = '20261025000000003';
const session = join(root, 'data/cruise/sessions', id);
const output = resolve('target/formal-reports/workflow-categories');
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('categories browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(repo, { recursive: true });
  await mkdir(workflows, { recursive: true });
  await mkdir(session, { recursive: true });
  await mkdir(output, { recursive: true });
  await writeFile(join(session, 'state.json'), JSON.stringify({ id, base_dir: repo, phase: 'Planned', config: { kind: 'inline_snapshot' }, input: 'Category fixture', title: 'Category fixture', current_step: null, created_at: '2026-10-25T00:00:00Z', completed_at: null, worktree_path: null, worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main', flags: {} }));
  await writeFile(join(session, 'config.yaml'), "steps:\n  original: {command: 'true'}\n");
  await writeFile(join(session, 'plan.md'), '# Category fixture\n');
  await writeFile(join(config, 'config.yaml'), 'workflow_categories:\n  Dev:\n    workflows:\n      - refactor\n      - custom-x: desc\nshow_others_category: true\n');
  const refactor = join(workflows, 'refactor.yaml');
  const custom = join(workflows, 'custom-x.yaml');
  await writeFile(refactor, "steps:\n  refactor_fixture: {command: 'true'}\n");
  await writeFile(custom, "steps:\n  custom_fixture: {command: 'true'}\n");
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
  // /new is the full-page route. /webui/new/steps is the HTMX fragment route.
  assert.equal((await page.goto(`${base}/new`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  const groups = page.locator('select#config-path optgroup');
  assert.deepEqual(await groups.evaluateAll(nodes => nodes.map(node => node.label)), ['Dev', 'Others']);
  const dev = groups.nth(0).locator('option');
  const values = await dev.evaluateAll(nodes => nodes.map(node => node.value));
  // A bare name matches both the user file and the built-in refactor workflow.
  assert.deepEqual(values, [refactor, 'builtin:refactor', custom]);
  assert.match(await dev.last().innerText(), /desc/);
  for (const value of ['__builtin__', 'builtin:research', 'builtin:security']) {
    assert.equal(await groups.nth(1).locator(`option[value="${value}"]`).count(), 1, `${value} in Others`);
  }
  const response = page.waitForResponse(r => new URL(r.url()).pathname === '/webui/new/steps');
  await page.locator('#config-path').selectOption(custom);
  assert.equal((await response).status(), 200);
  await page.locator('#step-tree input[name="runSteps"][value="custom_fixture"]').waitFor();
  assert.equal(await page.locator('#step-tree input[name="runSteps"][value="custom_fixture"]').count(), 1);
  await page.screenshot({ path: join(output, 'new-selected.png'), fullPage: true });

  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.locator(`#session-header-${id}`).getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.locator(`#session-settings-${id}`);
  await settings.waitFor();
  const settingGroups = settings.locator('#config-path optgroup');
  assert.deepEqual(await settingGroups.evaluateAll(nodes => nodes.map(node => node.label)), ['Dev', 'Others']);
  assert.equal(await settingGroups.nth(0).locator(`option[value="${custom}"]`).count(), 1);
  const settingsResponse = page.waitForResponse(r => new URL(r.url()).pathname === '/webui/new/steps');
  await settings.locator('#config-path').selectOption(custom);
  assert.equal((await settingsResponse).status(), 200);
  await settings.locator('#step-tree input[name="runSteps"][value="custom_fixture"]').waitFor();
  await page.screenshot({ path: join(output, 'settings-selected.png'), fullPage: true });
  const save = page.waitForResponse(r => new URL(r.url()).pathname === `/webui/sessions/${id}/settings` && r.request().method() === 'POST');
  await settings.getByRole('button', { name: 'Save', exact: true }).click();
  assert.equal((await save).status(), 200);
  await settings.waitFor({ state: 'detached' });
  await page.reload();
  await page.locator(`#session-header-${id}`).getByRole('button', { name: 'Settings', exact: true }).click();
  await settings.waitFor();
  assert.equal(await settings.locator('#config-path').inputValue(), custom, 'selected workflow survives reload');

  await writeFile(join(config, 'config.yaml'), 'workflow_categories:\n  Dev:\n    workflows:\n      - custom-x\nshow_others_category: false\n');
  assert.equal((await page.goto(`${base}/new`)).status(), 200);
  assert.deepEqual(await page.locator('#config-path optgroup').evaluateAll(nodes => nodes.map(node => node.label)), ['Dev']);
  assert.equal(await page.locator('#config-path option[value="builtin:research"]').count(), 0);
  await page.screenshot({ path: join(output, 'new-others-hidden.png'), fullPage: true });
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.locator(`#session-header-${id}`).getByRole('button', { name: 'Settings', exact: true }).click();
  await settings.waitFor();
  assert.deepEqual(await settingGroups.evaluateAll(nodes => nodes.map(node => node.label)), ['Dev']);
  assert.equal(await settings.locator('#config-path option[value="builtin:research"]').count(), 0);
  await page.screenshot({ path: join(output, 'settings-others-hidden.png'), fullPage: true });

  assert.deepEqual(errors, [], 'production workflow picker page must not throw');
  console.log('workflow categories, HTMX step updates and persisted session settings: passed');
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
