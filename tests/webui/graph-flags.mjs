// Production graph tab with persisted flags, real HTMX and an isolated HOME/XDG tree.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-graph-browser-'));
const repo = join(root, 'repo');
const id = '20261025000000003';
const session = join(root, 'data/cruise/sessions', id);
const output = resolve(process.env.WEBUI_GRAPH_REPORT_DIR || 'target/formal-reports/graph-browser');
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('graph browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(repo, { recursive: true });
  await mkdir(session, { recursive: true });
  await mkdir(output, { recursive: true });
  const state = { id, base_dir: repo, phase: 'Planned', config: { kind: 'inline_snapshot' },
    input: 'Graph flag browser fixture', title: 'Graph flag fixture', current_step: null,
    created_at: '2026-10-25T00:00:00Z', completed_at: null, worktree_path: null,
    worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main', flags: {} };
  const saveState = () => writeFile(join(session, 'state.json'), JSON.stringify(state));
  await saveState();
  await writeFile(join(session, 'config.yaml'), "flags:\n  browser: {}\nsteps:\n  first: {command: 'true'}\n  gated: {command: 'true', flag: browser}\n  last: {command: 'true'}\n");
  await writeFile(join(session, 'plan.md'), '# Graph flag fixture\n');
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
  for (const [name, enabled] of [['off', false], ['on', true], ['off-again', false], ['all-disabled', false]]) {
    state.flags = { browser: enabled };
    await saveState();
    if (name === 'all-disabled') {
      await writeFile(join(session, 'config.yaml'), "flags:\n  browser: {}\nsteps:\n  gated: {command: 'true', flag: browser}\n");
    }
    // Remove the previous panel before requesting the next state, so screenshots
    // cannot accidentally capture a stale swap.
    if (name !== 'off') {
      await page.getByRole('tab', { name: 'Info', exact: true }).click();
      await page.locator(`#tab-dag-${id}`).waitFor({ state: 'detached' });
    }
    const response = page.waitForResponse(r => r.url().endsWith(`/webui/sessions/${id}/tab/dag`));
    await page.getByRole('tab', { name: 'Graph', exact: true }).click();
    const result = await response;
    assert.equal(result.status(), 200);
    const html = await result.text();
    if (name === 'all-disabled') {
      assert.match(html, /graph TD/);
      assert.doesNotMatch(html, /gated|first|last|--&gt;/, 'empty graph must contain no nodes or edges');
    } else {
      assert.match(html, /first/);
      assert.match(html, /last/);
      assert.equal(html.includes('gated'), enabled, `${name}: gated node visibility`);
      assert.match(html, enabled ? /s\d+_first --&gt; s\d+_gated/ : /s\d+_first --&gt; s\d+_last/, `${name}: projected edge`);
    }
    await page.locator(`#tab-dag-${id}`).waitFor();
    await page.locator(`#tab-dag-${id} pre.mermaid`).waitFor({ state: 'attached' });
    await page.waitForFunction(id => {
      const pre = document.querySelector(`#tab-dag-${id} pre.mermaid`);
      return pre && (pre.textContent.trim().length > 0 || pre.querySelector('svg'));
    }, id);
    await page.screenshot({ path: join(output, `${name}.png`), fullPage: true });
    console.log(`graph ${name}: passed; screenshot ${join(output, `${name}.png`)}`);
  }
  assert.deepEqual(errors, [], 'production graph page must not throw');
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
