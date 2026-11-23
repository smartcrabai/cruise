// Production WebUI, coordinator and managed Option, without a model or remote service.
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readdir, readFile, realpath, rename, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import { loginWebUi, waitForWebUiLogin } from './auth.mjs';

const root = await mkdtemp(join(tmpdir(), 'cruise-manager-browser-'));
const repo = join(root, 'repo');
const report = resolve(process.env.WEBUI_MANAGER_REPORT_DIR || 'target/formal-reports/manager-browser');
const objective = 'Review a local Manager workflow';
const question = 'What should the Manager workflow use?';
const answer = 'Use the local browser fixture';
const proposalId = 'browser-proposal';
const revision = 7;
const taskId = 'browser-task';
const origin = 'https://github.com/example/manager-browser-fixture.git';
const env = {
  PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', CRUISE_UI_LANGUAGE: 'en', NO_COLOR: '1',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_ALLOW_PROTOCOL: 'file',
};
let server, closed, browser, page, record, logs = '';
const watchdog = setTimeout(() => {
  console.error('Manager browser watchdog expired', logs);
  server?.kill('SIGKILL');
  process.exit(1);
}, 120000);
try {
  await mkdir(repo);
  await mkdir(report, { recursive: true });
  const configDir = join(env.XDG_CONFIG_HOME, 'cruise');
  await mkdir(configDir, { recursive: true });
  await writeFile(join(configDir, 'default.yaml'), 'check_base_ci: false\n');
  const git = (...args) => execFileSync('git', args, { cwd: repo, env, encoding: 'utf8' }).trim();
  git('init', '-b', 'main');
  git('config', 'user.name', 'Manager Browser');
  git('config', 'user.email', 'manager-browser@example.invalid');
  git('commit', '--allow-empty', '-m', 'isolated Manager fixture');
  const target = git('rev-parse', 'HEAD');
  const linked = join(root, 'linked');
  git('worktree', 'add', '--detach', linked, target);
  const unlisted = join(root, 'unlisted');
  git('init', '-b', 'main', unlisted);
  git('remote', 'add', 'origin', origin);
  git('update-ref', 'refs/remotes/origin/main', target);
  const anchor = await realpath(repo);
  const repository = await realpath(join(repo, '.git'));
  const key = Buffer.from(repository).toString('base64url');
  const recordDir = join(root, 'data/cruise/manager');
  record = join(recordDir, `${createHash('sha256').update(repository).digest('hex')}.json`);
  await mkdir(recordDir, { recursive: true });
  const conversation = 'Please review the local workflow before approving it.';
  await writeFile(record, JSON.stringify({
    repository,
    transcript: [{ id: 'browser-turn', role: 'user', text: conversation, status: 'completed' }],
    proposal: {
      id: proposalId, revision, objective, target_branch: 'main', target_sha: target,
      criteria: [{ id: 'local', description: 'Stay in the disposable checkout' }],
      checks: [{ id: 'local-check', criterion_ids: ['local'], argv: ['git', 'status', '--short'] }],
      exclusions: ['External services'],
      tasks: [{ id: taskId, instructions: 'Exercise the local Option', dependencies: [] }],
      workflow: 'builtin:tdd',
      contract: {
        anchor, origin, forge_kind: 'GitHub', forge_host: 'github.com',
        forge_repository: 'example/manager-browser-fixture',
        // Managed children already have a nonempty approved plan as input, so
        // a prompt-step instruction does not trigger Ask. Use a real Option.
        workflow: {
          create_pr: false,
          steps: {
            choose: { option: [{ 'text-input': question, next: 'intentional_failure' }] },
            intentional_failure: {
              command: 'printf "%s\\n" "$BROWSER_ANSWER"; exit 42',
              env: { BROWSER_ANSWER: '{prev.input}' },
            },
          },
        },
      },
    },
    goals: [],
  }));
  const snapshot = async () => JSON.parse(await readFile(record, 'utf8'));

  const binary = resolve(process.env.WEBUI_BDD_BIN || 'target/debug/cruise-runtime');
  const start = (runtime, args = []) => {
    server = spawn(runtime, ['daemon', 'serve', '--port', '0', ...args],
      { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
    closed = new Promise(ok => server.once('close', ok));
    server.stderr.on('data', data => { logs += data; });
    return waitForWebUiLogin(server, () => logs);
  };
  const stop = async () => {
    server.kill('SIGTERM');
    await closed;
    server = undefined;
  };
  const baseline = process.env.WEBUI_MANAGER_BASELINE_BIN;
  const assetRoot = join(env.XDG_DATA_HOME, 'cruise', 'webui');
  const manifest = async directory => {
    const files = await readdir(directory, { recursive: true, withFileTypes: true });
    return Promise.all(files.filter(file => file.isFile()).map(async file => {
      const path = join(file.parentPath, file.name);
      return [path, createHash('sha256').update(await readFile(path)).digest('hex')];
    }));
  };
  let oldTree, oldManifest, currentMarker, markerBefore;
  if (baseline) {
    await start(resolve(baseline));
    await stop();
    const trees = await readdir(assetRoot);
    assert.equal(trees.length, 1);
    oldTree = join(assetRoot, trees[0]);
    assert.match(trees[0], /^\d+\.\d+\.\d+$/);
    assert(!(await readdir(join(oldTree, 'templates'))).includes('manager-page.tsx'));
    oldManifest = (await manifest(oldTree)).sort();
    await start(binary);
    await stop();
    const newTrees = (await readdir(assetRoot)).filter(tree => tree !== trees[0]);
    assert.equal(newTrees.length, 1);
    assert.equal(newTrees[0].split('-')[0], trees[0], 'both binaries have the same package version');
    assert.match(newTrees[0].split('-')[1], /^[a-f0-9]{64}$/);
    currentMarker = join(assetRoot, newTrees[0], '.complete');
    markerBefore = await stat(currentMarker);
  }
  const { base, loginUrl } = await start(binary, baseline ? [] : ['--webui-dir', resolve('webui')]);
  if (baseline) {
    const markerAfter = await stat(currentMarker);
    assert.equal(markerAfter.ino, markerBefore.ino);
    assert.equal(markerAfter.mtimeMs, markerBefore.mtimeMs);
    assert.deepEqual((await manifest(oldTree)).sort(), oldManifest);
    console.log('Embedded asset migration preserved the baseline and reused the task extraction.');
  }
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  console.log(`Chromium ${browser.version()}`);
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, reducedMotion: 'reduce', locale: 'en-US' });
  page = await context.newPage();
  page.setDefaultTimeout(15000);
  const errors = [], bearerRequests = [], approvalRequests = [];
  const actionPath = action => `/webui/manager/${key}/${action}`;
  const waitForAction = action => page.waitForResponse(response =>
    response.url() === `${base}${actionPath(action)}` && response.request().method() === 'POST');
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => {
    if (request.url().startsWith(`${base}/api/manager/`)) bearerRequests.push(request.url());
    if (request.url() === `${base}${actionPath('approve')}` && request.method() === 'POST') approvalRequests.push(request);
  });
  const screenshot = name => page.screenshot({ path: join(report, `${name}.png`), fullPage: true });
  await loginWebUi(page, loginUrl);
  await page.getByRole('link', { name: 'Manager', exact: true }).click();
  await page.waitForURL(`${base}/manager`);
  const repoLink = page.locator(`a[href="/manager/${key}"]`);
  await repoLink.waitFor();
  assert((await repoLink.innerText()).includes(repository), 'lists the canonical repository');
  await screenshot('01-manager-list');
  const openForm = page.locator('form[action="/webui/manager/open"]');
  await openForm.locator('[name="path"]').fill(unlisted);
  const [rejected] = await Promise.all([
    page.waitForResponse(r => r.url() === `${base}/webui/manager/open` && r.request().method() === 'POST'),
    openForm.locator('button[type="submit"]').click(),
  ]);
  assert.equal(rejected.status(), 400);
  await page.locator('#manager-open-error').getByText('No Manager record exists for this repository.').waitFor();
  assert.equal(page.url(), `${base}/manager`, 'a rejected Open remains on the list');
  await openForm.locator('[name="path"]').fill(linked);
  await openForm.locator('button[type="submit"]').click();
  await page.waitForURL(`${base}/manager/${key}`);
  await page.getByText(conversation, { exact: true }).waitFor();
  await page.getByText(objective, { exact: true }).first().waitFor();
  await screenshot('02-proposal');

  const review = page.getByRole('button', { name: /^(approve|approve proposal)$/i });
  await review.click();
  const dialog = page.getByRole('dialog');
  await dialog.waitFor();
  assert.match(await dialog.innerText(), new RegExp(objective));
  assert.match(await dialog.innerText(), new RegExp(`revision\\W+${revision}\\b`, 'i'));
  assert.equal(approvalRequests.length, 0, 'opening review sends no approval');
  assert.deepEqual((await snapshot()).goals, [], 'opening review creates no Goal');
  await screenshot('03-approve-dialog');
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await dialog.waitFor({ state: 'hidden' });
  assert.equal(approvalRequests.length, 0, 'canceling review sends no approval');
  assert.deepEqual((await snapshot()).goals, [], 'canceling review creates no Goal');
  await review.click();
  await dialog.waitFor();
  const [approved] = await Promise.all([waitForAction('approve'), dialog.locator('button[type="submit"]').click()]);
  assert(approved.ok(), `approval returned HTTP ${approved.status()}`);
  const approvalPayload = new URLSearchParams(approved.request().postData());
  assert.equal(approvalPayload.get('proposal_id'), proposalId);
  assert.equal(approvalPayload.get('revision'), String(revision));
  assert.equal(approvalRequests.length, 1, 'explicit confirmation sends one approval');
  const [goal] = (await snapshot()).goals;
  assert(goal?.id, 'approval durably registers a Goal');
  assert.equal(goal.proposal.id, proposalId);
  assert.equal(goal.proposal.revision, revision);
  // No reload: the new Goal and its real prompt must appear via action fragment and polling.
  const optionForm = page.locator(`form[hx-post="${actionPath('answer-option')}"]`).filter({ has: page.locator('[name="textInput"]') });
  await optionForm.waitFor();
  await page.getByText(/^(?:Status:\s*)?Active$/i).first().waitFor();
  await page.getByText(question, { exact: true }).waitFor();
  const [attempt] = (await snapshot()).goals[0].execution.attempts;
  assert.equal(attempt.task_id, taskId);
  const childLink = page.locator(`main a[href="/sessions/${attempt.session_id}"]`).first();
  await childLink.waitFor();
  const sync = await page.waitForResponse(response =>
    response.url() === `${base}${actionPath('sync')}` && response.request().method() === 'GET');
  assert(sync.ok(), `Manager sync returned HTTP ${sync.status()}`);
  await optionForm.locator('[name="textInput"]').fill(answer);
  await screenshot('04-prompt-answer');
  const promptId = await optionForm.locator('input[name="prompt_id"]').inputValue();
  assert(promptId, 'rendered Option binds the live prompt identity');
  const [answered] = await Promise.all([waitForAction('answer-option'), optionForm.locator('button[type="submit"]').click()]);
  assert(answered.ok(), `answer returned HTTP ${answered.status()}`);
  const answerPayload = new URLSearchParams(answered.request().postData());
  assert.equal(answerPayload.get('goal_id'), goal.id);
  assert.equal(answerPayload.get('prompt_id'), promptId);
  assert.equal(answerPayload.get('textInput'), answer);
  assert.equal(answerPayload.get('nextStep'), 'intentional_failure');
  await optionForm.waitFor({ state: 'detached' });
  await page.getByText(/Managed child failure for task browser-task/).first().waitFor();
  await screenshot('05-failed-attempt');

  await childLink.click();
  await page.waitForURL(`${base}/sessions/${attempt.session_id}`);
  const header = page.locator(`#session-header-${attempt.session_id}`);
  await header.getByText(/Manager child, read-only/).waitFor();
  assert.equal(await header.locator('[hx-post$="/run"], [hx-post$="/reset"], [hx-get$="/editor/tell"]').count(), 0);
  await page.getByText(answer, { exact: false }).first().waitFor();
  const backToManager = header.getByRole('link', { name: 'Open in Manager', exact: true });
  assert.equal(await backToManager.getAttribute('href'), `/manager/${key}`);
  await screenshot('06-child-manager-link');
  await backToManager.click();
  await page.waitForURL(`${base}/manager/${key}`);
  await page.locator(`main a[href="/sessions/${attempt.session_id}"]`).first().waitFor();
  assert.deepEqual(bearerRequests, [], 'the browser never calls the bearer-token Manager API');
  assert.deepEqual(errors, [], 'no browser exceptions');

  const messageForm = page.locator(`form[hx-post="${actionPath('message')}"]`);
  const composer = messageForm.locator('textarea');
  await composer.fill('An unsent draft');
  await page.waitForResponse(`${base}${actionPath('sync')}`);
  assert.equal(await composer.inputValue(), 'An unsent draft');

  await composer.fill('   ');
  const [rejectedMessage] = await Promise.all([waitForAction('message'), messageForm.locator('button[type="submit"]').click()]);
  assert.equal(rejectedMessage.status(), 400);
  await page.locator('#manager-action-error p').waitFor();
  assert.equal(await composer.inputValue(), '   ');

  const newMessage = 'Try a new conversation turn without provider credentials.';
  await composer.fill(newMessage);
  const [accepted] = await Promise.all([waitForAction('message'), messageForm.locator('button[type="submit"]').click()]);
  assert.equal(accepted.status(), 202);
  await page.waitForFunction(() => document.querySelector('#manager-message').value === '');
  await page.getByText(/^Turn interrupted:/).waitFor({ timeout: 30000 });
  const transcript = (await snapshot()).transcript;
  assert.equal(transcript.at(-2).text, newMessage);
  assert.equal(transcript.at(-1).status, 'interrupted');

  await page.route(`${base}${actionPath('message')}`, async route => {
    await composer.fill('A newer edit');
    await route.continue();
  }, { times: 1 });
  await composer.fill('Another submitted turn');
  const [acceptedWithEdit] = await Promise.all([waitForAction('message'), messageForm.locator('button[type="submit"]').click()]);
  assert.equal(acceptedWithEdit.status(), 202);
  await page.getByText('Another submitted turn', { exact: true }).waitFor();
  assert.equal(await composer.inputValue(), 'A newer edit');

  // Keep the linked worktree and persisted record, but remove access to the main checkout.
  await rename(repo, join(root, 'unavailable-main'));
  const missingMain = await page.goto(`${base}/manager/${key}`);
  assert(!missingMain.ok(), 'detail cannot load without the main checkout');
  console.log(`Manager browser scenario passed. Screenshots: ${report}`);
} catch (error) {
  if (page) await page.screenshot({ path: join(report, 'failure.png'), fullPage: true }).catch(() => {});
  console.error(error, '\nProduction server stderr:', logs);
  if (record) console.error('Manager record:', await readFile(record, 'utf8').catch(() => '(unavailable)'));
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
  try { await browser?.close(); } finally {
    try { if (server) { server.kill('SIGKILL'); await closed; } } finally {
      await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}
