// Secret-free Verify visibility boundaries against the production server and browser.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-verify-boundary-'));
const repo = join(root, 'repo');
const output = resolve(process.env.WEBUI_VERIFY_BOUNDARY_REPORT_DIR || 'target/formal-reports/verify-boundary-browser');
const cases = [
  { id: '20261107000000011', phase: 'AwaitingApproval', plan: true, visible: true, name: 'approval' },
  { id: '20261107000000012', phase: 'AwaitingApproval', plan: false, visible: false, name: 'missing-plan' },
  { id: '20261107000000013', phase: 'Draft', plan: true, visible: false, name: 'draft' },
  { id: '20261107000000014', phase: 'Planned', plan: true, visible: false, name: 'planned' },
];
const plan = '# Saved plan\n\nKeep this plan unchanged.\n';
let server, closed, browser, logs = '';
try {
  await mkdir(repo, { recursive: true });
  await mkdir(output, { recursive: true });
  await mkdir(join(root, 'empty-bin'));
  for (const item of cases) {
    const dir = join(root, 'data/cruise/sessions', item.id);
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, 'state.json'), JSON.stringify({ id: item.id, base_dir: repo, phase: item.phase,
      config: { kind: 'inline_snapshot' }, input: 'Verify boundary', title: `Verify ${item.name}`,
      current_step: null, created_at: '2026-11-07T00:00:00Z', completed_at: null,
      worktree_path: null, worktree_branch: null }));
    await writeFile(join(dir, 'config.yaml'), 'sdk: claude\nsteps:\n  harmless:\n    command: "true"\n');
    if (item.plan) await writeFile(join(dir, 'plan.md'), plan);
  }
  // No credentials or Claude executable are available to the production runtime.
  const env = { PATH: join(root, 'empty-bin'), HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
    XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
    CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
    GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['webui', '--port', '0', '--webui-dir', resolve('webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', chunk => { logs += chunk; });
  const base = await new Promise((ok, fail) => {
    let stdout = '';
    const timer = setTimeout(() => fail(new Error(`startup timeout: ${logs}`)), 20000);
    server.once('error', error => { clearTimeout(timer); fail(error); });
    server.once('exit', code => { clearTimeout(timer); fail(new Error(`server exited ${code}: ${logs}`)); });
    server.stdout.on('data', chunk => { stdout += chunk; const match = stdout.match(/http:\/\/127\.0\.0\.1:\d+/); if (match) { clearTimeout(timer); ok(match[0]); } });
  });
  const browserEnv = { ...env };
  browser = await chromium.launch({ headless: true, env: browserEnv });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
  page.setDefaultTimeout(20000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  let verifyPosts = 0;
  page.on('request', request => { if (request.method() === 'POST' && request.url().endsWith('/verify')) verifyPosts++; });
  for (const item of cases) {
    assert.equal((await page.goto(`${base}/sessions/${item.id}`)).status(), 200);
    await page.waitForFunction(() => !!window.app);
    const button = page.getByRole('button', { name: 'Verify', exact: true });
    assert.equal(await button.count(), item.visible ? 1 : 0, `${item.name}: Verify visibility`);
    if (item.visible) {
      assert.equal(await button.isEnabled(), true);
      assert.equal(await button.getAttribute('hx-post'), `/webui/sessions/${item.id}/verify`);
    }
    await page.screenshot({ path: join(output, `${item.name}.png`), fullPage: true });
    if (item.plan) assert.equal(await readFile(join(root, 'data/cruise/sessions', item.id, 'plan.md'), 'utf8'), plan);
  }
  await page.setViewportSize({ width: 375, height: 800 });
  await page.goto(`${base}/sessions/${cases[0].id}`);
  const mobileButton = page.getByRole('button', { name: 'Verify', exact: true });
  await mobileButton.waitFor({ state: 'visible' });
  await page.screenshot({ path: join(output, 'approval-mobile.png'), fullPage: true });
  const post = page.waitForResponse(r => r.request().method() === 'POST' && r.url() === `${base}/webui/sessions/${cases[0].id}/verify`);
  await mobileButton.click();
  assert.equal((await post).status(), 200, 'Verify reaches production HTTP route');
  const errorToast = page.getByTestId('toast-detail');
  await errorToast.waitFor({ timeout: 60000 });
  assert.equal(await errorToast.textContent(), 'claude is not installed: no managed install and no claude on PATH. Run `cruise …');
  await page.screenshot({ path: join(output, 'provider-startup-failure.png'), fullPage: true });
  assert.equal(verifyPosts, 1, 'exactly one real Verify POST');
  assert.equal(await readFile(join(root, 'data/cruise/sessions', cases[0].id, 'plan.md'), 'utf8'), plan, 'failed Verify preserves the saved plan');
  assert.deepEqual(errors, [], 'no browser exceptions');
  console.log(`Verify browser boundaries passed. Screenshots: ${output}`);
} catch (error) {
  console.error(error, '\nProduction server stderr:', logs);
  process.exitCode = 1;
} finally {
  await browser?.close();
  if (server) { server.kill('SIGKILL'); await closed; }
  await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
