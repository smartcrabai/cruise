// Production flag editors with real Chromium, HTMax, and isolated session data.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { chromium } from 'playwright';
import { expect } from 'playwright/test';
import { loginWebUi, waitForWebUiLogin } from './auth.mjs';

const root = await mkdtemp(join(tmpdir(), 'cruise-flag-buttons-'));
const repo = join(root, 'repo');
const workflows = join(root, 'config/cruise/workflows');
const sessions = join(root, 'data/cruise/sessions');
const workflow = join(workflows, 'flag-buttons.yaml');
const replacement = join(workflows, 'replacement.yaml');
const plain = join(workflows, 'no-flags.yaml');
const alpha = 'alpha_flag_with_a_long_name_that_must_wrap_on_a_narrow_screen';
const beta = 'beta';
const description = 'Include the optional alpha step';
const preview = '/webui/new/steps/flags';
const reset = '/webui/new/steps';
const reports = resolve(process.env.WEBUI_FLAG_BUTTONS_REPORT_DIR || 'target/formal-reports/flag-buttons-browser');
const pathname = request => new URL(request.url()).pathname;
const env = { PATH: process.env.PATH, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', CRUISE_UI_LANGUAGE: 'en',
  NO_COLOR: '1', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
let server, closed, browser, logs = '';
const watchdog = setTimeout(() => {
  console.error('flag buttons browser watchdog expired', logs);
  server?.kill('SIGKILL');
  process.exit(1);
}, 90000);

try {
  await Promise.all([repo, workflows, sessions, reports].map(dir => mkdir(dir, { recursive: true })));
  await writeFile(workflow, `flags:
  ${alpha}: {description: '${description}'}
  beta: {}
steps:
  keep: {command: 'true'}
  omit: {command: 'true'}
  alpha_step: {command: 'true', flag: ${alpha}}
  beta_step: {command: 'true', flag: beta}
`);
  await writeFile(replacement, `flags:
  replacement_flag: {}
steps:
  replacement_step: {command: 'true'}
`);
  await writeFile(plain, "steps:\n  plain_step: {command: 'true'}\n");
  server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
    ['daemon', 'serve', '--port', '0', '--webui-dir', resolve('webui')],
    { cwd: repo, env, stdio: ['ignore', 'pipe', 'pipe'] });
  closed = new Promise(ok => server.once('close', ok));
  server.stderr.on('data', data => { logs += data; });
  const { base, loginUrl } = await waitForWebUiLogin(server, () => logs);
  const browserEnv = { ...process.env };
  for (const key of ['DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH']) delete browserEnv[key];
  browser = await chromium.launch({ headless: true, env: browserEnv });
  console.log(`Chromium ${browser.version()}`);
  const page = await browser.newPage({ viewport: { width: 360, height: 800 }, reducedMotion: 'reduce' });
  page.setDefaultTimeout(10000);
  const errors = [], editorRequests = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => {
    const { origin, pathname } = new URL(request.url());
    if (origin === base && (pathname === '/webui' || pathname.startsWith('/webui/') || request.method() !== 'GET')) {
      editorRequests.push(request);
    }
  });
  await loginWebUi(page, loginUrl);
  assert.equal((await page.goto(`${base}/new`)).status(), 200);
  await page.waitForFunction(() => !!window.app);
  const form = page.locator('#new-session-form');
  const tree = page.locator('#step-tree');
  const toggle = name => tree.getByRole('button', { name, exact: true });
  const wire = name => tree.locator(`input[type="hidden"][name="flag.${name}"]`);
  const step = id => tree.locator(`input[name="runSteps"][value="${id}"]`);
  const screenshot = async (name, subject = tree) => {
    const path = join(reports, `${name}.png`);
    await subject.screenshot({ path, animations: 'disabled' });
    console.log(`screenshot ${path}`);
  };
  const buttons = (form, names) => names.map(name => form.getByRole('button', { name, exact: true }));
  const createButtons = buttons(form, ['Save draft', 'Create draft', 'Create & plan']);
  const responseFor = (path, method = 'GET') => {
    const response = page.waitForResponse(r => pathname(r) === path && r.request().method() === method);
    response.catch(() => {});
    return response;
  };
  const assertState = async (name, on) => {
    await expect(toggle(name), `${name} must be a single accessible workflow toggle`).toHaveCount(1);
    await expect(toggle(name)).toHaveAttribute('aria-pressed', String(on));
    await expect(toggle(name)).toContainText(on ? /\bon\b/i : /\boff\b/i);
    await expect(wire(name)).toHaveCount(1);
    await expect(wire(name)).toHaveValue(on ? 'on' : 'off');
  };
  const picker = page.locator('#config-path');
  const choose = async path => {
    const response = responseFor(reset);
    await picker.selectOption(path);
    assert.equal((await response).status(), 200);
  };
  const checkPreview = (request, a, b) => {
    assert.equal(request.method(), 'GET');
    const params = new URL(request.url()).searchParams;
    assert.deepEqual(params.getAll(`flag.${alpha}`), [a]);
    assert.deepEqual(params.getAll(`flag.${beta}`), [b]);
    assert(params.getAll('stepIds').includes('omit'));
    assert(params.getAll('runSteps').includes('keep'));
    assert(!params.getAll('runSteps').includes('omit'));
  };
  const flip = async (name, on, a, b, action) => {
    const start = editorRequests.length;
    const response = responseFor(preview);
    await action(toggle(name));
    const result = await response;
    assert.equal(result.status(), 200);
    await assertState(name, on);
    await expect(toggle(name)).toBeFocused();
    checkPreview(result.request(), a, b);
    assert.deepEqual(editorRequests.slice(start).map(r => [r.method(), pathname(r)]), [['GET', preview]]);
    await expect(step('keep')).toBeChecked();
    await expect(step('omit')).not.toBeChecked();
  };
  // Hold a genuine server response, not a mocked fragment. The test controls only delivery.
  const holdResponse = async (path = preview) => {
    let release, ready, done, fail, arrivalTimer;
    const gate = new Promise(resolve => { release = resolve; });
    const received = new Promise((resolve, reject) => { ready = resolve; fail = reject; });
    received.catch(() => {});
    const finished = new Promise(resolve => { done = resolve; });
    await page.route(url => url.pathname === path, async route => {
      clearTimeout(arrivalTimer);
      try {
        const response = await route.fetch({ timeout: 10000 });
        assert.equal(response.status(), 200);
        ready(route.request());
        await gate;
        await route.fulfill({ response });
      } catch (error) {
        fail(error);
      } finally {
        done();
      }
    }, { times: 1 });
    arrivalTimer = setTimeout(() => fail(new Error(`Expected request to ${path} did not arrive`)), 10000);
    return { received, release, finished };
  };

  await choose(workflow);
  await expect(step('keep')).toBeChecked();
  await assertState(alpha, false);
  await assertState(beta, false);
  await expect(toggle(alpha)).toHaveAccessibleDescription(description);
  await expect(step('alpha_step')).toHaveCount(0);
  await expect(step('beta_step')).toHaveCount(0);
  await step('omit').uncheck();
  for (const scheme of ['light', 'dark']) {
    await page.emulateMedia({ colorScheme: scheme });
    await expect(toggle(alpha)).toBeVisible();
    assert(await toggle(alpha).evaluate(el => {
      const rect = el.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth && el.scrollWidth <= el.clientWidth;
    }), `long toggle must fit the narrow ${scheme} viewport without clipping`);
    await screenshot(`new-session-${scheme}`);
  }
  await flip(alpha, true, 'on', 'off', control => control.click());
  await expect(step('alpha_step')).toBeChecked();
  await assertState(beta, false);
  await flip(beta, true, 'on', 'on', async control => { await control.focus(); await page.keyboard.press('Space'); });
  await expect(step('alpha_step')).toBeChecked();
  await expect(step('beta_step')).toBeChecked();

  const delayed = await holdResponse();
  const delayedStart = editorRequests.length;
  const delayedResponse = responseFor(preview);
  try {
    await toggle(beta).click();
    checkPreview(await delayed.received, 'on', 'off');
    await assertState(beta, true);
    await expect(toggle(beta)).toBeEnabled();
    await expect(toggle(beta)).toBeFocused();
    for (const button of createButtons) await expect(button).toBeDisabled();
    for (const id of ['keep', 'omit', 'alpha_step', 'beta_step']) await expect(step(id)).toBeDisabled();
    for (const name of [alpha, beta]) await expect(wire(name)).toBeEnabled();
    await screenshot('new-session-preview-pending');
    await page.keyboard.press('Enter');
  } finally {
    delayed.release();
  }
  assert.equal((await delayedResponse).status(), 200);
  await delayed.finished;
  await assertState(beta, false);
  await expect(toggle(beta)).toBeFocused();
  await expect(step('beta_step')).toHaveCount(0);
  for (const button of createButtons) await expect(button).toBeEnabled();
  assert.deepEqual(editorRequests.slice(delayedStart).map(r => [r.method(), pathname(r)]), [['GET', preview]], 'overlapping activation must be dropped');
  assert.deepEqual(await readdir(sessions), [], 'previews must not create a session');

  const superseded = await holdResponse();
  const waitingReset = await holdResponse(reset);
  await page.evaluate(({ preview, reset, replacement }) => {
    window.supersededPreviewCleanedUp = false;
    window.replacementResetFinished = false;
    document.addEventListener('htmx:finally:request', event => {
      const request = event.detail.ctx.request;
      const url = new URL(request.action, location.href);
      if (url.pathname === preview && request.signal.aborted) {
        window.supersededPreviewCleanedUp = true;
      }
      if (url.pathname === reset && url.searchParams.get('configPath') === replacement) {
        window.replacementResetFinished = true;
      }
    });
  }, { preview, reset, replacement });
  await toggle(beta).click();
  await superseded.received;
  try {
    await picker.selectOption(replacement);
    await waitingReset.received;
    await page.waitForFunction(() => window.supersededPreviewCleanedUp);
    await expect(toggle(beta)).toBeDisabled();
    await choose(plain);
    await expect(step('plain_step')).toBeChecked();
  } finally {
    waitingReset.release();
    superseded.release();
  }
  await Promise.all([waitingReset.finished, superseded.finished]);
  await page.waitForFunction(() => window.replacementResetFinished);
  await expect(picker).toHaveValue(plain);
  await expect(step('plain_step')).toBeChecked();
  await expect(tree.locator('button[aria-pressed]')).toHaveCount(0);
  await expect(form.locator('[name^="flag."], [name="runSteps"][value="replacement_step"], [name="stepIds"][value="replacement_step"]')).toHaveCount(0);
  for (const button of createButtons) await expect(button).toBeEnabled();
  await screenshot('new-session-no-flags');

  await choose(workflow);
  await assertState(alpha, false);
  await assertState(beta, false);
  const directoryPreview = await holdResponse();
  await toggle(alpha).click();
  await directoryPreview.received;
  try {
    const directoryReset = responseFor(reset);
    await form.locator('#base-dir-input').fill(repo);
    await form.locator('#base-dir-input').press('Tab');
    assert.equal((await directoryReset).status(), 200);
  } finally {
    directoryPreview.release();
  }
  await directoryPreview.finished;
  await assertState(alpha, false);
  await expect(step('alpha_step')).toHaveCount(0);
  await step('omit').uncheck();
  await flip(alpha, true, 'on', 'off', control => control.click());
  await form.locator('#task-input').fill('Flag button draft fixture');
  const created = responseFor('/webui/sessions/draft', 'POST');
  await createButtons[1].click();
  assert.equal((await created).status(), 200);
  await page.waitForURL(/\/sessions\/[^/]+$/);
  const id = new URL(page.url()).pathname.split('/').at(-1);
  const readState = async () => JSON.parse(await readFile(join(sessions, id, 'state.json'), 'utf8'));
  let state = await readState();
  assert.equal(state.phase, 'Draft');
  assert.deepEqual(state.flags, { [alpha]: true, beta: false });
  assert.deepEqual(state.skipped_steps, ['omit']);

  const settings = page.locator(`#session-settings-${id}`);
  const settingsButtons = buttons(settings, ['Save', 'Save & regenerate']);
  const openSettings = async () => {
    await page.locator(`#session-header-${id}`).getByRole('button', { name: 'Settings', exact: true }).click();
    await settings.waitFor();
  };
  await openSettings();
  await assertState(alpha, true);
  await assertState(beta, false);
  await expect(step('omit')).not.toBeChecked();
  assert.equal(await settings.locator('select[name="currentStep"] option[value="beta_step"]').count(), 1);
  const pendingSave = await holdResponse();
  const settingsPreview = responseFor(preview);
  try {
    await toggle(beta).click();
    checkPreview(await pendingSave.received, 'on', 'on');
    for (const button of settingsButtons) await expect(button).toBeDisabled();
    await expect(step('omit')).toBeDisabled();
    assert.deepEqual((await readState()).flags, state.flags, 'preview must not persist Settings');
  } finally {
    pendingSave.release();
  }
  assert.equal((await settingsPreview).status(), 200);
  await pendingSave.finished;
  await assertState(beta, true);
  await expect(toggle(beta)).toBeFocused();
  await expect(step('beta_step')).toBeChecked();
  await flip(alpha, false, 'off', 'on', async control => { await control.focus(); await page.keyboard.press('Enter'); });
  await expect(step('alpha_step')).toHaveCount(0);
  await step('keep').uncheck();
  await step('omit').check();
  assert.deepEqual((await readState()).flags, state.flags, 'completed previews must not persist Settings');
  const saved = responseFor(`/webui/sessions/${id}/settings`, 'POST');
  await settingsButtons[0].click();
  assert.equal((await saved).status(), 200);
  await settings.waitFor({ state: 'detached' });
  state = await readState();
  assert.equal(state.phase, 'Draft');
  assert.deepEqual(state.flags, { [alpha]: false, beta: true });
  assert.deepEqual(state.skipped_steps, ['keep']);
  await page.reload();
  await openSettings();
  await assertState(alpha, false);
  await assertState(beta, true);
  await expect(step('omit')).toBeChecked();
  await expect(step('keep')).not.toBeChecked();
  await expect(step('beta_step')).toBeChecked();
  assert.deepEqual(editorRequests.filter(r => r.method() !== 'GET').map(pathname),
    ['/webui/sessions/draft', `/webui/sessions/${id}/settings`], 'only the two explicit save actions may mutate state');
  await screenshot('settings-reopened', settings);
  assert.deepEqual(errors, []);
  console.log('workflow flag toggles, preview coordination, and persistence: passed');
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
