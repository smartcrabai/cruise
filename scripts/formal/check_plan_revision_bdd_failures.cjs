// Apply focused mutations in a scratch repository and require the real BDD to reject each.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { spawnSync } = require('node:child_process');

const output = process.argv[2];
if (!output) throw Error('Usage: check_plan_revision_bdd_failures.cjs run-directory');
const report = path.resolve(output);
fs.mkdirSync(report, { recursive: true });
const source = path.resolve(__dirname, '../..');
const scratchRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'cruise-plan-revision-'));
const scratch = path.join(scratchRoot, 'repo');
const target = path.join(scratchRoot, 'target');
try {
fs.cpSync(source, scratch, {
  recursive: true,
  filter: (entry) => {
    const relative = path.relative(source, entry);
    if (!relative) return true;
    return !['.git', 'target', 'node_modules'].includes(relative.split(path.sep)[0]);
  },
});

const files = {
  prompt: 'prompts/fix-plan.md',
  webui: 'src/webui/actions.rs',
  application: 'src/application.rs',
};
const originals = Object.fromEntries(
  Object.entries(files).map(([name, relative]) => [name, fs.readFileSync(path.join(scratch, relative), 'utf8')]),
);
const cases = [
  {
    name: 'fix-prompt-input',
    marker: 'sentinel missing from plan',
    mutate() {
      const text = originals.prompt;
      if (!text.includes('{prev.input}')) throw Error('fix-plan prompt has no {prev.input} mutation target');
      fs.writeFileSync(path.join(scratch, files.prompt), text.replaceAll('{prev.input}', ''));
    },
  },
  {
    name: 'fix-prev-input-assignment',
    marker: 'undefined variable: {prev.input}',
    mutate() {
      const from = '        vars.set_prev_input(Some(input));';
      if (originals.application.split(from).length !== 2) {
        throw Error('prev input assignment mutation target changed');
      }
      fs.writeFileSync(
        path.join(scratch, files.application),
        originals.application.replace(from, '        let _ = input;'),
      );
    },
  },
  {
    name: 'planned-webui-fix',
    marker: 'WebUI planning action parity failed in Planned',
    mutate() {
      const from = 'let show_fix = review_actions_available;';
      if (originals.webui.split(from).length !== 2) throw Error('WebUI Planned show_fix mutation target changed');
      fs.writeFileSync(
        path.join(scratch, files.webui),
        originals.webui.replace(from, 'let show_fix = session.phase == "Awaiting Approval" && review_actions_available;'),
      );
    },
  },
  {
    name: 'replan-feedback',
    marker: 'replan unexpectedly accepted user text',
    mutate() {
      const from = '        Self::validate_plan_request_text(operation, &request)?;';
      if (originals.application.split(from).length !== 2) {
        throw Error('plan request validation mutation target changed');
      }
      fs.writeFileSync(
        path.join(scratch, files.application),
        originals.application.replace(from, '        let _ = &request;'),
      );
    },
  },
];

const cargo = spawnSync('cargo', ['+1.99.1', '--version'], { encoding: 'utf8' });
const toolchain = cargo.status === 0 ? '+1.99.1' : null;
for (const test of cases) {
  for (const [name, relative] of Object.entries(files)) {
    fs.writeFileSync(path.join(scratch, relative), originals[name]);
  }
  test.mutate();
  const args = [
    ...(toolchain ? [toolchain] : []),
    'test', '--locked', '--all-features', '--manifest-path', path.join(scratch, 'Cargo.toml'),
    '--test', 'plan_revision_bdd', '--', '--color', 'never',
  ];
  console.log(`Negative control argv: ${JSON.stringify(['cargo', ...args])}`);
  let result;
  try {
    result = spawnSync('cargo', args, {
      cwd: scratch,
      encoding: 'utf8',
      timeout: 600000,
      // A user-level `build.build-dir` would otherwise share intermediate test
      // binaries with the real checkout and bake the scratch path into them.
      env: { ...process.env, CARGO_TARGET_DIR: target, CARGO_BUILD_BUILD_DIR: path.join(target, 'build') },
    });
  } finally {
    for (const [name, relative] of Object.entries(files)) {
      fs.writeFileSync(path.join(scratch, relative), originals[name]);
    }
  }
  const text = `${result.stdout ?? ''}${result.stderr ?? ''}`;
  fs.writeFileSync(path.join(report, `plan-revision-negative-${test.name}.log`), text);
  if (result.error || result.signal || result.status !== 101 || !text.includes(test.marker)) {
    console.error(text);
    throw Error(`${test.name}: expected the BDD to reject the mutation, got exit ${result.status}`);
  }
  console.log(`${test.name}: BDD rejection confirmed`);
}
} finally {
  fs.rmSync(scratchRoot, { recursive: true, force: true });
}
