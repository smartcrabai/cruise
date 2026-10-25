// Mutate copied production assets only. An expected assertion, not any crash, must kill each mutant.
import assert from 'node:assert/strict';
import { cp, mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const root = await mkdtemp(join(tmpdir(), 'cruise-webui-negative-'));
try {
  const undefinedResult = spawnSync('cargo', ['+1.99.1', 'test', '--locked', '--all-features', '--test', 'webui_responsive_bdd'], {
    encoding: 'utf8', timeout: 120000,
    env: { ...process.env, WEBUI_BDD_FEATURE: resolve('tests/webui/undefined-control.feature') },
  });
  assert(undefinedResult.status !== 0, 'undefined Cucumber steps must fail');
  assert((undefinedResult.stdout + undefinedResult.stderr).includes("Step doesn't match any function"),
    'undefined control must fail because its step is undefined, not because compilation failed');
  console.log('KILLED: undefined Cucumber step by fail_on_skipped');
  for (const mutation of [
    { name: 'missing Escape focus restoration', contract: 'drawer', file: 'static/app.js',
      change: text => { assert(text.includes('toggle.focus();')); return text.replace('toggle.focus();', '/* negative control: no focus restoration */'); },
      expected: 'Escape restores toggle focus' },
    { name: 'mobile undersized controls', contract: 'layout', file: 'static/app.css',
      change: text => text + '\n@media (width < 48rem) { #main input, #main textarea, #main select { font-size: 14px !important; } }\n',
      expected: 'R8 mobile text controls avoid iOS zoom' },
  ]) {
    const assets = join(root, mutation.contract);
    await cp(resolve('webui'), assets, { recursive: true });
    const file = join(assets, mutation.file);
    await writeFile(file, mutation.change(await readFile(file, 'utf8')));
    const result = spawnSync(process.execPath, ['tests/webui/browser.mjs', mutation.contract, '375', '667'], {
      env: { ...process.env, WEBUI_BDD_ASSETS: assets }, encoding: 'utf8', timeout: 100000,
    });
    const output = result.stdout + result.stderr;
    assert(result.status !== 0, `surviving mutant: ${mutation.name}`);
    assert(output.includes(mutation.expected), `wrong failure for ${mutation.name}: ${output}`);
    console.log(`KILLED: ${mutation.name} by ${mutation.expected}`);
  }
} finally {
  await rm(root, { recursive: true, force: true, maxRetries: 5 });
}
