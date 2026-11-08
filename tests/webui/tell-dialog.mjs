// B-1/B-2: exercise Tell against a real running WebUI and a Planned session.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-tell-browser-'));
const repo = join(root, 'repo');
const id = '20261025000000003';
const session = join(root, 'data/cruise/sessions', id);
const release = join(repo, 'release');
const screenshots = resolve(process.env.WEBUI_TELL_REPORT_DIR || 'target/formal-reports/tell-browser');
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('tell browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(repo, { recursive: true });
  await mkdir(screenshots, { recursive: true });
  await mkdir(session, { recursive: true });
  for (const args of [['init', '-b', 'main'], ['config', 'user.name', 'Tell BDD'],
    ['config', 'user.email', 'tell-bdd@example.invalid'], ['commit', '--allow-empty', '-m', 'fixture']]) {
    const result = spawnSync('git', args, { cwd: repo, env, encoding: 'utf8' });
    assert.equal(result.status, 0, `git fixture: ${result.stderr}`);
  }
  await writeFile(join(session, 'state.json'), JSON.stringify({
    id, base_dir: repo, phase: 'Planned', config: { kind: 'inline_snapshot' },
    input: 'Tell browser fixture', title: 'Tell browser fixture', current_step: null,
    created_at: '2026-10-25T00:00:00Z', completed_at: null, worktree_path: null,
    worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main',
  }));
  // Hold a real command step open until cleanup, keeping the in-process run alive.
  await writeFile(join(session, 'config.yaml'), `create_pr: false\nsteps:\n  hold:\n    command: 'while [ ! -f release ]; do sleep 0.1; done'\n`);
  await writeFile(join(session, 'plan.md'), '# Tell browser fixture\n');
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['webui', '--port', '0', '--webui-dir', resolve(process.env.WEBUI_BDD_ASSETS || 'webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', data => { logs += data; });
  const base = await new Promise((ok, fail) => {
    let output = '';
    const timeout = setTimeout(() => fail(new Error(`server startup timeout: ${logs}`)), 20000);
    server.once('error', error => { clearTimeout(timeout); fail(error); });
    server.once('exit', code => { clearTimeout(timeout); fail(new Error(`server exited ${code}: ${logs}`)); });
    server.stdout.on('data', data => {
      output += data;
      const match = output.match(/http:\/\/127\.0\.0\.1:\d+/);
      if (match) { clearTimeout(timeout); ok(match[0]); }
    });
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
  const header = page.locator(`#session-header-${id}`);
  const tell = header.getByRole('button', { name: 'Tell', exact: true });
  const run = header.getByRole('button', { name: 'Run on current branch', exact: true });
  await run.waitFor();
  assert.equal(await tell.count(), 0, 'B-2: Planned session must not offer Tell');
  await page.screenshot({ path: join(screenshots, 'planned.png') });
  await run.click();
  const runningDeadline = Date.now() + 15000;
  while (JSON.parse(await readFile(join(session, 'state.json'), 'utf8')).phase !== 'Running') {
    assert(Date.now() < runningDeadline, 'real workflow did not reach Running');
    await new Promise(ok => setTimeout(ok, 50));
  }
  await tell.waitFor();
  const editorResponse = page.waitForResponse(r => r.url().endsWith(`/webui/sessions/${id}/editor/tell`) && r.request().method() === 'GET');
  await tell.click();
  assert.equal((await editorResponse).status(), 200, 'Tell editor request succeeds');
  const editor = page.locator(`#editor-${id}`);
  const instruction = editor.locator('textarea[name="instruction"]');
  await instruction.waitFor();
  await instruction.fill('   ');
  const rejected = page.waitForResponse(r => r.url().endsWith(`/webui/sessions/${id}/tell`) && r.request().method() === 'POST');
  await editor.getByRole('button', { name: 'Send', exact: true }).click();
  assert.equal((await rejected).status(), 400, 'blank instruction must be rejected');
  await page.getByText(/instruction.*empty/i).first().waitFor();
  await page.screenshot({ path: join(screenshots, 'tell-rejected.png') });
  const text = 'Please inspect the browser Tell acceptance path.';
  await instruction.fill(text);
  await page.screenshot({ path: join(screenshots, 'tell-editor.png') });
  // Record actual out-of-band header replacement, not merely a successful POST.
  await page.evaluate(id => { window.tellOriginalHeader = document.getElementById(`session-header-${id}`); }, id);
  const submitted = page.waitForResponse(r => r.url().endsWith(`/webui/sessions/${id}/tell`) && r.request().method() === 'POST');
  await editor.getByRole('button', { name: 'Send', exact: true }).click();
  const response = await submitted;
  assert.equal(response.status(), 200, 'B-1: Tell is accepted');
  assert((await response.text()).includes(`session-header-${id}`), 'acceptance response contains the updated header');
  await page.waitForFunction(id => document.getElementById(`session-header-${id}`) !== window.tellOriginalHeader, id);
  await instruction.waitFor({ state: 'detached' });
  const ledger = await readFile(join(session, 'instructions.jsonl'), 'utf8');
  assert(ledger.includes(text), 'accepted instruction is persisted in the real ledger');
  await page.screenshot({ path: join(screenshots, 'tell-submitted.png') });
  await page.getByRole('tab', { name: 'Info', exact: true }).click();
  const pending = page.locator(`#tab-info-${id} dl > div`).filter({ hasText: 'Pending instructions' });
  await pending.locator('dd').waitFor();
  assert.equal((await pending.locator('dd').innerText()).trim(), '1', 'Info displays the pending instruction count');
  await page.screenshot({ path: join(screenshots, 'tell-info.png') });
  console.log(`Screenshots: ${screenshots}`);
  assert.deepEqual(errors, [], 'production browser JavaScript must not throw');
  console.log('WEBUI_BROWSER_CONTRACT_PASSED tell B-1 B-2');
} catch (error) {
  console.error(error, '\nProduction server stderr:', logs);
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
  try {
    await writeFile(release, '').catch(() => {});
    await browser?.close();
  } finally {
    try {
      if (server) { server.kill('SIGKILL'); await closed; }
    } finally {
      await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}
