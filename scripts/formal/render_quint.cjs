const { readFileSync, readdirSync, lstatSync, writeFileSync } = require('node:fs');
const { join, resolve } = require('node:path');
const { createRequire } = require('node:module');

const dir = resolve(process.argv[2]);
const tools = resolve(process.argv[3]);
const load = createRequire(join(tools, 'package.json'));
const { instance } = load('@viz-js/viz');
const { Resvg } = load('@resvg/resvg-js');

function label(value) {
  if (typeof value !== 'string' || !value.length || value.length > 96 || /[\x00-\x1f\x7f]/u.test(value)) {
    throw new Error('Expected nonempty diagram.state and diagram.via strings of at most 96 characters');
  }
  return value;
}

async function render() {
  const witnesses = readdirSync(dir).filter(name => /^test_.*\.itf\.json$/.test(name)).sort();
  if (!witnesses.length || witnesses.length > 64) throw new Error('Expected 1–64 Quint test traces');
  const nodes = new Map();
  const edges = new Map();
  let total = 0;
  for (const witness of witnesses) {
    const path = join(dir, witness);
    const meta = lstatSync(path);
    total += meta.size;
    if (!meta.isFile() || total > 8 * 1024 * 1024) throw new Error('Unsafe or oversized Quint traces');
    const { states } = JSON.parse(readFileSync(path, 'utf8'));
    if (!Array.isArray(states) || !states.length || states.length > 512) throw new Error('Invalid trace length');
    // A `.fail()` witness ends in the refused step's undefined state; draw only its valid prefix.
    let end = states.length;
    while (end > 1 && states[end - 1]?.diagram === undefined) end -= 1;
    let prev;
    for (const { diagram } of states.slice(0, end)) {
      const current = { state: label(diagram?.state), via: label(diagram?.via) };
      if (!nodes.has(current.state)) nodes.set(current.state, `s${nodes.size}`);
      if (prev && (current.state !== prev.state || current.via !== prev.via)) {
        const edge = [prev.state, current.state, current.via];
        edges.set(JSON.stringify(edge), edge);
      }
      if (nodes.size > 64 || edges.size > 192) throw new Error('Quint diagram has too many states or transitions');
      prev = current;
    }
  }
  if (!edges.size) throw new Error('Quint witnesses must exercise at least one transition');

  const quote = JSON.stringify;
  const dot = [
    'digraph model {',
    '  graph [rankdir=TB, pad=0.25, nodesep=0.35, ranksep=0.45, bgcolor="white"];',
    '  node [shape=box, style="rounded,filled", color="#5f7188", fillcolor="#f2f6fc", fontcolor="#121c2c", fontname="Arial", fontsize=13, margin="0.18,0.12"];',
    '  edge [color="#526278", fontcolor="#344359", fontname="Arial", fontsize=11, arrowsize=0.7];',
    ...Array.from(nodes, ([name, id]) => `  ${id} [label=${quote(name)}];`),
    ...Array.from(edges.values(), ([from, to, via]) => `  ${nodes.get(from)} -> ${nodes.get(to)} [label=${quote(via)}];`),
    '}',
  ].join('\n');
  const svg = (await instance()).renderString(dot, { format: 'svg', engine: 'dot' });
  const intrinsic = new Resvg(svg);
  const zoom = Math.min(1.5, 2400 / Math.max(intrinsic.width, intrinsic.height));
  const png = new Resvg(svg, { fitTo: { mode: 'zoom', value: zoom } }).render();
  const bytes = png.asPng();
  if (!bytes.length || bytes.length > 1024 * 1024) throw new Error('Quint PNG exceeds 1 MiB');
  writeFileSync(join(dir, 'diagram.png'), bytes, { flag: 'wx' });
  console.log(`${witnesses.length} witnesses, ${nodes.size} states, ${edges.size} transitions`);
}

render().catch(error => { console.error(error.message); process.exitCode = 1; });
