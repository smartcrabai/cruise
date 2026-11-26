import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { access, mkdir, readFile, realpath, rename, symlink, unlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const revision = 7;
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const fixtureOrigin = 'https://github.com/example/quint-handoff-fixture.git';

export async function prepareManagerHandoff({ repo, dir, id, data, env, bin, run, reports }) {
  run('git', ['remote', 'add', 'origin', fixtureOrigin], repo);
  run('git', ['update-ref', 'refs/remotes/origin/main', 'HEAD'], repo);
  run('git', ['config', 'user.name', 'Manager Quint Fixture'], repo);
  run('git', ['config', 'user.email', 'manager-quint@example.invalid'], repo);
  const anchor = await realpath(repo);
  const repository = await realpath(join(repo, '.git'));
  const key = Buffer.from(repository).toString('base64url');
  const recordDir = join(data, 'manager');
  const record = join(recordDir, `${sha(repository)}.json`);
  const load = async () => JSON.parse(await readFile(record, 'utf8'));
  const target = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: repo, env, encoding: 'utf8' }).stdout.trim();
  const permit = join(data, 'allow-quint-child');
  const command = 'set -eu; test -f "$QUINT_INPUT"; echo "Child read the approved Quint file"; shasum -a 256 "$QUINT_INPUT" | cut -d " " -f 1; test -f "$PERMIT"; echo received > quint-result';
  await mkdir(recordDir, { recursive: true });
  await writeFile(record, JSON.stringify({ repository, goals: [],
    transcript: [{ id: 'fixture-user', role: 'user', text: 'Reuse the reviewed counter specification.', status: 'completed' }],
    proposal: {
      id: 'fixture-manager-quint', revision, objective: 'Check the counter contract', workflow: 'builtin:tdd',
      target_branch: 'main', target_sha: target, exclusions: ['External services'],
      criteria: [{ id: 'counter', description: 'Read the pinned model' }],
      checks: [{ id: 'offline', criterion_ids: ['counter'], argv: ['false'] }],
      tasks: [{ id: 'read-counter', instructions: 'Check the counter contract', dependencies: [] },
        { id: 'unassigned', instructions: 'Do not receive a model', dependencies: ['read-counter'] }],
      contract: { anchor, origin: fixtureOrigin, forge_kind: 'GitHub', forge_host: 'github.com',
        forge_repository: 'example/quint-handoff-fixture', workflow: { command: ['sh'], create_pr: false,
          steps: { 'check-contract': { command, env: { QUINT_INPUT: '{quint.goal_approved}', PERMIT: permit } } } } },
    },
  }));
  const terminal = [];
  const cli = (label, args, exit, output) => {
    const command = ['manager', ...args];
    const pty = process.env.WEBUI_QUINT_PTY;
    const receipt = `cruise ${command.join(' ')}`;
    const wrapper = `import errno, os, pty, sys
pid, terminal = pty.fork()
if pid == 0:
    assert all(os.isatty(fd) for fd in (0, 1, 2))
    os.execv(sys.argv[1], sys.argv[1:])
while True:
    try: data = os.read(terminal, 65536)
    except OSError as error:
        if error.errno != errno.EIO: raise
        break
    if not data: break
    sys.stdout.buffer.write(data)
sys.exit(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]))`;
    const child = pty ? spawnSync(pty, ['-c', wrapper, bin, ...command], { cwd: repo, env, encoding: 'utf8' })
      : spawnSync(bin, command, { cwd: repo, env, encoding: 'utf8' });
    assert.ifError(child.error);
    const text = `${child.stdout}${child.stderr}`.replaceAll('\r', '').trim();
    assert.equal(child.status, exit, text);
    assert.match(text, output);
    const lines = text.split('\n');
    const shown = lines.length > 14 ? ['… full proposal omitted …', ...lines.slice(-2)].join('\n') : text;
    terminal.push([label, `$ ${receipt}\n${shown}\n[exit ${child.status}]`]);
  };
  const bind = (source, expected = revision, task = 'read-counter') => ['bind-quint', task, source, '--revision', String(expected)];
  cli('Unapproved source', bind(id), 1, /no approved model/);
  return async ({ page, base, context, source, foreign }) => {
    const action = name => `/webui/manager/${key}/${name}`;
    const waitFor = name => page.waitForResponse(r => r.url() === `${base}${action(name)}` && r.request().method() === 'POST');
    const execution = source.execution_id;
    const preview = source.quint_run.stage.preview;
    const root = join(dir, 'artifacts', 'quint', execution);
    const modelPath = join(root, `${preview.model_sha256}.qnt`);
    const bytes = await readFile(modelPath);
    const png = await readFile(join(dir, 'artifacts', preview.image.name));
    assert.equal(sha(bytes), preview.model_sha256);
    assert.equal(sha(png), preview.image.sha256);
    cli('Source in another repository', bind(foreign), 1, /another repository/);
    cli('Unknown destination task', bind(id, revision, 'not-in-proposal'), 1, /unknown task/);
    cli('Bind a reviewed model; no child starts', bind(id), 0, /Pending proposal revision 8 \(not approved\)/);
    assert.equal((await load()).goals.length, 0);
    cli('Stale proposal revision', ['approve', '--yes', '--revision', String(revision)], 1, /stale proposal revision/i);
    await writeFile(modelPath, 'tampered model\n');
    cli('Changed approved model bytes', ['approve', '--yes', '--revision', '8'], 1, /Approved Quint model changed/);
    await rename(modelPath, `${modelPath}.saved`);
    cli('Missing approved model', ['approve', '--yes', '--revision', '8'], 1, /failed to read artifact/);
    await symlink(`${modelPath}.saved`, modelPath);
    cli('Symbolic link is not an approved file', ['approve', '--yes', '--revision', '8'], 1, /symbolic link/);
    await unlink(modelPath);
    await rename(`${modelPath}.saved`, modelPath);
    await writeFile(modelPath, bytes);
    const statePath = join(dir, 'state.json');
    await writeFile(statePath, JSON.stringify({ ...source, execution_id: 'cf4f187a-a959-4412-81f9-5c85405a0d13', quint_run: null }));
    cli('Source approval invalidated', ['approve', '--yes', '--revision', '8'], 1, /no approved model/);
    await writeFile(statePath, JSON.stringify(source));
    assert.equal((await load()).goals.length, 0);

    await page.goto(`${base}/manager/${key}`);
    await page.getByLabel('Source session ID').fill(id);
    const [bound] = await Promise.all([waitFor('bind-quint'), page.getByRole('button', { name: 'Attach to preview' }).click()]);
    assert.equal(bound.status(), 202, await bound.text());
    await page.locator('[data-manager-review$="-9"]').click();
    const panel = page.getByRole('region', { name: 'Review proposal approval' });
    const image = panel.getByAltText('Approved Quint model diagram');
    await image.waitFor();
    await image.evaluate(img => img.decode());
    const imageUrl = `${base}${await image.getAttribute('src')}`;
    assert.deepEqual(await (await context.request.get(imageUrl)).body(), png);
    const unauthenticated = await fetch(imageUrl);
    assert.equal(unauthenticated.status, 401);
    assert.match(await panel.innerText(), new RegExp(preview.model_sha256));
    assert.match(await panel.innerText(), new RegExp(execution));
    assert.match(await panel.innerText(), /for task read-counter/);
    assert.match(await panel.innerText(), /Revision: 9/);
    const message = page.locator('#manager-message');
    await message.click();
    await message.pressSequentially('Editing while the approval panel stays open');
    assert(await panel.isVisible());
    assert.equal(await message.inputValue(), 'Editing while the approval panel stays open');
    await panel.evaluate(el => el.scrollIntoView({ block: 'start' }));
    await page.screenshot({ path: join(reports, 'manager-quint-inline-review.png') });
    const [approved] = await Promise.all([waitFor('approve'), panel.getByRole('button', { name: 'Confirm approval' }).click()]);
    assert(approved.ok(), await approved.text());
    assert.equal((await context.request.get(imageUrl)).status(), 404, 'consumed proposal has no preview URL');
    const readGoal = async () => (await load()).goals[0];
    const eventually = async (read, condition) => {
      let value;
      for (let attempt = 0; attempt < 100; attempt++) {
        value = await read();
        if (condition(value)) return value;
        await delay(100);
      }
      assert.fail(`Expected Manager state not reached: ${JSON.stringify(value)}`);
    };
    const failed = await eventually(readGoal, goal => goal?.reason?.includes('Managed child failure for task read-counter'));
    const sessions = join(data, 'sessions');
    const child = join(sessions, failed.execution.attempts[0].session_id);
    const copy = join(child, 'artifacts', 'manager-quint', 'approved.qnt');
    const childState = async () => JSON.parse(await readFile(join(child, 'state.json'), 'utf8'));
    assert.deepEqual(await readFile(copy), bytes);
    await writeFile(copy, 'modified child copy\n');
    await writeFile(permit, '');
    const resume = ['resume', failed.id, 'read-counter'];
    cli('Request resume with a changed child copy', resume, 0, /Accepted/);
    await eventually(childState, state => JSON.stringify(state.phase).includes('Manager child Quint file changed'));
    await assert.rejects(access(join((await childState()).worktree_path, 'quint-result')));
    await writeFile(copy, bytes);
    cli('Resume with the original digest restored', resume, 0, /Accepted/);
    const goal = await eventually(readGoal, goal => goal?.execution.attempts.length === 2 && goal.reason?.includes('unassigned'));
    assert.equal(goal.proposal.revision, 9);
    const assigned = goal.execution.attempts.find(a => a.task_id === 'read-counter');
    const unassigned = goal.execution.attempts.find(a => a.task_id === 'unassigned');
    assert(assigned?.integrated && unassigned && !unassigned.integrated, JSON.stringify(goal));
    const receipt = async attempt => readFile(join(sessions, attempt.session_id, 'run.log'), 'utf8');
    assert.match(await receipt(assigned), new RegExp(preview.model_sha256));
    const peer = join(sessions, unassigned.session_id);
    await assert.rejects(access(join(peer, 'artifacts', 'manager-quint', 'approved.qnt')));
    const peerState = JSON.parse(await readFile(join(peer, 'state.json'), 'utf8'));
    assert.match(JSON.stringify(peerState.phase), /quint\.goal_approved/);
    assert.deepEqual(await readFile(copy), bytes);
    assert(!(await readFile(join(child, 'plan.md'), 'utf8')).includes('module model {'));
    assert.equal(JSON.parse(await readFile(join(child, 'state.json'), 'utf8')).quint_run ?? null, null);
    await page.goto(`${base}/sessions/${assigned.session_id}?tab=log`);
    await page.getByText('Child read the approved Quint file', { exact: false }).first().waitFor();
    await page.screenshot({ path: join(reports, 'manager-quint-child-log.png') });
    console.log(`Manager handoff: source review PNG ${sha(png)}, child read ${bytes.length} bytes SHA-256 ${sha(bytes)}, unassigned child got no model`);
    if (process.env.WEBUI_QUINT_PTY) {
      const shot = await context.newPage();
      const escape = text => text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
      await shot.setContent(`<meta charset="utf-8"><style>html{background:#10192b;color:#f1f5f9;font:16px/1.35 monospace}body{margin:25px}h1,h2{font-family:system-ui}h2{font-size:16px;color:#b9dbff}pre{white-space:pre-wrap;overflow-wrap:anywhere}</style><h1>Manager Quint handoff · real PTY</h1>${terminal.map(([label, output]) => `<h2>${escape(label)}</h2><pre>${escape(output)}</pre>`).join('')}`);
      await shot.screenshot({ path: join(reports, 'manager-quint-pty.png'), fullPage: true });
      await shot.close();
    }
  };
}
