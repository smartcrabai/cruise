// Run the actual Cucumber executable with deliberate negative controls.
// A build/platform failure is not evidence that a step assertion was rejected.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const output = process.argv[2];
if (!output) throw Error('Usage: check_bdd_failures.cjs report-directory');
fs.mkdirSync(output, { recursive: true });
for (const [name, marker] of [
  ['undefined', 'this deliberately undefined OpenCode contract step'],
  ['assertion', 'unexpected CLI attempts or hidden model fallback'],
]) {
  const args = ['+1.99.1', 'test', '--locked', '--all-features', '--test',
    'opencode_bdd', '--', '--color', 'never', '--input',
    `tests/fixtures/opencode_specs/${name}.feature`];
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
