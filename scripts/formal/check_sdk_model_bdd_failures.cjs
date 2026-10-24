// Exercise the real SDK Cucumber runner, not just feature parsing. A compiler,
// platform, timeout or fixture-launch failure cannot satisfy these controls.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const output = process.argv[2];
if (!output) throw Error('Usage: check_sdk_model_bdd_failures.cjs report-directory');
fs.mkdirSync(output, { recursive: true });
for (const [name, marker] of [
  ['undefined', 'this deliberately undefined SDK model contract step'],
  ['assertion', 'SDK subprocesses ran in order jcode claude jcode'],
]) {
  const args = ['+1.99.1', 'test', '--locked', '--all-features', '--test',
    'sdk_model_bdd', '--', '--color', 'never', '--input',
    `tests/fixtures/sdk_model_specs/${name}.feature`];
  console.log(`Negative control argv: ${JSON.stringify(['cargo', ...args])}`);
  const result = spawnSync('cargo', args, { encoding: 'utf8', timeout: 120000 });
  const text = `${result.stdout ?? ''}${result.stderr ?? ''}`;
  fs.writeFileSync(path.join(output, `bdd-negative-${name}.log`), text);
  if (result.error || result.signal || result.status !== 101
      || !text.includes('1 step failed') || !text.includes(marker)
      || text.includes('Retry attempt:')) {
    console.error(text);
    throw Error(`${name}: expected one rejected step, no scenario rerun, exit 101; got ${result.status}`);
  }
  console.log(`${name}: one rejected step, no scenario rerun, exit 101 confirmed`);
}
// A filtered-out suite must fail too, rather than silently passing zero checks.
const args = ['+1.99.1', 'test', '--locked', '--all-features', '--test',
  'sdk_model_bdd', '--', '--color', 'never', '--name', '^NO_SUCH_SDK_MODEL_SCENARIO$'];
console.log(`Empty selection argv: ${JSON.stringify(['cargo', ...args])}`);
const result = spawnSync('cargo', args, { encoding: 'utf8', timeout: 120000 });
const text = `${result.stdout ?? ''}${result.stderr ?? ''}`;
fs.writeFileSync(path.join(output, 'bdd-negative-empty.log'), text);
if (result.error || result.signal || result.status !== 101
    || !text.includes('no SDK model scenarios executed')) {
  console.error(text);
  throw Error(`Empty selection: expected executed-step guard and exit 101; got ${result.status}`);
}
console.log('empty selection: executed-step guard and exit 101 confirmed');
