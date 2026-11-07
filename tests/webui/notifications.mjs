// Real browser, production WebUI and workflow execution, and a local HTTP webhook receiver.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-notifications-e2e-'));
const repo = join(root, 'repo');
const config = join(root, 'config/cruise/config.json');
const requests = [];
const receiver = createServer((req, res) => {
  let body = '';
  req.setEncoding('utf8');
  req.on('data', chunk => { body += chunk; });
  req.on('end', () => { requests.push({ method: req.method, body }); res.writeHead(200); res.end('ok'); });
});
await new Promise(ok => receiver.listen(0, '127.0.0.1', ok));
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_HERDR: '1', CRUISE_NOTIFY_WEBHOOK: `http://127.0.0.1:${receiver.address().port}/hook`,
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', NO_COLOR: '1' };
let server, browser, logs = '';
const watchdog = setTimeout(() => { console.error('notification browser timed out', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(repo, { recursive: true });
  await mkdir(join(root, 'config/cruise'), { recursive: true });
  for (const args of [['init', '-b', 'main'], ['-c', 'user.name=E2E', '-c', 'user.email=e2e@example.invalid', 'commit', '--allow-empty', '-m', 'fixture']]) {
    const result = spawnSync('git', args, { cwd: repo, env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
  }
  const ids = ['20261025000000001', '20261025000000002', '20261025000000003', '20261025000000004', '20261025000000005', '20261025000000006', '20261025000000007'];
  for (const [index, id] of ids.entries()) {
    const dir = join(root, 'data/cruise/sessions', id);
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, 'state.json'), JSON.stringify({ id, base_dir: repo, phase: 'Planned',
      config: { kind: 'inline_snapshot' }, input: `notification-case-${index}`, title: `notification-case-${index}`,
      current_step: null, created_at: '2026-10-25T00:00:00Z', completed_at: null,
      worktree_path: null, worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main' }));
    const workflow = index >= 5
      ? 'create_pr: false\nsteps:\n  execute:\n    command: "true"\n    if:\n      no-file-changes: failed\n'
      : index >= 3
      ? 'create_pr: false\nsteps:\n  choose:\n    plan: "{plan}"\n    option:\n      - selector: "Continue notification test"\n        next: done\n  done:\n    command: "true"\n'
      : 'create_pr: false\nsteps:\n  execute:\n    command: "true"\n';
    await writeFile(join(dir, 'config.yaml'), workflow);
    await writeFile(join(dir, 'plan.md'), '# Notification fixture\n');
  }
  await writeFile(config, JSON.stringify({ notifications: { desktop: true, sound: false, webhook: true,
    events: { completed: { desktop: false, webhook: false } } } }));
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['webui', '--port', '0', '--webui-dir', resolve('webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  server.stderr.on('data', data => { logs += data; });
  const closed = new Promise(ok => server.once('close', ok));
  const base = await new Promise((ok, fail) => {
    let output = '';
    const timeout = setTimeout(() => fail(new Error(`server startup timeout: ${logs}`)), 20000);
    server.once('error', fail);
    server.once('exit', code => fail(new Error(`server exited ${code}: ${logs}`)));
    server.stdout.on('data', data => { output += data; const match = output.match(/http:\/\/127\.0\.0\.1:\d+/); if (match) { clearTimeout(timeout); ok(match[0]); } });
  });
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  console.log(`Chromium ${browser.version()}`);
  const page = await browser.newPage();
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  async function connect(id) {
    assert.equal((await page.goto(`${base}/sessions/${id}`, { timeout: 20000 })).status(), 200);
    await page.waitForFunction(() => !!window.app);
    await page.evaluate(async () => {
      window.testNotifications = [];
      const stream = new EventSource('/webui/events');
      window.testStream = stream;
      stream.addEventListener('notify', event => window.testNotifications.push(JSON.parse(event.data)));
      await Promise.race([new Promise((ok, fail) => { stream.onopen = ok; stream.onerror = fail; }),
        new Promise((_, fail) => setTimeout(() => fail(new Error('SSE connection timeout')), 20000))]);
    });
  }
  async function run(id, enabled) {
    const expected = 'Completed';
    console.log(`running ${id}, expecting ${expected}`);
    await connect(id);
    await page.getByRole('button', { name: 'Run on current branch', exact: true }).click();
    const path = join(root, 'data/cruise/sessions', id, 'state.json');
    const until = Date.now() + 15000;
    while (true) {
      const state = JSON.parse(await readFile(path, 'utf8'));
      if (state.phase === expected) break;
      assert(Date.now() < until, `expected ${expected}: ${JSON.stringify(state)}\n${logs}`);
      await new Promise(ok => setTimeout(ok, 50));
    }
    await page.getByText(expected, { exact: true }).first().waitFor();
    await page.locator('#toasts').getByText(expected, { exact: false }).first().waitFor();
    await mkdir(resolve('target/formal-reports/notification-browser'), { recursive: true });
    await page.screenshot({ path: resolve(`target/formal-reports/notification-browser/${id}.png`) });
    if (enabled) {
      await page.waitForFunction(() => window.testNotifications.length === 1);
      assert.match((await page.evaluate(() => window.testNotifications[0].body)), /completed/i);
    } else {
      // Bound the negative observation after the terminal toast was received.
      await page.waitForTimeout(500);
      assert.deepEqual(await page.evaluate(() => window.testNotifications), [], 'disabled completed desktop event emits no notify SSE');
    }
    await page.evaluate(() => window.testStream.close());
  }
  async function option(id, enabled) {
    await connect(id);
    await page.getByRole('button', { name: 'Run on current branch', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('button', { name: 'Continue notification test', exact: true }).waitFor();
    await page.locator('#toasts').getByText('Action required', { exact: false }).waitFor();
    await page.screenshot({ path: resolve(`target/formal-reports/notification-browser/${id}.png`) });
    if (enabled) {
      await page.waitForFunction(() => window.testNotifications.length === 1);
      assert.match(await page.evaluate(() => window.testNotifications[0].body), /action required/i);
    } else {
      await page.waitForTimeout(500);
      assert.deepEqual(await page.evaluate(() => window.testNotifications), [], 'disabled action-required desktop event emits no notify SSE');
    }
    const response = page.waitForResponse(response => response.url().endsWith(`/webui/sessions/${id}/option`) && response.request().method() === 'POST');
    await dialog.getByRole('button', { name: 'Continue notification test', exact: true }).click();
    assert.equal((await response).status(), 200);
    await dialog.waitFor({ state: 'detached' });
    await page.getByText('Completed', { exact: true }).first().waitFor();
    await page.evaluate(() => window.testStream.close());
  }
  async function failure(id, enabled) {
    await connect(id);
    await page.getByRole('button', { name: 'Run on current branch', exact: true }).click();
    await page.locator('#toasts').getByText('Failed', { exact: true }).waitFor();
    await page.screenshot({ path: resolve(`target/formal-reports/notification-browser/${id}.png`) });
    if (enabled) {
      await page.waitForFunction(() => window.testNotifications.length === 1);
      assert.match(await page.evaluate(() => window.testNotifications[0].body), /failed/i);
    } else {
      await page.waitForTimeout(500);
      assert.deepEqual(await page.evaluate(() => window.testNotifications), [], 'disabled failure desktop event emits no notify SSE');
    }
    await page.evaluate(() => window.testStream.close());
  }
  // Exercise event override off, on, then master off with override on.
  await run(ids[0], false);
  await writeFile(config, JSON.stringify({ notifications: { desktop: true, sound: false, webhook: true,
    events: { completed: { desktop: true, webhook: true } } } }));
  await run(ids[2], true);
  const deadline = Date.now() + 10000;
  while (requests.length < 1 && Date.now() < deadline) await new Promise(ok => setTimeout(ok, 50));
  assert.equal(requests.length, 1, `only enabled completed event should be delivered: ${JSON.stringify(requests)}`);
  assert.equal(requests[0].method, 'POST');
  assert.match(JSON.parse(requests[0].body).text, /completed/i);
  // A master-off setting must suppress even an explicit event opt-in.
  await writeFile(config, JSON.stringify({ notifications: { desktop: false, sound: false, webhook: false,
    events: { completed: { desktop: true, webhook: true } } } }));
  await run(ids[1], false);
  assert.equal(requests.length, 1, `master-off suppresses override: ${JSON.stringify(requests)}`);
  await writeFile(config, JSON.stringify({ notifications: { desktop: true, sound: false, webhook: true,
    events: { action_required: { desktop: false, webhook: false }, completed: { desktop: false, webhook: false } } } }));
  await option(ids[3], false);
  assert.equal(requests.length, 1, `disabled action-required produces no webhook: ${JSON.stringify(requests)}`);
  await writeFile(config, JSON.stringify({ notifications: { desktop: true, sound: false, webhook: true,
    events: { action_required: { desktop: true, webhook: true }, completed: { desktop: false, webhook: false } } } }));
  await option(ids[4], true);
  const actionDeadline = Date.now() + 10000;
  while (requests.length < 2 && Date.now() < actionDeadline) await new Promise(ok => setTimeout(ok, 50));
  assert.equal(requests.length, 2, `only enabled action-required adds a webhook: ${JSON.stringify(requests)}`);
  assert.match(JSON.parse(requests[1].body).text, /action required/i);
  await writeFile(config, JSON.stringify({ notifications: { desktop: true, sound: false, webhook: true,
    events: { failed: { desktop: false, webhook: false }, completed: { desktop: false, webhook: false } } } }));
  await failure(ids[5], false);
  assert.equal(requests.length, 2, `disabled failure produces no webhook: ${JSON.stringify(requests)}`);
  await writeFile(config, JSON.stringify({ notifications: { desktop: true, sound: false, webhook: true,
    events: { failed: { desktop: true, webhook: true }, completed: { desktop: false, webhook: false } } } }));
  await failure(ids[6], true);
  const failureDeadline = Date.now() + 10000;
  while (requests.length < 3 && Date.now() < failureDeadline) await new Promise(ok => setTimeout(ok, 50));
  assert.equal(requests.length, 3, `only enabled failure adds a webhook: ${JSON.stringify(requests)}`);
  assert.match(JSON.parse(requests[2].body).text, /failed/i);
  assert.deepEqual(errors, []);
  console.log('production browser per-event webhook notification coverage passed');
  await browser.close(); browser = null;
  server.kill('SIGTERM'); await closed; server = null;
} finally {
  clearTimeout(watchdog);
  if (browser) await browser.close();
  if (server) server.kill('SIGKILL');
  await new Promise(ok => receiver.close(ok));
  await rm(root, { recursive: true, force: true });
}
