// Alloy 6.2.0 exec may exit successfully despite expectation mismatches.
// Inspect every explicitly selected command in its newly generated receipt.
const fs = require('node:fs');
const model = fs.readFileSync(process.argv[2], 'utf8')
  .replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '');
const receipt = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
const commands = [...model.matchAll(/^\s*(run|check)\s+([A-Za-z_][A-Za-z_0-9]*)\b/gm)];
if (!commands.length) throw Error('No named Alloy commands found');
let witnesses = 0;
let assertions = 0;
const seen = new Set();
for (let i = 0; i < commands.length; i++) {
  const match = commands[i];
  const name = match[2];
  const text = model.slice(match.index, commands[i + 1]?.index ?? model.length);
  const expected = text.match(/\bexpect\s+([01])\b/);
  if (!expected) throw Error(`Alloy command ${name} needs explicit expect 0 or 1`);
  if (seen.has(name)) throw Error(`Duplicate Alloy command ${name}`);
  seen.add(name);
  const required = match[1] === 'check' ? 0 : 1;
  if (Number(expected[1]) !== required) throw Error(`Wrong expectation for ${name}`);
  const actual = receipt.commands?.[name];
  if (!actual || actual.type !== match[1]) throw Error(`Missing Alloy result: ${name}`);
  // An absent solution is Alloy 6.2.0's UNSAT encoding, but a malformed
  // present field is not evidence of UNSAT and must never pass an assertion.
  if (actual.solution !== undefined && !Array.isArray(actual.solution)) {
    throw Error(`Malformed Alloy solution: ${name}`);
  }
  if (actual.solution?.some(solution =>
    !Array.isArray(solution.instances) || solution.instances.length === 0)) {
    throw Error(`Alloy solution has no concrete instance: ${name}`);
  }
  const sat = Array.isArray(actual.solution) && actual.solution.length > 0;
  if (Number(sat) !== required) throw Error(`Alloy ${name}: expected ${required}, got ${Number(sat)}`);
  // Alloy's JSON omits zero-valued expects fields, including explicit expect 0.
  if ((actual.expects ?? 0) !== required) throw Error(`Receipt expectation differs: ${name}`);
  if (required === 1) witnesses++; else assertions++;
  console.log(`${name}: ${sat ? 'SAT' : 'UNSAT'}, expect ${required} passed`);
}
if (!assertions || !witnesses) throw Error('Both assertions and nonvacuous witnesses are required');
if (Object.keys(receipt.commands).length !== seen.size) throw Error('Alloy command coverage mismatch');
console.log(`Receipt validated: ${assertions} UNSAT assertions, ${witnesses} SAT witnesses`);
