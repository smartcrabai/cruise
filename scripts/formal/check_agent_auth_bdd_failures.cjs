// Native cucumber 0.23.0 negative controls. Compiler/platform/launch errors do
// not count as successful controls. Cucumber has no pending API: an explicitly
// unimplemented step panics, which must propagate through the native runner.
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const output = process.argv[2];
if (!output) throw Error('Usage: check_agent_auth_bdd_failures.cjs report-directory');
fs.mkdirSync(output, { recursive: true });
const base = ['+1.99.1', 'test', '--locked', '--all-features', '--test', 'agent_auth_bdd', '--', '--color', 'never'];
for (const [name, marker] of [
  ['undefined', 'this deliberately undefined agent auth contract step'],
  ['pending', 'AUTH_BDD_PENDING: contract deliberately unimplemented'],
  ['assertion', 'SDK never reached:'],
  ['empty', 'no agent auth scenarios executed'],
]) {
  const args = [...base, ...(name === 'empty'
    ? ['--name', '^NO_SUCH_AGENT_AUTH_SCENARIO$']
    : ['--input', `tests/fixtures/agent_auth_specs/${name}.feature`])];
  console.log(`Negative control argv: ${JSON.stringify(['cargo', ...args])}`);
  const result = spawnSync('cargo', args, { encoding: 'utf8', timeout: 120000 });
  const text = `${result.stdout ?? ''}${result.stderr ?? ''}`;
  fs.writeFileSync(path.join(output, `agent-auth-negative-${name}.log`), text);
  if (result.error || result.signal || result.status !== 101
      || !text.includes(marker) || text.includes('Retry attempt:')
      || (name !== 'empty' && !text.includes('1 step failed'))) {
    console.error(text);
    throw Error(`${name}: expected native contract rejection and exit 101, got ${result.status}`);
  }
  console.log(`${name}: native rejection, no scenario rerun, exit 101 confirmed`);
}
