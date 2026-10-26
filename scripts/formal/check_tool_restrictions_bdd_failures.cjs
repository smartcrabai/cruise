// Run actual Cucumber failure controls. Generated features stay in the report
// directory, so no extra repository fixture files are needed.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const directory = process.argv[2];
if (!directory) throw Error('Usage: check_tool_restrictions_bdd_failures.cjs report-directory');
const manifest = path.resolve(__dirname, '../../Cargo.toml');
const output = path.resolve(directory);
fs.mkdirSync(output, { recursive: true });
const cases = [
  ['undefined', 'Feature: Undefined control\n  Scenario: Undefined steps must fail\n    Then this deliberately undefined tool restrictions step\n', 'this deliberately undefined tool restrictions step', '1 step failed'],
  ['assertion', 'Feature: Assertion control\n  Scenario: Real policy assertion must fail\n    Given workflow restrictions "none" and step restrictions "none"\n    When the public policy is computed with permission full and allow_commit true\n    Then the policy has 99 shell rules\n', 'the policy has 99 shell rules', '1 step failed'],
  ['parse', 'not a Gherkin feature\n', 'parsing error', 'parsing error'],
  ['empty', null, 'no tool restriction scenarios executed', 'no tool restriction scenarios executed'],
];
for (const [name, feature, marker, failure] of cases) {
  const args = ['+1.99.1', 'test', '--locked', '--all-features', '--manifest-path', manifest, '--test', 'tool_restrictions_bdd', '--', '--color', 'never'];
  if (feature !== null) {
    const fixture = path.join(output, `${name}.feature`);
    fs.writeFileSync(fixture, feature);
    // globwalk treats an absolute path without a wildcard as its walk root.
    args.push('--input', path.join(output, `${name}*.feature`));
  } else {
    args.push('--name', '^NO_SUCH_TOOL_RESTRICTION_SCENARIO$');
  }
  console.log(`Negative control argv: ${JSON.stringify(['cargo', ...args])}`);
  const result = spawnSync('cargo', args, { cwd: output, encoding: 'utf8', timeout: 180000 });
  const text = `${result.stdout ?? ''}${result.stderr ?? ''}`;
  fs.writeFileSync(path.join(output, `bdd-negative-${name}.log`), text);
  if (result.error || result.signal || result.status !== 101 || !text.includes(marker)
      || !text.includes(failure) || text.includes('Retry attempt:')) {
    console.error(text);
    throw Error(`${name}: expected runner rejection without retry and exit 101, got ${result.status}`);
  }
  console.log(`${name}: runner rejection without retry and exit 101 confirmed`);
}
