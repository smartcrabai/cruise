// Production WebUI + real Git/gh, with an isolated local repository per scenario.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';

const output = resolve(process.env.WEBUI_PR_REPORT_DIR || 'target/formal-reports/pr-failed-browser');
await mkdir(output, { recursive: true });
const cleanEnv = { ...process.env };
for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete cleanEnv[key];
const browser = await chromium.launch({ headless: true, env: cleanEnv });
console.log(`Node ${process.version}, Chromium ${browser.version()}`);

async function scenario(name, changed) {
  const root = await mkdtemp(join(tmpdir(), 'cruise-pr-real-'));
  const repo = join(root, 'repo'), worktree = join(root, 'worktree');
  const id = '20261105000000001', session = join(root, 'data/cruise/sessions', id);
  const env = { ...cleanEnv, HOME: root, XDG_CONFIG_HOME: join(root, 'config'), XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'), GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_AUTHOR_NAME: 'Browser Fixture', GIT_AUTHOR_EMAIL: 'fixture@example.invalid', GIT_COMMITTER_NAME: 'Browser Fixture', GIT_COMMITTER_EMAIL: 'fixture@example.invalid', CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1' };
  delete env.GH_TOKEN; delete env.GITHUB_TOKEN;
  let server, closed, logs = '';
  const git = (cwd, ...args) => { const r = spawnSync('git', args, { cwd, env, encoding: 'utf8' }); assert.equal(r.status, 0, `git ${args.join(' ')}: ${r.stderr}`); };
  try {
    await mkdir(repo, { recursive: true }); await mkdir(session, { recursive: true });
    git(repo, 'init', '-b', 'main'); git(repo, 'commit', '--allow-empty', '-m', 'base');
    // This intentionally absent local remote makes push fail without network or credentials.
    git(repo, 'remote', 'add', 'origin', join(root, 'missing.git'));
    git(repo, 'worktree', 'add', '-b', 'fixture/pr-failed', worktree);
    if (changed) await writeFile(join(worktree, 'change.txt'), 'local change\n');
    await writeFile(join(session, 'state.json'), JSON.stringify({ id, base_dir: repo, phase: { PrFailed: 'original publication failure' }, config: { kind: 'inline_snapshot' }, input: 'Publish fixture', title: 'PR retry fixture', current_step: null, created_at: '2026-11-05T00:00:00Z', completed_at: null, worktree_path: worktree, worktree_branch: 'fixture/pr-failed' }));
    await writeFile(join(session, 'config.yaml'), `command: ["sh", "-c", "printf 'TITLE: fixture PR\\nBODY: fixture body\\n'"]\nsteps:\n  workflow:\n    command: 'touch ${join(root, 'workflow-ran')}'\n`);
    await writeFile(join(session, 'plan.md'), '# Fixture plan\n');
    server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'), ['webui', '--port', '0', '--webui-dir', resolve('webui')], { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
    closed = new Promise(ok => server.once('close', ok)); server.stderr.on('data', d => { logs += d; });
    const base = await new Promise((ok, fail) => { let stdout = ''; const timer = setTimeout(() => fail(new Error(`startup timeout ${logs}`)), 20000); server.once('error', fail); server.once('exit', code => fail(new Error(`server exited ${code}: ${logs}`))); server.stdout.on('data', d => { stdout += d; const m = stdout.match(/http:\/\/127\.0\.0\.1:\d+/); if (m) { clearTimeout(timer); ok(m[0]); } }); });
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: 'reduce' });
    try {
      page.setDefaultTimeout(15000); const errors = []; page.on('pageerror', e => errors.push(e.message));
      assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
      await page.waitForFunction(() => !!window.app);
      const header = page.locator(`#session-header-${id}`);
      await header.getByText('original publication failure', { exact: false }).waitFor();
      assert.match(await header.innerText(), /PR Failed/);
      for (const button of ['Run', 'Retry']) assert.equal(await header.getByRole('button', { name: button, exact: true }).count(), 0);
      await page.screenshot({ path: join(output, `${name}-before.png`) });
      const response = page.waitForResponse(r => r.request().method() === 'POST' && r.url().endsWith(`/webui/sessions/${id}/create-pr`));
      await header.getByRole('button', { name: 'Create PR', exact: true }).click();
      assert.equal((await response).status(), 200);
      await page.waitForFunction(({ id, changed }) => { const text = document.getElementById(`session-header-${id}`)?.textContent || ''; return changed ? text.includes('git push failed') : text.includes('Completed'); }, { id, changed }, { timeout: 30000 });
      const state = JSON.parse(await readFile(join(session, 'state.json'), 'utf8'));
      if (changed) {
        assert.match(state.phase.PrFailed, /git push failed/);
        assert.equal(await header.getByRole('button', { name: 'Create PR', exact: true }).isVisible(), true);
        assert.equal(await readFile(join(worktree, 'change.txt'), 'utf8'), 'local change\n');
      } else {
        assert.equal(state.phase, 'Completed');
        assert.equal(await header.getByRole('button', { name: 'Create PR', exact: true }).count(), 0);
        assert.equal(state.pr_url ?? null, null);
      }
      await assert.rejects(readFile(join(root, 'workflow-ran')), { code: 'ENOENT' });
      assert.deepEqual(errors, []);
      await page.screenshot({ path: join(output, `${name}-after.png`) });
      console.log(`${name}: passed`);
    } finally { await page.close(); }
  } finally {
    if (server) { server.kill('SIGTERM'); await Promise.race([closed, new Promise(ok => setTimeout(ok, 5000))]); if (server.exitCode === null) server.kill('SIGKILL'); await closed; }
    await rm(root, { recursive: true, force: true });
  }
}
try { await scenario('no-commits', false); await scenario('push-failure', true); }
finally { await browser.close(); }
