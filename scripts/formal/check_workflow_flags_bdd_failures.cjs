// Exercise native Cucumber failure behavior, not only Gherkin parsing.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const report = process.argv[2];
if (!report) throw Error('Usage: check_workflow_flags_bdd_failures.cjs report-directory');
fs.mkdirSync(report, { recursive: true });
for (const [name, marker, tail] of [
  ['undefined', 'this deliberately undefined workflow flag contract step',
    ['--input', 'tests/fixtures/workflow_flags_specs/undefined.feature']],
  ['assertion', 'only the e2e optional effect runs',
    ['--input', 'tests/fixtures/workflow_flags_specs/assertion.feature']],
  ['empty', 'no workflow flag scenarios executed', ['--name', '^NO_SUCH_WORKFLOW_FLAG_SCENARIO$']],
]) {
  const args = ['+1.99.1', 'test', '--locked', '--all-features', '--test',
    'workflow_flags_bdd', '--', '--color', 'never', ...tail];
  console.log(`Negative control argv: ${JSON.stringify(['cargo', ...args])}`);
  const result = spawnSync('cargo', args, { encoding: 'utf8', timeout: 120000 });
  const text = `${result.stdout ?? ''}${result.stderr ?? ''}`;
  fs.writeFileSync(path.join(report, `bdd-negative-${name}.log`), text);
  if (result.error || result.signal || result.status !== 101 || !text.includes(marker)
      || (name !== 'empty' && !text.includes('1 failed steps'))
      || text.includes('Retry attempt:')) {
    console.error(text);
    throw Error(`${name}: expected native rejection, no rerun and exit 101, got ${result.status}`);
  }
  console.log(`${name}: native rejection and exit 101 confirmed`);
}
