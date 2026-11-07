// Real local-merge browser contract using isolated Git worktrees.
// Run after building cruise-runtime: node tests/webui/local-merge.mjs
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-local-merge-browser-'));
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('local merge browser watchdog expired', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
function git(cwd, ...args) {
  const result = spawnSync('git', args, { cwd, env, encoding: 'utf8' });
  assert.equal(result.status, 0, `git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}
async function exists(path) { try { await access(path); return true; } catch (error) { if (error.code === 'ENOENT') return false; throw error; } }
async function fixture(suffix) {
  const id = `2026102500000000${suffix}`;
  const repo = join(root, `repo-${suffix}`), worktree = join(root, `worktree-${suffix}`);
  const branch = `fixture-${suffix}`, sessionDir = join(root, 'data/cruise/sessions', id);
  await mkdir(repo, { recursive: true });
  await mkdir(sessionDir, { recursive: true });
  git(repo, 'init', '-b', 'main');
  git(repo, 'config', 'user.name', 'Browser Fixture');
  git(repo, 'config', 'user.email', 'browser@example.invalid');
  await writeFile(join(repo, 'base.txt'), 'base\n');
  git(repo, 'add', 'base.txt');
  git(repo, 'commit', '-m', 'base');
  const head = git(repo, 'rev-parse', 'HEAD');
  git(repo, 'worktree', 'add', '-b', branch, worktree);
  await writeFile(join(worktree, 'committed.txt'), 'committed change\n');
  git(worktree, 'add', 'committed.txt');
  git(worktree, 'commit', '-m', 'worktree change');
  await writeFile(join(worktree, 'uncommitted.txt'), 'uncommitted change\n');
  await writeFile(join(sessionDir, 'state.json'), JSON.stringify({ id, base_dir: repo,
    phase: 'Completed', config: { kind: 'inline_snapshot' }, input: 'Local merge browser fixture',
    title: 'Local merge browser fixture', current_step: null, created_at: '2026-10-25T00:00:00Z',
    completed_at: '2026-10-25T00:01:00Z', worktree_path: worktree, worktree_branch: branch,
    workspace_mode: 'Worktree', forked_from_branch: 'main' }));
  await writeFile(join(sessionDir, 'config.yaml'), 'create_pr: false\nsteps:\n  harmless:\n    command: "true"\n');
  return { id, repo, worktree, branch, sessionDir, head };
}
try {
  const stage = await fixture('1');
  const merge = await fixture('2');
  const dirty = await fixture('3');
  await writeFile(join(dirty.repo, 'base.txt'), 'dirty base\n');
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['webui', '--port', '0', '--webui-dir', resolve('webui')],
    { cwd: stage.repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', chunk => { logs += chunk; });
  const base = await new Promise((ok, fail) => {
    let output = '';
    const timeout = setTimeout(() => fail(new Error(`server startup timeout: ${logs}`)), 20000);
    server.once('error', error => { clearTimeout(timeout); fail(error); });
    server.once('exit', code => { clearTimeout(timeout); fail(new Error(`server exited ${code}: ${logs}`)); });
    server.stdout.on('data', chunk => {
      output += chunk;
      const match = output.match(/http:\/\/127\.0\.0\.1:\d+/);
      if (match) { clearTimeout(timeout); ok(match[0]); }
    });
  });
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  console.log(`Node ${process.version}, Chromium ${browser.version()}`);
  const reports = resolve(process.env.WEBUI_LOCAL_MERGE_REPORT_DIR || 'target/formal-reports/local-merge-browser');
  await mkdir(reports, { recursive: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  async function open(f) {
    assert.equal((await page.goto(`${base}/sessions/${f.id}`)).status(), 200);
    await page.waitForFunction(() => !!window.app);
    await page.getByRole('button', { name: 'Stage local merge', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Merge locally & cleanup', exact: true }).waitFor();
  }
  async function clickConfirmed(label, endpoint, accept, expectedStatus = 200) {
    const confirmation = new Promise(resolve => page.once('dialog', async dialog => {
      assert.equal(dialog.type(), 'confirm');
      await (accept ? dialog.accept() : dialog.dismiss());
      resolve();
    }));
    const response = accept ? page.waitForResponse(r => r.url().endsWith(endpoint) && r.request().method() === 'POST') : null;
    await page.getByRole('button', { name: label, exact: true }).click();
    await confirmation;
    if (response) assert.equal((await response).status(), expectedStatus);
  }
  await open(stage);
  let posts = 0;
  page.on('request', request => { if (request.method() === 'POST' && request.url().endsWith(`/webui/sessions/${stage.id}/stage-local`)) posts++; });
  await clickConfirmed('Stage local merge', `/webui/sessions/${stage.id}/stage-local`, false);
  assert.equal(posts, 0, 'cancel must not submit');
  assert.equal(git(stage.repo, 'rev-parse', 'HEAD'), stage.head);
  assert.equal(git(stage.repo, 'status', '--porcelain'), '');
  await clickConfirmed('Stage local merge', `/webui/sessions/${stage.id}/stage-local`, true);
  await page.waitForFunction(() => !document.querySelector('.htmx-request'));
  assert.equal(git(stage.repo, 'rev-parse', 'HEAD'), stage.head, 'stage does not commit in base');
  assert.deepEqual(git(stage.repo, 'diff', '--cached', '--name-only').split('\n').sort(), ['committed.txt', 'uncommitted.txt']);
  assert(await exists(stage.worktree));
  assert(await exists(join(stage.sessionDir, 'state.json')));
  assert(git(stage.repo, 'branch', '--list', stage.branch).includes(stage.branch));
  await page.getByText('Local merge staged', { exact: true }).waitFor();
  await page.screenshot({ path: join(reports, 'stage-success.png') });
  await open(dirty);
  await clickConfirmed('Merge locally & cleanup', `/webui/sessions/${dirty.id}/merge-local`, true, 400);
  await page.getByText('base checkout has uncommitted changes', { exact: false }).waitFor();
  await page.screenshot({ path: join(reports, 'dirty-base-error.png') });
  assert.equal(git(dirty.repo, 'rev-parse', 'HEAD'), dirty.head);
  assert.equal(await readFile(join(dirty.repo, 'base.txt'), 'utf8'), 'dirty base\n');
  assert(await exists(dirty.worktree));
  assert(await exists(join(dirty.sessionDir, 'state.json')));
  assert(git(dirty.repo, 'branch', '--list', dirty.branch).includes(dirty.branch));
  await open(merge);
  await clickConfirmed('Merge locally & cleanup', `/webui/sessions/${merge.id}/merge-local`, true);
  await page.waitForFunction(() => !document.querySelector('.htmx-request'));
  await page.waitForURL(`${base}/`);
  await page.getByText('Merged locally', { exact: true }).waitFor();
  await page.screenshot({ path: join(reports, 'merge-success.png') });
  assert.notEqual(git(merge.repo, 'rev-parse', 'HEAD'), merge.head);
  assert.equal(git(merge.repo, 'rev-list', '--count', `${merge.head}..HEAD`), '1', 'one squash commit');
  assert.equal(await readFile(join(merge.repo, 'committed.txt'), 'utf8'), 'committed change\n');
  assert.equal(await readFile(join(merge.repo, 'uncommitted.txt'), 'utf8'), 'uncommitted change\n');
  assert.equal(git(merge.repo, 'status', '--porcelain'), '');
  assert.equal(await exists(merge.worktree), false);
  assert.equal(git(merge.repo, 'branch', '--list', merge.branch), '');
  assert.equal(await exists(join(merge.sessionDir, 'state.json')), false);
  assert.deepEqual(errors, [], 'no browser exceptions');
  console.log('Local merge browser contracts passed');
} finally {
  clearTimeout(watchdog);
  await browser?.close();
  if (server && server.exitCode === null && server.signalCode === null) server.kill('SIGTERM');
  if (closed) await closed;
  await rm(root, { recursive: true, force: true });
}
