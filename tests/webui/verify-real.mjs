// Real Claude provider, production runtime and real formal checker. No provider stubs.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

if (!process.env.ANTHROPIC_API_KEY) throw new Error('ANTHROPIC_API_KEY is required for the real-provider test');
const root = await mkdtemp(join(tmpdir(), 'cruise-verify-real-'));
const repo = join(root, 'repo');
const id = '20261107000000001', draftId = '20261107000000002';
const output = resolve(process.env.WEBUI_VERIFY_REPORT_DIR || 'target/formal-reports/verify-browser');
let server, closed, browser, logs = '';
try {
  await mkdir(repo, { recursive: true });
  await mkdir(output, { recursive: true });
  const plan = '# Counter requirements\n\nA counter starts at zero. It can increment while below two. It must always remain between zero and two inclusive.\n';
  for (const [sessionId, phase] of [[id, 'AwaitingApproval'], [draftId, 'Draft']]) {
    const dir = join(root, 'data/cruise/sessions', sessionId);
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, 'state.json'), JSON.stringify({ id: sessionId, base_dir: repo, phase,
      config: { kind: 'inline_snapshot' }, input: 'Verify the counter requirements', title: `Verify ${phase}`,
      current_step: null, created_at: '2026-11-07T00:00:00Z', completed_at: null,
      worktree_path: null, worktree_branch: null }));
    await writeFile(join(dir, 'config.yaml'), `sdk: claude\nmodel: ${process.env.WEBUI_VERIFY_MODEL || 'haiku'}\nsteps:\n  harmless:\n    command: "true"\n`);
    await writeFile(join(dir, 'plan.md'), plan);
  }
  const env = { ...process.env, PATH: `${resolve('tests/formal/node_modules/.bin')}:${process.env.PATH}`,
    HOME: root, XDG_CONFIG_HOME: join(root, 'config'), XDG_DATA_HOME: join(root, 'data'),
    XDG_STATE_HOME: join(root, 'state'), CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1',
    GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', NO_COLOR: '1' };
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['webui', '--port', '0', '--webui-dir', resolve('webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', chunk => { logs += chunk; });
  const base = await new Promise((ok, fail) => {
    let stdout = '';
    const timer = setTimeout(() => fail(new Error(`startup timeout: ${logs}`)), 20000);
    server.once('error', fail);
    server.once('exit', code => fail(new Error(`server exited ${code}: ${logs}`)));
    server.stdout.on('data', chunk => { stdout += chunk; const match = stdout.match(/http:\/\/127\.0\.0\.1:\d+/); if (match) { clearTimeout(timer); ok(match[0]); } });
  });
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  delete browserEnv.ANTHROPIC_API_KEY;
  browser = await chromium.launch({ headless: true, env: browserEnv });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
  page.setDefaultTimeout(20000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  assert.equal((await page.goto(`${base}/sessions/${draftId}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  assert.equal(await page.getByRole('button', { name: 'Verify', exact: true }).count(), 0, 'Draft must not offer Verify');
  await page.screenshot({ path: join(output, 'boundary-draft.png'), fullPage: true });
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  const verify = page.getByRole('button', { name: 'Verify', exact: true });
  await verify.waitFor();
  const response = page.waitForResponse(r => r.url() === `${base}/webui/sessions/${id}/verify` && r.request().method() === 'POST');
  await verify.click();
  assert.equal((await response).status(), 200, 'Verify POST reaches production handler');
  const progress = page.locator(`#plan-progress-${id}`);
  await page.waitForFunction(id => document.querySelector(`#plan-progress-${id}`)?.textContent.includes('Formal verification:'), id, { timeout: 300000 });
  // The action reappears only after the asynchronous verification operation finishes.
  await verify.waitFor({ state: 'visible', timeout: 300000 });
  const text = await progress.innerText();
  assert.match(text, /Formal verification:/i, 'real checker report is shown');
  assert.match(text, /(?:quint|alloy) block \d+/i, 'provider produced a formal block checked by the runtime');
  assert.match(text, /- [^\n:]+: (?:passed|failed|error) \(timedOut:/i, 'checker stages are reported');
  assert.match(text, /- [^\n:]+: passed \(timedOut: false\)/i, 'at least one checker stage must pass');
  assert.equal(await readFile(join(root, 'data/cruise/sessions', id, 'plan.md'), 'utf8'), plan, 'saved plan is unchanged');
  assert.deepEqual(errors, [], 'no browser exceptions');
  await page.screenshot({ path: join(output, 'verification-report.png'), fullPage: true });
  console.log('Real provider verification completed and report observed. This does not assert a passing formal verdict.');
} catch (error) {
  console.error(error, '\nProduction server stderr:', logs);
  process.exitCode = 1;
} finally {
  await browser?.close();
  if (server) { server.kill('SIGKILL'); await closed; }
  await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
