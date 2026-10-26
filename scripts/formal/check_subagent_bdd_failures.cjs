// Real-runner negative controls. Temporary features keep failure fixtures out of
// the positive suite. Compiler failures and timeouts cannot satisfy a control.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const output = process.argv[2];
if (!output) throw Error('Usage: check_subagent_bdd_failures.cjs report-directory');
fs.mkdirSync(output, { recursive: true });
const root = fs.mkdtempSync(path.join(process.env.JCODE_SCRATCH_DIR || os.tmpdir(), 'subagent-bdd-'));
const base = ['+1.99.1', 'test', '--locked', '--all-features', '--test', 'subagent_model_bdd', '--', '--color', 'never'];
function run(name, args, marker, stepMarker = marker) {
  console.log(`Negative control argv: ${JSON.stringify(['cargo', ...args])}`);
  const result = spawnSync('cargo', args, { encoding: 'utf8', timeout: 120000 });
  const text = `${result.stdout || ''}${result.stderr || ''}`;
  fs.writeFileSync(path.join(output, `bdd-negative-${name}.log`), text);
  if (result.error || result.signal || result.status !== 101 || !text.includes(marker) || !text.includes(stepMarker) || text.includes('Retry attempt:')
      || (name === 'assertion' && (!text.includes('left: "haiku"') || !text.includes('right: "deliberately-wrong"')))) {
    console.error(text);
    throw Error(`${name}: expected contract failure and exit 101, got ${result.status}`);
  }
  console.log(`${name}: contract failure and exit 101 confirmed`);
}
try {
  const undefinedFeature = path.join(root, 'undefined.feature');
  fs.writeFileSync(undefinedFeature, 'Feature: Undefined control\n  Scenario: Undefined contract must fail\n    Then deliberately undefined subagent contract\n');
  run('undefined', [...base, '--input', path.join(root, 'undefined*.feature')], '1 step failed', 'deliberately undefined subagent contract');
  const assertionFeature = path.join(root, 'assertion.feature');
  fs.writeFileSync(assertionFeature, `Feature: Assertion control
  Scenario: Wrong model must fail
    Given this subagent workflow
      """
      sdk: claude
      subagent_model: haiku
      steps:
        answer:
          prompt: SUBAGENT_NEGATIVE
      """
    When the production CLI runs exec
    Then the CLI succeeds
    And task requests use model deliberately-wrong and force 1
`);
  run('assertion', [...base, '--input', path.join(root, 'assertion*.feature')], '1 step failed', 'deliberately-wrong');
  run('empty', [...base, '--name', '^NO_SUCH_SUBAGENT_SCENARIO$'], 'no subagent model scenarios executed');
} finally {
  fs.rmSync(root, { recursive: true, force: true });
}
