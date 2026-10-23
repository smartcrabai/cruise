const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { test } = require('node:test');
const model = 'check Safety for 3 expect 0\nrun Witness for 3 expect 1\n';
const valid = () => ({ commands: {
  Safety: { type: 'check' }, // Alloy omits zero-valued fields.
  Witness: { type: 'run', expects: 1, solution: [{ instances: [{ values: {} }] }] },
} });
function check(text, receipt) {
  fs.mkdirSync('target/formal-reports', { recursive: true });
  const dir = fs.mkdtempSync('target/formal-reports/receipt-test.');
  try {
    fs.writeFileSync(path.join(dir, 'model.als'), text);
    fs.writeFileSync(path.join(dir, 'receipt.json'), JSON.stringify(receipt));
    return spawnSync(process.execPath, ['scripts/formal/check_alloy_receipt.cjs',
      path.join(dir, 'model.als'), path.join(dir, 'receipt.json')], { encoding: 'utf8' });
  } finally {
    fs.rmSync(dir, { recursive: true });
  }
}
test('accepts explicit UNSAT assertions and SAT witnesses', () => {
  const result = check(model, valid());
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /1 UNSAT assertions, 1 SAT witnesses/);
});
for (const [name, mutate] of [
  ['counterexample', r => { r.commands.Safety.solution = [{ instances: [{ values: {} }] }]; }],
  ['malformed assertion solution', r => { r.commands.Safety.solution = {}; }],
  ['malformed witness solution', r => { r.commands.Witness.solution = 'SAT'; }],
  ['witness without instance', r => { r.commands.Witness.solution = [{}]; }],
  ['empty witness instances', r => { r.commands.Witness.solution = [{ instances: [] }]; }],
  ['unsatisfiable witness', r => { r.commands.Witness.solution = []; }],
  ['missing command', r => { delete r.commands.Safety; }],
  ['extra command', r => { r.commands.Extra = { type: 'check' }; }],
  ['wrong receipt expectation', r => { r.commands.Witness.expects = 0; }],
  ['wrong command kind', r => { r.commands.Safety.type = 'run'; }],
]) {
  test(`rejects ${name}`, () => {
    const receipt = valid();
    mutate(receipt);
    assert.notEqual(check(model, receipt).status, 0);
  });
}
test('rejects weakened or omitted model expectations', () => {
  assert.notEqual(check(model.replace('expect 0', 'expect 1'), valid()).status, 0);
  assert.notEqual(check(model.replace('expect 0', ''), valid()).status, 0);
});
test('requires assertions and witnesses', () => {
  assert.notEqual(check('', { commands: {} }).status, 0);
  assert.notEqual(check('check Safety for 3 expect 0', { commands: { Safety: { type: 'check' } } }).status, 0);
});
