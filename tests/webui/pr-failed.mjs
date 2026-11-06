// Test-only entry point: real WebUI and Git, isolated local remote and fake forge boundary.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const root = await mkdtemp(join(tmpdir(), 'cruise-pr-failed-browser-'));
const repo = join(root, 'repo'), worktree = join(root, 'worktree'), bin = join(root, 'bin');
const id = '20261105000000001', session = join(root, 'data/cruise/sessions', id);
const url = 'https://github.com/fixture/project/pull/42';
const env = { PATH: `${bin}:${process.env.PATH}`, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_AUTHOR_NAME: 'Browser Fixture',
  GIT_AUTHOR_EMAIL: 'fixture@example.invalid', GIT_COMMITTER_NAME: 'Browser Fixture',
  GIT_COMMITTER_EMAIL: 'fixture@example.invalid', CRUISE_DISABLE_NOTIFICATIONS: '1',
  CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => { console.error('PR browser contract timed out', logs); server?.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  await Promise.all([mkdir(repo, { recursive: true }), mkdir(bin), mkdir(session, { recursive: true })]);
  const git = (cwd, ...args) => {
    const result = spawnSync('git', args, { cwd, env, encoding: 'utf8' });
    assert.equal(result.status, 0, `git ${args.join(' ')}: ${result.stderr}`);
  };
  git(repo, 'init', '-b', 'main');
  git(repo, 'commit', '--allow-empty', '-m', 'fixture base');
  git(root, 'init', '--bare', join(root, 'remote.git'));
  git(repo, 'remote', 'add', 'origin', join(root, 'remote.git'));
  git(repo, 'worktree', 'add', '-b', 'fixture/pr-failed', worktree);
  await writeFile(join(worktree, 'change.txt'), 'publication fixture\n');
  const forgeLog = join(root, 'forge.log');
  await writeFile(join(bin, 'gh'), `#!/bin/sh\nprintf '%s\\n' "$*" >> "${forgeLog}"\nif [ "$1" = --version ]; then echo 'gh fixture'; elif [ "$1" = pr ] && [ "$2" = create ]; then echo '${url}'; else echo 'unexpected gh invocation' >&2; exit 1; fi\n`, { mode: 0o755 });
  await writeFile(join(session, 'state.json'), JSON.stringify({ id, base_dir: repo,
    phase: { PrFailed: 'fixture push failed' }, config: { kind: 'inline_snapshot' },
    input: 'Publish the fixture', title: 'PR retry fixture', current_step: null,
    created_at: '2026-11-05T00:00:00Z', completed_at: null,
    worktree_path: worktree, worktree_branch: 'fixture/pr-failed' }));
  await writeFile(join(session, 'config.yaml'), `command: ["sh", "-c", "printf 'TITLE: fixture PR\\nBODY: fixture body\\n'"]\nsteps:\n  workflow:\n    command: 'touch ${join(root, 'workflow-ran')}'\n`);
  await writeFile(join(session, 'plan.md'), '# Fixture plan\n');
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
  const page = await browser.newPage();
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  const header = page.locator(`#session-header-${id}`);
  assert.match(await header.innerText(), /PR Failed/);
  assert.match(await header.innerText(), /fixture push failed/);
  assert.equal(await header.getByRole('button', { name: 'Run', exact: true }).count(), 0);
  assert.equal(await header.getByRole('button', { name: 'Retry', exact: true }).count(), 0);
  const create = header.getByRole('button', { name: 'Create PR', exact: true });
  assert.equal(await create.isVisible(), true);
  const response = page.waitForResponse(r => r.request().method() === 'POST' && r.url().endsWith(`/webui/sessions/${id}/create-pr`));
  await create.click();
  assert.equal((await response).status(), 200);
  await page.waitForFunction(({ id, url }) => {
    const header = document.getElementById(`session-header-${id}`);
    return header?.textContent.includes('Completed') && header.textContent.includes(url);
  }, { id, url }, { timeout: 30000 });
  const state = JSON.parse(await readFile(join(session, 'state.json'), 'utf8'));
  assert.equal(state.phase, 'Completed');
  assert.equal(state.pr_url, url);
  assert.equal(await header.getByRole('button', { name: 'Create PR', exact: true }).count(), 0);
  assert.deepEqual((await readFile(forgeLog, 'utf8')).trim().split('\n').filter(line => line.startsWith('pr create ')).length, 1);
  await assert.rejects(access(join(root, 'workflow-ran')), { code: 'ENOENT' }, 'Create PR must not rerun the workflow');
  assert.deepEqual(errors, []);
  console.log('PR_FAILED_BROWSER_CONTRACT_PASSED');
} finally {
  clearTimeout(watchdog);
  await browser?.close();
  if (server) { server.kill('SIGTERM'); await Promise.race([closed, new Promise(ok => setTimeout(ok, 5000))]); if (server.exitCode === null) server.kill('SIGKILL'); await closed; }
  await rm(root, { recursive: true, force: true });
}
