// B-3: removing the real Tell button must fail the same browser contract.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cp, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const root = await mkdtemp(join(tmpdir(), 'cruise-tell-negative-'));
try {
  const assets = join(root, 'webui');
  await cp(resolve('webui'), assets, { recursive: true });
  const header = join(assets, 'templates/components/session-header.tsx');
  const source = await readFile(header, 'utf8');
  assert(source.includes('props.actions.showTell &&'), 'mutation target must exist');
  await writeFile(header, source.replace('props.actions.showTell &&', 'false &&'));
  const result = spawnSync(process.execPath, ['tests/webui/tell-dialog.mjs'], {
    encoding: 'utf8', timeout: 110000,
    env: { ...process.env, WEBUI_BDD_ASSETS: assets, WEBUI_TELL_REPORT_DIR: join(root, 'screenshots') },
  });
  const output = result.stdout + result.stderr;
  assert.notEqual(result.status, 0, 'missing Tell button must fail');
  assert(output.includes("getByRole('button', { name: 'Tell', exact: true })") && output.includes('Timeout'),
    `expected missing Tell button assertion, not an unrelated failure: ${output}`);
  console.log('KILLED: missing Tell button by B-1 browser contract');
} finally {
  await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
