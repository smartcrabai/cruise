// Production WebUI, real Quint, and a deterministic Claude process boundary.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-verify-e2e-'));
const id = '20261107000000001';
const dir = join(root, 'data/cruise/sessions', id), repo = join(root, 'repo'), bin = join(root, 'bin');
let server, closed, browser, logs = '';
try {
  await Promise.all([mkdir(dir, { recursive: true }), mkdir(repo), mkdir(bin)]);
  const plan = '# Saved verification plan\n\nNever modify this plan.\n';
  await writeFile(join(dir, 'state.json'), JSON.stringify({ id, base_dir: repo, phase: 'AwaitingApproval',
    config: { kind: 'inline_snapshot' }, input: 'Verify the saved plan', title: 'Verify browser fixture',
    current_step: null, created_at: '2026-11-07T00:00:00Z', completed_at: null,
    worktree_path: null, worktree_branch: null }));
  await writeFile(join(dir, 'config.yaml'), 'sdk: claude\nmodel: fixture-model\nsteps:\n  harmless:\n    command: "true"\n');
  await writeFile(join(dir, 'plan.md'), plan);
  const spec = '```quint\nthis is not valid quint\n```';
  const assistant = text => JSON.stringify({ type: 'assistant', message: { role: 'assistant', content: [{ type: 'text', text }] }, session_id: 'fixture' });
  const first = `${assistant(spec)}\n${JSON.stringify({ type: 'result', subtype: 'success', is_error: false, session_id: 'fixture', result: spec })}`;
  const second = `${assistant('FIXTURE_INTERPRETATION')}\n${JSON.stringify({ type: 'result', subtype: 'success', is_error: false, session_id: 'fixture', result: 'FIXTURE_INTERPRETATION' })}`;
  const callLog = join(root, 'provider-calls');
  await writeFile(join(bin, 'claude'), `#!/bin/sh\nread -r frame\nprintf '%s\\n' "$frame" >> '${callLog}'\nif [ "$(wc -l < '${callLog}' | tr -d ' ')" = 1 ]; then\n  printf '%s\\n' '${first}'\nelse\n  printf '%s\\n' '${second}'\nfi\n`, { mode: 0o755 });
  const env = { PATH: `${bin}:${resolve('tests/formal/node_modules/.bin')}:${process.env.PATH}`, HOME: root,
    XDG_CONFIG_HOME: join(root, 'config'), XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
    CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1' };
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['webui', '--port', '0', '--webui-dir', resolve('webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', chunk => { logs += chunk; });
  const base = await new Promise((ok, fail) => {
    let output = '';
    const timer = setTimeout(() => fail(new Error(`startup timeout: ${logs}`)), 20000);
    server.once('error', fail);
    server.once('exit', code => fail(new Error(`server exited ${code}: ${logs}`)));
    server.stdout.on('data', chunk => { output += chunk; const match = output.match(/http:\/\/127\.0\.0\.1:\d+/); if (match) { clearTimeout(timer); ok(match[0]); } });
  });
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  const page = await browser.newPage();
  page.setDefaultTimeout(20000);
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  const verify = page.getByRole('button', { name: 'Verify', exact: true });
  await verify.waitFor();
  const response = page.waitForResponse(r => r.url() === `${base}/webui/sessions/${id}/verify` && r.request().method() === 'POST');
  await verify.click();
  assert.equal((await response).status(), 200, 'Verify button POST reaches production route');
  const progress = page.locator(`#plan-progress-${id}`);
  await page.waitForFunction(id => document.querySelector(`#plan-progress-${id}`)?.textContent.includes('FIXTURE_INTERPRETATION'), id, { timeout: 60000 });
  const text = await progress.innerText();
  assert.match(text, /parse/i, 'real checker report contains parse stage');
  assert.match(text, /error|failed/i, 'invalid specification yields a failure report');
  assert.equal((await readFile(callLog, 'utf8')).trim().split('\n').length, 2, 'provider generates and interprets report');
  assert.equal(await readFile(join(dir, 'plan.md'), 'utf8'), plan, 'Verify preserves saved plan');
  assert.deepEqual(errors, []);
  console.log('VERIFY_WEBUI_E2E_PASSED');
} catch (error) {
  console.error(error, '\nProduction server stderr:', logs);
  process.exitCode = 1;
} finally {
  await browser?.close();
  if (server) { server.kill('SIGKILL'); await closed; }
  await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
