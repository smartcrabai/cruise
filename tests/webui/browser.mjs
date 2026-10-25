import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { chromium } from 'playwright';

const [contract, rawWidth, rawHeight] = process.argv.slice(2);
const width = Number(rawWidth), height = Number(rawHeight);
assert(['drawer', 'navigation', 'desktop', 'layout', 'dialogs', 'runall'].includes(contract), `undefined contract: ${contract}`);
assert(width > 0 && height > 0);
const root = await mkdtemp(join(tmpdir(), 'cruise-responsive-bdd-'));
const id = '20261025000000001';
const sessionDir = join(root, 'data/cruise/sessions', id);
await mkdir(sessionDir, { recursive: true });
await mkdir(join(root, 'repo'), { recursive: true });
const recentDir = '/Users/responsive/projects/recent-directory-with-a-wide-control';
await mkdir(join(root, 'state/cruise'), { recursive: true });
await writeFile(join(root, 'state/cruise/history.json'), JSON.stringify({ entries: [
  { working_dir: recentDir, resolved_config_key: '__builtin__' },
] }));
// Durable inputs exercise the normal session loader, reconciler and Markdown renderer.
const session = {
  id, base_dir: join(root, 'repo', 'long-repository-name-'.repeat(12)), phase: 'Planned',
  config: { kind: 'inline_snapshot' }, input: 'Responsive input with long content. '.repeat(75),
  title: 'responsive-'.repeat(24), current_step: null, created_at: '2026-10-25T00:00:00Z',
  completed_at: null, worktree_path: null, worktree_branch: 'feature/'.repeat(30),
};
await writeFile(join(sessionDir, 'state.json'), JSON.stringify(session));
await writeFile(join(sessionDir, 'config.yaml'), 'steps:\n  harmless:\n    command: "true"\n');
await writeFile(join(sessionDir, 'plan.md'), '# Responsive plan\n\n' + 'longword'.repeat(150) + '\n\n| Wide | Table |\n| --- | --- |\n| ' + 'column'.repeat(120) + ' | second |\n\n```text\n' + 'code'.repeat(200) + '\n```\n\n' + 'Paragraph for scrolling.\n\n'.repeat(50));
await writeFile(join(sessionDir, 'run.log'), Array.from({ length: 300 }, (_, i) => `line ${i} saved production log`).join('\n'));
const mergeId = '20261025000000002';
const bin = join(root, 'bin');
await mkdir(bin);
// A strict external-process boundary fixture, never a mocked HTTP/DOM renderer.
// Preview must not execute gh pr merge or reach a real forge/account.
const forgeLog = join(root, 'forge.log');
const forgeJson = join(root, 'forge.json');
await writeFile(forgeJson, JSON.stringify({ state: 'OPEN', mergeable: 'MERGEABLE', reviewDecision: 'APPROVED',
  statusCheckRollup: Array.from({ length: 50 }, (_, i) => ({ __typename: 'CheckRun',
    name: `responsive integration check ${i}`, status: 'COMPLETED', conclusion: 'SUCCESS' })) }));
