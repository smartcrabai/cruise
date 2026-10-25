import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';

for (const [bridge, args, prefix] of [
  ['browser.mjs', ['drawer', '360', '640'], 'cruise-responsive-bdd-'],
  ['option-dialog.mjs', ['option', '360', '640'], 'cruise-option-browser-'],
]) {
  for (const closeRejects of [false, true]) {
    test(`${bridge}: ${closeRejects ? 'browser close rejection' : 'missing executable'} cleans up`, async () => {
      const root = await mkdtemp(join(tmpdir(), 'cruise-cleanup-test-'));
      let pid;
      try {
        const path = resolve('tests/webui', bridge);
        let command = [path, ...args];
        let executable = '/nonexistent/cruise-runtime';
        if (closeRejects) {
          executable = join(root, 'runtime');
          const pidFile = join(root, 'pid');
          await writeFile(executable, `#!/bin/sh\necho $$ > '${pidFile.replaceAll("'", "'\\''")}'\nprintf 'http://127.0.0.1:12345\\n'\nexec /bin/sleep 60\n`, { mode: 0o755 });
          const source = (await readFile(path, 'utf8')).replace(
            "import { chromium } from 'playwright';",
            "const chromium = { launch: async () => ({ version: () => 'cleanup fixture', newContext: async () => { throw new Error('injected context failure'); }, close: async () => { throw new Error('injected close rejection'); } }) };",
          );
          command = ['--input-type=module', '-e', source, '--', path, ...args];
        }
        const result = spawnSync(process.execPath, command, {
          env: { ...process.env, TMPDIR: root, TMP: root, TEMP: root, WEBUI_BDD_BIN: executable },
          encoding: 'utf8', timeout: 8000,
        });
        if (closeRejects) pid = Number(await readFile(join(root, 'pid'), 'utf8'));
        assert.ifError(result.error);
        assert.equal(result.signal, null, result.stderr);
        assert.notEqual(result.status, 0, result.stdout);
        assert.match(result.stderr, closeRejects ? /injected close rejection/ : /spawn \/nonexistent\/cruise-runtime ENOENT/);
        assert.match(result.stderr, /Production server stderr:/);
        assert.deepEqual((await readdir(root)).filter(name => name.startsWith(prefix)), [], 'temporary fixture tree removed');
        if (pid) assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' }, 'spawned child terminated despite browser close rejection');
      } finally {
        if (pid) {
          try { process.kill(pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
        }
        await rm(root, { recursive: true, force: true });
      }
    });
  }
}
