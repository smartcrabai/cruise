import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { access, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';
import { loginWebUi, waitForWebUiLogin } from './auth.mjs';

if (process.argv[2] === '--agent') {
  let prompt = '';
  for await (const chunk of process.stdin) prompt += chunk;
  if (prompt.includes('Draft the required behavior')) {
    const saved = prompt.includes('Make the saved-state label clearer') ? 'Seven stored on disk' : 'Counter saved';
    console.log(`\`\`\`quint
module model {
  var count: int
  var diagram: { state: str, via: str }
  action init = all { count' = 0, diagram' = { state: "Empty counter", via: "start" } }
  action step = all { count == 0, count' = 7, diagram' = { state: "${saved}", via: "persist seven" } }
  val invPersisted = diagram.state == "${saved}" implies count == 7
  run saveTest = init.then(step).expect(invPersisted and count == 7)
}
\`\`\``);
  } else if (prompt.includes('Verify that the implemented behavior')) {
    const model = prompt.match(/^Approved model file: (.+)$/m)?.[1];
    assert(model, 'final check receives the approved model path');
    assert.equal(await readFile('pre.txt', 'utf8'), 'pre');
    const measured = spawnSync('cat', ['observed.txt'], { encoding: 'utf8' });
    assert.equal(measured.status, 0);
    assert.equal(measured.stdout, '7\n');
    await writeFile('model.qnt', await readFile(model));
    await writeFile('check-model.sh', 'quint typecheck model.qnt && quint test model.qnt\n');
    console.log(JSON.stringify({ model: 'model.qnt', ci: 'check-model.sh', blockers: [],
      checks: [{ command: 'cat observed.txt', exit: measured.status, excerpt: measured.stdout.trim(), earlier: false }],
      measurements: [{ label: 'counter entries', before: 0, after: Number(measured.stdout) }], recordings: [] }));
  } else {
    throw new Error('Unexpected agent prompt');
  }
  process.exit(0);
}

const root = await mkdtemp(join(tmpdir(), 'cruise-quint-browser-'));
const reports = resolve(process.env.WEBUI_QUINT_REPORT_DIR ?? 'target/formal-reports/quint-browser');
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', CRUISE_UI_LANGUAGE: 'en', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
const data = join(env.XDG_DATA_HOME, 'cruise');
const self = fileURLToPath(import.meta.url);
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const run = (cmd, args, cwd) => {
  const result = spawnSync(cmd, args, { cwd, env, encoding: 'utf8' });
  assert.ifError(result.error);
  assert.equal(result.status, 0, `${cmd} failed: ${result.stderr}`);
};
async function fixture(name, enabled) {
  const id = `2026112600000000${{ approve: 1, reject: 2, revise: 3, off: 4, cli: 5 }[name]}`;
  const repo = join(root, name);
  const dir = join(data, 'sessions', id);
  await mkdir(repo, { recursive: true });
  await mkdir(dir, { recursive: true });
  run('git', ['init', '-b', 'main'], repo);
  run('git', ['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--allow-empty', '-m', 'fixture'], repo);
  await writeFile(join(dir, 'state.json'), JSON.stringify({ id, base_dir: repo, phase: 'Planned',
    config: { kind: 'inline_snapshot' }, input: 'Persist the public counter value seven', title: `Quint ${name}`,
    execution_id: randomUUID(), flags: { formal_methods: enabled }, created_at: '2026-11-26T00:00:00Z',
    workspace_mode: 'CurrentBranch', target_branch: 'main' }));
  await writeFile(join(dir, 'plan.md'), '# Counter\nPersist the synthetic value seven.\n');
  await writeFile(join(dir, 'config.yaml'), `create_pr: false\ncommand: ${JSON.stringify([process.execPath, self, '--agent'])}\nflags:\n  formal_methods:\n    quint_review: true\nsteps:\n  pre/setup:\n    command: "printf pre > pre.txt"\n  implement:\n    command: "printf '7\\\\n' > observed.txt"\n`);
  return { id, repo, dir };
}
async function stored(dir) { return JSON.parse(await readFile(join(dir, 'state.json'), 'utf8')); }
async function complete(dir, expected) {
  for (let attempt = 0; attempt < 150; attempt++) {
    const state = await stored(dir);
    if (state.phase === 'Completed' || Object.hasOwn(state.phase, 'Failed')) {
      assert.equal(typeof state.phase === 'string' ? state.phase : 'Failed', expected, JSON.stringify(state.phase));
      return state;
    }
    await delay(100);
  }
  assert.fail(`Timed out waiting for ${expected}: ${await readFile(join(dir, 'run.log'), 'utf8')}`);
}

let browser, server, closed, logs = '';
const watchdog = setTimeout(() => { console.error('Quint browser watchdog expired'); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await mkdir(data, { recursive: true });
  await mkdir(reports, { recursive: true });
  await symlink(resolve('tests/formal'), join(data, 'quint-tools'), 'dir');
  const bin = process.env.WEBUI_BDD_BIN ?? resolve('target/debug/cruise-runtime');
  server = spawn(bin, ['daemon', 'serve', '--port', '0', '--webui-dir', resolve('webui')],
    { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', data => { logs += data; });
  const { base, loginUrl } = await waitForWebUiLogin(server, () => logs);
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  const context = await browser.newContext({ viewport: { width: 1200, height: 900 }, reducedMotion: 'reduce' });
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  await loginWebUi(page, loginUrl);
  for (const name of ['approve', 'reject', 'revise', 'off']) {
    const { repo, dir, id } = await fixture(name, name !== 'off');
    await page.goto(`${base}/sessions/${id}`);
    await page.waitForFunction(() => !!window.app);
    await page.getByRole('button', { name: 'Run on current branch', exact: true }).click();
    const panel = page.getByRole('region', { name: 'Quint model review' });
    const diagram = panel.getByRole('img', { name: 'Quint state and transition diagram' });
    const ready = async () => {
      await diagram.waitFor();
      await diagram.evaluate(img => img.decode());
      assert((await diagram.evaluate(img => img.naturalWidth)) > 100, 'real image decoded');
      await assert.rejects(access(join(repo, 'pre.txt')));
      await assert.rejects(access(join(repo, 'observed.txt')));
      const url = await diagram.getAttribute('src');
      const png = await page.request.get(`${base}${url}`);
      assert.equal(png.headers()['content-type'], 'image/png');
      return { url, digest: sha(await png.body()) };
    };
    if (name !== 'off') {
      const first = await ready();
      assert.equal(await page.getByRole('dialog').count(), 0);
      await page.getByRole('tab', { name: 'Plan', exact: true }).click();
      await page.getByRole('heading', { name: 'Counter', exact: true }).waitFor();
      assert.deepEqual(await ready(), first, 'changing tabs leaves the review available');
      await page.screenshot({ path: join(reports, `${name}-waiting.png`) });
      await page.reload();
      assert.deepEqual(await ready(), first, 'reattach shows the same pending preview');
      if (name === 'approve') {
        const other = await fixture('cli', false);
        await page.locator(`#session-row-${other.id}`).click();
        await page.waitForURL(`**/sessions/${other.id}`);
        await panel.waitFor({ state: 'detached' });
        await page.getByRole('button', { name: 'Run on current branch', exact: true }).click();
        await complete(other.dir, 'Completed');
        assert.equal(await readFile(join(other.repo, 'observed.txt'), 'utf8'), '7\n');
        assert.equal((await stored(dir)).awaiting_input, true);
        await assert.rejects(access(join(repo, 'observed.txt')));
        await page.getByText('Completed', { exact: true }).first().waitFor();
        await page.screenshot({ path: join(reports, 'other-session-completed.png') });
        await page.locator(`#session-row-${id}`).click();
        await page.waitForURL(`**/sessions/${id}`);
        assert.deepEqual(await ready(), first, 'returning keeps the original pending review');
        console.log('nonmodal: Plan tab remains usable; another session completed while approval was pending');
      }
      if (name === 'revise') {
        await panel.getByRole('textbox').fill('Make the saved-state label clearer');
        await panel.getByRole('button', { name: 'Submit', exact: true }).click();
        await page.waitForFunction(previous => {
          const img = document.querySelector('img[alt="Quint state and transition diagram"]');
          return img && img.getAttribute('src') !== previous;
        }, first.url);
        const second = await ready();
        assert.notEqual(second.digest, first.digest, 'revision produced a different PNG');
        assert.equal((await page.request.get(`${base}${first.url}`)).status(), 404, 'stale PNG request unavailable');
        await page.screenshot({ path: join(reports, 'revise-new.png') });
        console.log(`revision: PNG ${first.digest.slice(0, 12)} -> ${second.digest.slice(0, 12)}; no pre/implementation writes`);
      }
      if (name === 'reject') {
        const otherTab = await context.newPage();
        try {
          await otherTab.goto(`${base}/sessions/${id}`);
          await otherTab.getByRole('button', { name: 'Reject and stop', exact: true }).click();
          await panel.waitFor({ state: 'detached' });
        } finally { await otherTab.close(); }
      } else {
        await panel.getByRole('button', { name: 'Approve Quint model', exact: true }).click();
        await panel.waitFor({ state: 'detached' });
      }
    }
    const state = await complete(dir, name === 'reject' ? 'Failed' : 'Completed');
    if (name === 'reject') {
      assert.equal(state.quint_run.stage.status, 'rejected');
      assert.match(state.phase.Failed, /Quint model rejected/);
      await assert.rejects(access(join(repo, 'pre.txt')));
      await assert.rejects(access(join(repo, 'observed.txt')));
      assert.equal(state.pr_url, null);
      console.log(`reject: ${state.phase.Failed}; writes=0`);
    } else {
      assert.equal(await readFile(join(repo, 'observed.txt'), 'utf8'), '7\n');
      if (name === 'off') {
        assert.equal(state.quint_run.mode, 'off');
        await assert.rejects(access(join(dir, 'artifacts', 'quint')));
        await assert.rejects(access(join(repo, 'model.qnt')));
        console.log('off: completed; counter=7; no approval or Quint artifacts');
      } else {
        assert.equal(state.quint_run.stage.status, 'verified');
        const approved = state.quint_run.stage.preview.model_sha256;
        assert.equal(sha(await readFile(join(repo, 'model.qnt'))), approved);
        console.log(`${name}: verified; approved model ${approved.slice(0, 12)}; counter=7`);
      }
    }
  }
} catch (error) {
  console.error(error, logs);
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
  try { await browser?.close(); }
  finally {
    if (server) { server.kill('SIGKILL'); await closed; }
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
}