await writeFile(join(bin, 'gh'), `#!/bin/sh\nprintf '%s\\n' "$*" >> ${JSON.stringify(forgeLog)}\nif [ "$1" = pr ] && [ "$2" = view ]; then cat ${JSON.stringify(forgeJson)}; else echo 'unexpected forge mutation' >&2; exit 71; fi\n`, { mode: 0o755 });
if (contract === 'dialogs') {
  const dir = join(root, 'data/cruise/sessions', mergeId);
  await mkdir(dir);
  await writeFile(join(dir, 'state.json'), JSON.stringify({ ...session, id: mergeId,
    phase: 'Completed', title: 'Completed preview fixture', pr_url: 'https://github.com/fixture/responsive/pull/8' }));
  await writeFile(join(dir, 'config.yaml'), 'steps:\n  harmless:\n    command: "true"\n');
}
if (contract === 'runall') {
  const repo = join(root, 'repo');
  for (const args of [
    ['init', '-b', 'main'],
    ['-c', 'user.name=Responsive BDD', '-c', 'user.email=responsive-bdd@example.invalid', 'commit', '--allow-empty', '-m', 'isolated live-log fixture'],
  ]) {
    const result = spawnSync('git', args, { cwd: repo, encoding: 'utf8',
      env: { PATH: process.env.PATH, HOME: root, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' } });
    assert.equal(result.status, 0, `temporary live-log repository: ${result.stderr}`);
  }
  const command = 'i=0; while [ "$i" -lt 40 ]; do printf "Live responsive log line %s\\n" "$i"; i=$((i + 1)); done; sleep 0.1';
  for (let i = 1; i <= 21; i++) {
    const batchId = `20261025000000${String(i).padStart(3, '0')}`;
    const dir = join(root, 'data/cruise/sessions', batchId);
    await mkdir(dir, { recursive: true });
    await writeFile(join(dir, 'state.json'), JSON.stringify({ ...session, id: batchId,
      base_dir: repo, worktree_branch: null, workspace_mode: 'CurrentBranch', target_branch: 'main',
      allow_dirty_working_tree: true }));
    await writeFile(join(dir, 'config.yaml'), `create_pr: false\nsteps:\n  log:\n    command: ${JSON.stringify(command)}\n`);
    await writeFile(join(dir, 'plan.md'), '# Local fixture plan\n');
  }
}
const env = { PATH: `${bin}:${process.env.PATH}`, HOME: root, XDG_CONFIG_HOME: join(root, 'config'),
  XDG_DATA_HOME: join(root, 'data'), XDG_STATE_HOME: join(root, 'state'),
  CRUISE_DISABLE_NOTIFICATIONS: '1', CRUISE_DISABLE_HERDR: '1', NO_COLOR: '1' };
const server = spawn(process.env.WEBUI_BDD_BIN || resolve('target/debug/cruise-runtime'),
  ['webui', '--port', '0', '--webui-dir', resolve(process.env.WEBUI_BDD_ASSETS || 'webui')],
  { env, cwd: join(root, 'repo'), stdio: ['ignore', 'pipe', 'pipe'] });
const serverClosed = new Promise(ok => server.once('close', ok));
let logs = '', browser, page;
server.stderr.on('data', data => { logs += data; });
const watchdog = setTimeout(() => { console.error('browser contract timed out', logs); server.kill('SIGKILL'); process.exit(1); }, 90000);
try {
  const base = await new Promise((ok, fail) => {
    let text = '';
    const timeout = setTimeout(() => fail(new Error(`server startup timeout: ${logs}`)), 20000);
    server.on('error', error => { clearTimeout(timeout); fail(error); });
    server.on('exit', code => { clearTimeout(timeout); fail(new Error(`server exited ${code}: ${logs}`)); });
    server.stdout.on('data', chunk => {
      text += chunk;
      const match = text.match(/http:\/\/127\.0\.0\.1:\d+/);
      if (match) { clearTimeout(timeout); ok(match[0]); }
    });
  });
  browser = await chromium.launch({ headless: true });
  console.log(`Chromium ${browser.version()}`);
  const context = await browser.newContext({ viewport: { width, height }, reducedMotion: 'reduce', colorScheme: 'light' });
  page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const goto = async path => {
    const response = await page.goto(base + path);
    assert.equal(response.status(), 200, `production page ${path}`);
    await page.waitForFunction(() => !!window.app);
  };
  const open = async () => {
    await page.locator('#sidebar-toggle').click();
    await state(true);
  };
  const state = async expected => {
    await page.waitForFunction(value => document.querySelector('#app').dataset.sidebarOpen === String(value), expected);
    assert.equal(await page.locator('#sidebar-toggle').getAttribute('aria-expanded'), String(expected));
    assert.equal(await page.locator('#sidebar-backdrop').isVisible(), expected);
    assert.equal(await page.locator('#sidebar').isVisible(), expected);
  };
  const noOverflow = async () => {
    const metrics = await page.evaluate(() => ({ width: innerWidth, doc: document.documentElement.scrollWidth, main: document.querySelector('#main').getBoundingClientRect().right }));
    assert(metrics.doc <= metrics.width + 1, `document overflow ${JSON.stringify(metrics)}`);
    assert(metrics.main <= metrics.width + 1, 'main exceeds viewport');
  };
  const within = async selector => {
    const box = await page.locator(selector).boundingBox();
    assert(box && box.x >= 0 && box.x + box.width <= width + 1 && box.y >= 0 && box.y + box.height <= height + 1,
      `${selector} must fit viewport: ${JSON.stringify(box)}`);
  };
  await goto(contract === 'dialogs' ? `/sessions/${mergeId}` : '/new');
  if (contract === 'drawer') {
    await state(false);
    for (let i = 0; i < 15; i++) {
      await page.keyboard.press('Tab');
      assert.equal(await page.evaluate(() => !!document.activeElement.closest('#sidebar')), false, 'closed drawer must not receive Tab focus');
    }
    await open();
    await within('#sidebar');
    assert.equal(await page.locator('#sidebar').evaluate(el => getComputedStyle(el).transitionDuration), '0s', 'reduced motion wins even while open');
    const lightHeader = await page.locator('#app > header').evaluate(el => getComputedStyle(el).backgroundColor);
    await page.emulateMedia({ colorScheme: 'dark' });
    assert.notEqual(await page.locator('#app > header').evaluate(el => getComputedStyle(el).backgroundColor), lightHeader, 'mobile header honors dark mode');
    await page.emulateMedia({ colorScheme: 'light' });
    await page.keyboard.press('Enter');
    await state(false);
    await open();
    await page.locator('#sidebar-backdrop').click({ position: { x: width - 30, y: height / 2 } });
    await state(false);
    await open();
    await page.locator('#sidebar [aria-label="Settings"]').focus();
    await page.keyboard.press('Escape');
    await state(false);
    assert.equal(await page.locator('#sidebar-toggle').evaluate(el => el === document.activeElement), true, 'Escape restores toggle focus');
    const recent = page.getByRole('button', { name: recentDir, exact: true });
    const directory = page.locator('#base-dir-input');
    await recent.click();
    assert.equal(await directory.inputValue(), `${recentDir}/`, 'closed drawer permits persisted recent-directory selection');
    await directory.fill('/unchanged-directory');
    await open();
    const recentBox = await recent.boundingBox();
    const sidebarBox = await page.locator('#sidebar').boundingBox();
    const outside = { x: sidebarBox.x + sidebarBox.width + 10, y: recentBox.y + recentBox.height / 2 };
    assert(outside.x < recentBox.x + recentBox.width && outside.x > recentBox.x, 'recent control overlaps exposed backdrop');
    assert.equal(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.id, outside), 'sidebar-backdrop', 'backdrop covers recent-directory controls');
    await page.mouse.click(outside.x, outside.y);
    await state(false);
    assert.equal(await directory.inputValue(), '/unchanged-directory', 'outside tap dismisses without picking a directory');
    await open();
    const row = page.locator(`#sidebar [hx-get="/sessions/${id}"]`);
    const rowBox = await row.boundingBox();
    const rowY = rowBox.y + rowBox.height / 2;
    await recent.evaluate((el, y) => {
      document.querySelector('#main').scrollTop += el.getBoundingClientRect().y + el.getBoundingClientRect().height / 2 - y;
    }, rowY);
    const coveredBox = await recent.boundingBox();
    const covered = { x: Math.max(coveredBox.x, rowBox.x) + 20, y: rowY };
    assert(covered.y >= coveredBox.y && covered.y < coveredBox.y + coveredBox.height, 'recent control overlaps sidebar row');
    assert.equal(await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest('#sidebar a')?.id, covered), `session-row-${id}`, 'drawer row covers recent-directory controls');
    await page.mouse.click(covered.x, covered.y);
    await page.locator('#session-detail').waitFor();
    await state(false);
    assert.equal(await page.locator('#session-detail').getAttribute('data-session-id'), id, 'overlapping row tap navigates');
  } else if (contract === 'navigation') {
    await open();
    const poll = page.waitForResponse(response => response.url().endsWith('/webui/sidebar') && response.status() === 200);
    await poll;
    await state(true);
    const runAll = page.locator('#run-all-control button');
    assert.equal(await runAll.isEnabled(), true, 'fixture must expose real runnable session');
    let posts = 0;
    page.on('request', request => { if (request.url().endsWith('/webui/run-all') && request.method() === 'POST') posts++; });
    page.once('dialog', dialog => dialog.dismiss());
    await runAll.click();
    await state(true);
    assert.equal(posts, 0, 'cancelled native confirmation must not send POST');
    await page.locator('#sidebar [aria-label="Settings"]').click();
    await page.getByRole('dialog').waitFor();
    await state(true);
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await page.locator(`#sidebar [hx-get="/sessions/${id}"]`).click();
    await page.locator('#session-detail').waitFor();
    await state(false);
    assert.equal(await page.locator('#session-detail').getAttribute('data-session-id'), id);
    await open();
    await page.getByRole('button', { name: '+ New', exact: true }).click();
    await page.locator('#new-session-form').waitFor();
    await state(false);
    // Accepting Run All is a real navigation, but this isolated shell workflow has no external effects.
    await open();
    page.once('dialog', dialog => dialog.accept());
    await runAll.click();
    await page.locator('#run-all-view').waitFor();
    await state(false);
    assert.equal(posts, 1);
  } else if (contract === 'desktop') {
    const sidebarWidth = () => page.locator('#sidebar').evaluate(el => el.getBoundingClientRect().width);
    assert.equal(await sidebarWidth(), 288);
    assert.equal(await page.locator('#sidebar').isVisible(), true, 'desktop sidebar remains visible');
    assert.equal(await page.locator('#sidebar-toggle').isVisible(), false);
    const resizer = page.locator('#sidebar-resizer');
    await resizer.focus();
    await page.keyboard.press('ArrowRight');
    assert.equal(await sidebarWidth(), 304);
    const box = await resizer.boundingBox();
    await page.mouse.move(box.x + box.width / 2, 150);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 80, 150);
    await page.mouse.up();
    assert.equal(await sidebarWidth(), 384);
    assert.equal(await page.evaluate(() => localStorage.getItem('cruise-sidebar-width')), '384');
    await page.reload();
    assert.equal(await sidebarWidth(), 384, 'reload restores stored desktop width');
    await page.setViewportSize({ width: 767, height });
    await state(false);
    await open();
    assert.equal(await sidebarWidth(), 288, 'mobile width overrides saved desktop width');
    await page.setViewportSize({ width: 768, height });
    await page.waitForFunction(() => document.querySelector('#app').dataset.sidebarOpen === 'false');
    assert.equal(await page.locator('#sidebar-toggle').isVisible(), false);
    assert.equal(await page.locator('#sidebar-backdrop').isVisible(), false);
    assert.equal(await sidebarWidth(), 384);
    await page.setViewportSize({ width: 767, height });
    await state(false);
    await page.setViewportSize({ width: 1280, height });
    for (const [stored, expected] of [['100', 180], ['700', 600], ['invalid', 288]]) {
      await page.evaluate(value => localStorage.setItem('cruise-sidebar-width', value), stored);
      await page.reload();
      await page.waitForFunction(() => !!window.app);
      assert.equal(await sidebarWidth(), expected, 'stored widths are clamped or defaulted');
    }
    await resizer.focus();
    for (let i = 0; i < 30; i++) await page.keyboard.press('ArrowLeft');
    assert.equal(await sidebarWidth(), 180);
    for (let i = 0; i < 30; i++) await page.keyboard.press('ArrowRight');
    assert.equal(await sidebarWidth(), 600);
    assert.equal(await page.evaluate(() => localStorage.getItem('cruise-sidebar-width')), '600');
    assert.notEqual(await page.locator('#sidebar').evaluate(el => getComputedStyle(el).position), 'fixed', 'desktop sidebar is not off-canvas');
  } else if (contract === 'dialogs') {
    await page.getByRole('button', { name: 'Merge PR', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await dialog.waitFor();
    await within('[role=dialog]');
    assert.equal(await dialog.locator('li').count(), 50, 'real preview renders every forge check');
    assert(await dialog.evaluate(el => el.scrollHeight > el.clientHeight && getComputedStyle(el).overflowY === 'auto'), 'R6 tall merge dialog scrolls internally');
    await dialog.getByRole('button', { name: 'Confirm merge', exact: true }).scrollIntoViewIfNeeded();
    await within('[role=dialog] button:last-child');
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    const forgeCalls = (await readFile(forgeLog, 'utf8')).split('\n').filter(Boolean);
    assert(forgeCalls.every(line => line.startsWith('pr view ')), `preview never mutates a forge: ${JSON.stringify(forgeCalls)}`);
    await goto(`/sessions/${id}`);
    await page.getByRole('button', { name: 'Publish as Issue', exact: true }).click();
    await page.getByRole('dialog').waitFor();
    await within('[role=dialog]');
    assert.equal(await page.getByRole('dialog').evaluate(el => getComputedStyle(el).overflowY), 'auto');
    await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
    await open();
    await page.locator('#sidebar [aria-label="Settings"]').click();
    await page.getByRole('dialog').waitFor();
    await within('[role=dialog]');
    await noOverflow();
  } else if (contract === 'runall') {
    await open();
    page.once('dialog', dialog => dialog.accept());
    await page.locator('#run-all-control button').click();
    await page.locator('#run-all-view').waitFor();
    await state(false);
    // Real shell steps emit log chunks and complete only in the isolated repo.
    // The UI must keep every result reachable, including the final summary.
    await page.waitForFunction(() => document.querySelector('#run-all-results')?.children.length >= 21, null, { timeout: 30000 });
    const phases = await page.locator('#run-all-results > div > span').allTextContents();
    assert.equal(phases.length, 21, 'every real batch session must produce a result');
    assert(phases.every(phase => phase === 'Completed'), `live-log workflows must complete: ${JSON.stringify(phases)}`);
    const liveLog = await page.locator('#run-all-log').evaluate(el => ({
      height: el.clientHeight, total: el.scrollHeight, top: el.scrollTop,
    }));
    assert(liveLog.total > liveLog.height, 'fixture must produce a genuinely scrolling live log');
    assert(liveLog.total - liveLog.top - liveLog.height < 24, 'R5 live log keeps following production SSE swaps');
    assert(await page.locator('#main').evaluate(el => el.scrollHeight > el.clientHeight), 'R5 Run All scrolls main on mobile');
    assert.equal(await page.locator('#run-all-results').evaluate(el => getComputedStyle(el).overflowY), 'visible', 'mobile results do not become a second constrained scroll panel');
    await page.locator('#run-all-results > div').last().scrollIntoViewIfNeeded();
    await noOverflow();
    await page.setViewportSize({ width: 1280, height: 800 });
    assert.equal(await page.locator('#run-all-results').evaluate(el => getComputedStyle(el).overflowY), 'auto', 'desktop preserves internal results scrolling');
  } else {
    await noOverflow();
    const appBox = await page.locator('#app').boundingBox();
    assert(Math.abs(appBox.height - height) <= 1, 'R3 app follows dynamic viewport height');
    const inputs = page.locator('#main input:not([type=radio]):not([type=checkbox]):not([type=file]), #main textarea, #main select');
    assert(await inputs.count() >= 3, 'exercise real form controls');
    for (const input of await inputs.all()) {
      if (await input.isVisible()) assert(await input.evaluate(el => parseFloat(getComputedStyle(el).fontSize)) >= 16, 'R8 mobile text controls avoid iOS zoom');
    }
    await goto(`/sessions/${id}`);
    await page.locator('#session-detail').waitFor();
    await noOverflow();
    assert(await page.locator('#main').evaluate(el => el.scrollHeight > el.clientHeight), 'R5 long header must scroll main, not crush panel');
    await page.locator(`#session-header-${id}`).getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.locator(`#session-settings-${id}`);
    await settings.waitFor();
    for (const select of await settings.locator('select').all()) {
      assert.equal(await select.evaluate(el => getComputedStyle(el).fontSize), '16px', 'R8 session settings selects use mobile font size');
    }
    await noOverflow();
    await settings.getByRole('button', { name: 'Close', exact: true }).click();
    await page.getByRole('button', { name: 'Replan', exact: true }).click();
    const editor = page.locator(`#editor-${id} textarea`);
    await editor.waitFor();
    assert.equal(await editor.evaluate(el => getComputedStyle(el).fontSize), '16px', 'R8 editor uses mobile font size');
    await page.locator(`#editor-${id}`).getByRole('button', { name: 'Cancel', exact: true }).click();
    await page.getByRole('tab', { name: 'Info', exact: true }).scrollIntoViewIfNeeded();
    await within('[role=tablist]');
    await page.getByRole('tab', { name: 'Plan', exact: true }).click();
    await page.locator('.prose table').waitFor();
    await noOverflow();
    assert.equal(await page.locator('.prose table').evaluate(el => getComputedStyle(el).overflowX), 'auto');
    assert(await page.locator('.prose table').evaluate(el => el.scrollWidth > el.clientWidth), 'R9 wide table scrolls locally');
    await page.locator('.prose table').evaluate(el => { el.scrollLeft = el.scrollWidth; });
    assert(await page.locator('.prose table').evaluate(el => el.scrollLeft > 0));
    await page.getByRole('tab', { name: 'Log', exact: true }).click();
    await page.locator(`#tab-log-${id}`).waitFor();
    assert(await page.locator(`#tab-log-${id}`).evaluate(el => el.clientHeight <= innerHeight * .7 + 1 && el.scrollHeight > el.clientHeight), 'R5 log capped with internal scrolling');
    await page.getByRole('button', { name: 'Publish as Issue', exact: true }).click();
    await page.getByRole('dialog').waitFor();
    await within('[role=dialog]');
    await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
    await open();
    await page.locator('#sidebar [aria-label="Settings"]').click();
    await page.getByRole('dialog').waitFor();
    await within('[role=dialog]');
    assert.equal(await page.locator('#run-all-parallelism').evaluate(el => getComputedStyle(el).fontSize), '16px');
    assert(await page.getByRole('dialog').evaluate(el => {
      const box = el.getBoundingClientRect();
      return el.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
    }), 'settings dialog stays above the open drawer');
    await page.locator('#run-all-parallelism').fill('0');
    // Native validation is bypassed only to exercise the real server error/toast response.
    await page.locator('[role=dialog]').evaluate(el => { el.noValidate = true; });
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await page.locator('#toasts > *').first().waitFor();
    await within('#toasts');
    assert(await page.locator('#toasts > *').first().evaluate(el => {
      const box = el.getBoundingClientRect();
      return el.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
    }), 'toast stays above the drawer and dialog backdrop');
    await noOverflow();
  }
  assert.deepEqual(errors, [], 'production JavaScript must not throw');
  console.log(`WEBUI_BROWSER_CONTRACT_PASSED ${contract} ${width}x${height}`);
} catch (error) {
  console.error(error, '\nProduction server stderr:', logs);
  if (page && !page.isClosed()) console.error(await page.evaluate(() => ['#sidebar-backdrop', '#sidebar', '#app'].map(s => { const el = document.querySelector(s), r = el.getBoundingClientRect(), c = getComputedStyle(el); return { s, x:r.x,y:r.y,width:r.width,height:r.height, position:c.position, inset:c.inset, hit:document.elementFromPoint(innerWidth-10,innerHeight/2)?.outerHTML.slice(0,150) }; })).catch(() => 'page unavailable'));
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
  try {
    await browser?.close();
  } finally {
    try {
      server.kill('SIGKILL');
      await serverClosed;
    } finally {
      await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}
