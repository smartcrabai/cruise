// Real option-step lifecycle: persisted session -> production Run -> SSE dialog -> answer -> shell effect.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const args = process.argv.slice(2);
if (args[0] === 'option') args.shift();
const [rawWidth = '360', rawHeight = '640'] = args;
const width = Number(rawWidth), height = Number(rawHeight);
assert(width >= 320 && height >= 320, 'viewport dimensions required');
const root = await mkdtemp(join(tmpdir(), 'cruise-option-browser-'));
const repo = join(root, 'repo');
const id = '20261025000000002';
const sessionDir = join(root, 'data/cruise/sessions', id);
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, serverClosed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('option browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(repo, { recursive: true });
  await mkdir(sessionDir, { recursive: true });
  for (const args of [
    ['init', '-b', 'main'], ['config', 'user.name', 'Responsive BDD'],
    ['config', 'user.email', 'responsive-bdd@example.invalid'], ['commit', '--allow-empty', '-m', 'isolated option fixture'],
  ]) {
    const result = spawnSync('git', args, { cwd: repo, env, encoding: 'utf8' });
    assert.equal(result.status, 0, `isolated git fixture: ${result.stderr}`);
  }
  await writeFile(join(sessionDir, 'state.json'), JSON.stringify({
    id, base_dir: repo, phase: 'Planned', config: { kind: 'inline_snapshot' },
    input: 'Choose an option in a real production browser', title: 'Responsive option lifecycle',
    current_step: null, created_at: '2026-10-25T00:00:00Z', completed_at: null,
    worktree_path: null, worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main',
  }));
  const choices = Array.from({ length: 18 }, (_, i) => `      - selector: "Choice ${i + 1}: review the responsive production layout before continuing"\n        next: done`).join('\n');
  // This is a real map-shaped WorkflowConfig. The final shell step only writes inside this temporary repo.
  await writeFile(join(sessionDir, 'config.yaml'), `create_pr: false\nsteps:\n  choose:\n    plan: "{plan}"\n    option:\n${choices}\n      - text-input: "Explain your responsive layout choice"\n        next: done\n  done:\n    command: "printf option-completed > option-completed.txt"\n`);
  await writeFile(join(sessionDir, 'plan.md'), '# Option context\n\n' + 'Review the mobile dialog and keep all choices reachable. '.repeat(16));
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['webui', '--port', '0', '--webui-dir', resolve('webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  serverClosed = new Promise(ok => server.once('close', ok));
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
  // Do not leak Cargo's macOS library search paths into Chromium helper processes.
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  const context = await browser.newContext({ viewport: { width, height }, reducedMotion: 'reduce' });
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  const run = page.getByRole('button', { name: 'Run on current branch', exact: true });
  await run.waitFor();
  await run.click();
  const dialog = page.getByRole('dialog');
  await dialog.waitFor();
  assert.equal(await dialog.getByRole('button', { name: /^Choice / }).count(), 18, 'all real workflow choices rendered');
  const metrics = await dialog.evaluate(el => {
    const box = el.getBoundingClientRect(), css = getComputedStyle(el);
    return { x: box.x, y: box.y, right: box.right, bottom: box.bottom,
      clientHeight: el.clientHeight, scrollHeight: el.scrollHeight, overflowY: css.overflowY,
      docWidth: document.documentElement.scrollWidth, viewportWidth: innerWidth, viewportHeight: innerHeight };
  });
  assert(metrics.x >= 0 && metrics.y >= 0 && metrics.right <= metrics.viewportWidth + 1 && metrics.bottom <= metrics.viewportHeight + 1,
    `R6 real option dialog must fit viewport: ${JSON.stringify(metrics)}`);
  assert(metrics.scrollHeight > metrics.clientHeight, 'long real option dialog must have overflowing content');
  assert.equal(metrics.overflowY, 'auto', 'R6 option dialog scrolls internally');
  assert(metrics.docWidth <= metrics.viewportWidth + 1, 'real option dialog must not widen document');
  const input = dialog.locator('input[name=textInput]');
  assert.equal(await input.evaluate(el => getComputedStyle(el).fontSize), width < 768 ? '16px' : '14px', 'R8 real option input computed font size');
  await input.fill('BDD verified bounded dialog and reachable input');
  const response = page.waitForResponse(response => response.url().endsWith(`/webui/sessions/${id}/option`) && response.request().method() === 'POST');
  await dialog.getByRole('button', { name: 'Submit', exact: true }).click();
  assert.equal((await response).status(), 200, 'real option answer accepted');
  await dialog.waitFor({ state: 'detached' });
  const deadline = Date.now() + 15000;
  let state;
  do {
    state = JSON.parse(await readFile(join(sessionDir, 'state.json'), 'utf8'));
    if (state.phase === 'Completed') break;
    assert(Date.now() < deadline, `option workflow did not complete: ${JSON.stringify(state)}\n${logs}`);
    await new Promise(ok => setTimeout(ok, 50));
  } while (true);
  assert.equal(await readFile(join(repo, 'option-completed.txt'), 'utf8'), 'option-completed', 'real shell successor executes after answer');
  assert.deepEqual(errors, [], 'production option lifecycle JavaScript must not throw');
  console.log(`WEBUI_BROWSER_CONTRACT_PASSED option ${width}x${height}`);
} catch (error) {
  console.error(error, '\nProduction server stderr:', logs);
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
  try {
    await browser?.close();
  } finally {
    try {
      if (server) {
        server.kill('SIGKILL');
        await serverClosed;
      }
    } finally {
      await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}
