// Production browser bridge for webui_ask_keyboard.feature.
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium, errors } from 'playwright';
import { loginWebUi, waitForWebUiLogin } from './auth.mjs';

const modifier = process.argv[2];
assert(['Meta', 'Control', 'send-recovery'].includes(modifier), 'expected Meta, Control, or send-recovery');
const reportDir = resolve('target/formal-reports/ask-keyboard-browser', modifier.toLowerCase());
await mkdir(reportDir, { recursive: true });
const root = await mkdtemp(join(tmpdir(), 'cruise-ask-browser-'));
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', CRUISE_UI_LANGUAGE: 'en',
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, serverClosed, browser, logs = '';
const watchdog = setTimeout(() => {
  console.error('Planning keyboard browser watchdog expired', logs);
  server?.kill('SIGKILL');
  process.exit(1);
}, 90000);
try {
  const id = '20261025000000003';
  const tellId = '20261025000000004';
  const question = 'What should this workflow do?';
  const tellRepo = join(root, `repo-${tellId}`);
  const release = join(tellRepo, 'release');
  async function fixture(id, config) {
    const repo = join(root, `repo-${id}`);
    await mkdir(repo);
    execFileSync('git', ['init', '-b', 'main'], { cwd: repo, env });
    execFileSync('git', ['-c', 'user.name=Planning BDD', '-c', 'user.email=planning@example.invalid',
      'commit', '--allow-empty', '-m', 'isolated planning fixture'], { cwd: repo, env });
    const session = join(root, 'data/cruise/sessions', id);
    await mkdir(session, { recursive: true });
    await writeFile(join(session, 'state.json'), JSON.stringify({
      id, base_dir: repo, phase: 'Planned', config: { kind: 'inline_snapshot' },
      input: '', title: 'Planning keyboard fixture', current_step: null,
      created_at: '2026-10-25T00:00:00Z', completed_at: null, worktree_path: null,
      worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main',
    }));
    await writeFile(join(session, 'plan.md'), '# Planning keyboard fixture\n');
    await writeFile(join(session, 'config.yaml'), config);
  }
  await fixture(id, `command: [head, -n, '2']\ncreate_pr: false\nsteps:\n  ask:\n    instruction: ${question}\n    prompt: 'Use this answer: {input}'\n`);
  // Bounded hold so an interrupted test cannot leave a command running indefinitely.
  await fixture(tellId, `create_pr: false\nsteps:\n  hold:\n    command: 'for i in $(seq 1 200); do [ -f release ] && break; sleep 0.1; done'\n`);

  const binary = resolve(process.env.WEBUI_BDD_BIN || 'target/debug/cruise-runtime');
  server = spawn(binary, ['daemon', 'serve', '--port', '0', '--webui-dir', resolve('webui')],
    { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
  serverClosed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', data => { logs += data; });
  const { base, loginUrl } = await waitForWebUiLogin(server, () => logs);
  browser = await chromium.launch({ headless: true });
  console.log(`Chromium ${browser.version()}`);
  const context = await browser.newContext({ reducedMotion: 'reduce', colorScheme: 'light' });
  context.setDefaultTimeout(15000);
  const page = await context.newPage();
  const screenshot = name => page.screenshot({ path: join(reportDir, `${name}.png`) });
  await loginWebUi(page, loginUrl);

  const endpoint = `${base}/webui/sessions/${id}/ask-answer`;
  const isAnswer = request => request.url() === endpoint && request.method() === 'POST';
  assert.equal((await page.goto(`${base}/sessions/${id}`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  await page.getByRole('button', { name: 'Run on current branch', exact: true }).click();
  const panel = page.getByRole('region', { name: 'Planning agent question' });
  await panel.getByText(question, { exact: true }).waitFor();
  const answer = panel.getByRole('textbox', { name: 'Your answer' });
  assert.equal(await panel.locator('form').getAttribute('hx-sync'), 'this:drop');
  const requestField = panel.locator('input[name=requestId]');
  const requestId = await requestField.inputValue();
  assert(requestId, 'the pending question has a request ID');
  if (modifier === 'send-recovery') {
    const text = 'Keep this draft\nfor the retry';
    const send = panel.getByRole('button', { name: 'Send answer', exact: true });
    let first = true;
    await page.route(endpoint, async route => {
      if (first && isAnswer(route.request())) {
        first = false;
        await route.fulfill({ status: 500 });
      } else {
        await route.continue();
      }
    });
    await answer.fill(text);
    const [failure] = await Promise.all([
      page.waitForResponse(response => isAnswer(response.request())),
      send.click(),
    ]);
    assert.equal(failure.status(), 500);
    await answer.and(page.locator(':enabled')).waitFor();
    await send.and(page.locator(':enabled')).waitFor().catch(error => {
      throw new Error('Send answer must re-enable after HTTP 500', { cause: error });
    });
    assert.equal(await answer.inputValue(), text);
    assert.equal(await requestField.inputValue(), requestId);
    const [retry] = await Promise.all([
      page.waitForResponse(response => isAnswer(response.request())),
      send.click(),
    ]);
    assert.equal(retry.status(), 200);
    const fields = new URLSearchParams(retry.request().postData());
    assert.equal(fields.get('requestId'), requestId);
    assert.equal(fields.get('answer'), text);
    await panel.waitFor({ state: 'detached' });
    console.log('ASK_SEND_RECOVERY_PASSED');
  } else {
    await exerciseShortcut();
  }

  async function exerciseShortcut() {
    await page.reload();
    await panel.getByText(question, { exact: true }).waitFor();
    assert.equal(await requestField.inputValue(), requestId, 'reload preserves the pending request ID');

    const posted = [];
    page.on('request', request => { if (request.method() === 'POST') posted.push(request.url()); });
    await answer.fill('First line');
    await answer.press('End');
    await answer.press('Enter');
    assert.equal(await answer.inputValue(), 'First line\n');
    await answer.evaluate(el => {
      function observeArrowLeft(event) {
        if (event.target !== el || event.key !== 'ArrowLeft') return;
        el.dataset.arrowLeftCancelled = String(event.defaultPrevented);
        window.removeEventListener('keydown', observeArrowLeft);
      }
      window.addEventListener('keydown', observeArrowLeft);
    });
    await answer.press(`${modifier}+ArrowLeft`);
    assert.equal(await answer.getAttribute('data-arrow-left-cancelled'), 'false',
      `${modifier}+ArrowLeft must not be cancelled`);
    assert.equal(await answer.inputValue(), 'First line\n');
    await answer.fill('');
    await answer.evaluate(el => {
      el.addEventListener('invalid', () => { el.dataset.invalidObserved = 'yes'; }, { once: true });
    });
    await answer.press(`${modifier}+Enter`);
    assert.equal(await answer.getAttribute('data-invalid-observed'), 'yes', 'native validation fired');
    assert.deepEqual(posted, []);
    await screenshot('empty-answer');
    console.log('ASK_KEYBOARD_NATIVE_PASSED');

    const editorPage = await context.newPage();
    assert.equal((await editorPage.goto(`${base}/sessions/${tellId}`)).status(), 200);
    await editorPage.waitForFunction(() => !!window.app);
    await editorPage.getByRole('button', { name: 'Run on current branch', exact: true }).click();
    await editorPage.locator(`#session-header-${tellId}`).getByRole('button', { name: 'Tell', exact: true }).click();
    const editor = editorPage.locator(`#editor-${tellId}`);
    const instruction = editor.locator('textarea[name=instruction]');
    const editorPosts = [];
    editorPage.on('request', request => { if (request.method() === 'POST') editorPosts.push(request.url()); });
    await instruction.fill('Separate instruction');
    await instruction.evaluate(el => {
      window.addEventListener('keydown', event => {
        if (event.target === el && event.key === 'Enter') {
          el.dataset.enterCancelled = String(event.defaultPrevented);
        }
      });
    });
    await instruction.press(`${modifier}+Enter`);
    assert.equal(await instruction.getAttribute('data-enter-cancelled'), 'false');
    assert((await instruction.inputValue()).startsWith('Separate instruction'));
    assert.equal(await requestField.inputValue(), requestId);
    await editor.getByRole('button', { name: 'Cancel', exact: true }).click();
    assert.deepEqual([...posted, ...editorPosts], [], 'shortcut on the Editor must not submit either form');
    await writeFile(release, '');
    await editorPage.close();
    console.log('ASK_KEYBOARD_EDITOR_PASSED');

    const send = panel.getByRole('button', { name: 'Send answer', exact: true });
    const holds = [500, 200].map(status => ({
      status, gate: Promise.withResolvers(), arrived: Promise.withResolvers(),
    }));
    let attempt = 0;
    const delayAnswer = async route => {
      const hold = isAnswer(route.request()) && holds[attempt++];
      if (hold) {
        hold.arrived.resolve();
        await hold.gate.promise;
        if (hold.status === 500) return route.fulfill({ status: 500 });
      }
      await route.continue();
    };
    await page.route(endpoint, delayAnswer);
    async function submitWithCompetingShortcut(hold) {
      const result = page.waitForResponse(response => isAnswer(response.request()), { timeout: 8000 });
      result.catch(() => {});
      await answer.press(`${modifier}+Enter`);
      await Promise.race([hold.arrived.promise, result]);
      assert.equal(await answer.isDisabled(), true, 'answer is disabled during POST');
      assert.equal(await send.isDisabled(), true);
      if (hold.status === 500) await screenshot('in-flight');
      // Explicitly deliver the repeat to the original target even if disabling it moved focus.
      await answer.dispatchEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true, repeat: true,
        metaKey: modifier === 'Meta', ctrlKey: modifier === 'Control' });
      hold.gate.resolve();
      return result;
    }
    async function expectNoAnswerPost(message) {
      await page.waitForRequest(isAnswer, { timeout: 500 }).then(
        () => assert.fail(message),
        error => { if (!(error instanceof errors.TimeoutError)) throw error; },
      );
    }
    try {
      const text = 'First line\nSecond line';
      await answer.fill(text);
      const failure = await submitWithCompetingShortcut(holds[0]);
      assert.equal(failure.status(), 500);
      await answer.and(page.locator(':enabled')).waitFor();
      await send.and(page.locator(':enabled')).waitFor();
      assert.equal(await answer.inputValue(), text);
      assert.equal(await requestField.inputValue(), requestId);
      await expectNoAnswerPost('a busy shortcut replayed before the explicit retry');
      assert.deepEqual(posted, [endpoint], 'only the failed attempt may post before the explicit retry');

      const response = await submitWithCompetingShortcut(holds[1]);
      assert.equal(response.status(), 200);
      const fields = new URLSearchParams(response.request().postData());
      assert.equal(fields.get('requestId'), requestId);
      assert.equal(fields.get('answer'), text);
      await panel.waitFor({ state: 'detached' });
      await expectNoAnswerPost('an extra answer POST was queued');
      assert.deepEqual(posted, [endpoint, endpoint]);
      await page.locator(`#session-header-${id}`).getByText('Completed', { exact: true }).waitFor();
      await page.getByText(/\[stdout\] Use this answer: First line/).last().waitFor();
      await page.getByText(/\[stdout\] Second line/).last().waitFor();
      await screenshot('completed');
      console.log('ASK_KEYBOARD_SUBMISSION_PASSED');
    } finally {
      for (const hold of holds) hold.gate.resolve();
      await page.unrouteAll({ behavior: 'wait' });
    }
  }
} catch (error) {
  console.error(error, '\nProduction server stderr:', logs);
  process.exitCode = 1;
} finally {
  try {
    if (server) server.kill('SIGKILL');
    await browser?.close();
  } finally {
    await serverClosed;
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    clearTimeout(watchdog);
  }
}
