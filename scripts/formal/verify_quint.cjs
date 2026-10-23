// A fresh endpoint prevents attaching to an unpinned, already running backend.
const net = require('node:net');
const { spawn } = require('node:child_process');
const [quint, model, main, steps, output] = process.argv.slice(2);
if (!output) throw Error('Usage: verify_quint.cjs quint model main steps output');
const reservation = net.createServer();
reservation.on('error', error => { console.error(error); process.exitCode = 1; });
reservation.listen(0, '127.0.0.1', () => {
  const endpoint = `127.0.0.1:${reservation.address().port}`;
  reservation.close(() => {
    const args = [quint, 'verify', '--invariant', 'safety', '--max-steps', steps,
      '--main', main, '--init', 'init', '--step', 'step', '--backend', 'apalache',
      '--apalache-version', '0.62.1', '--server-endpoint', endpoint,
      '--out-itf', output, model];
    console.log(`Apalache 0.62.1, bounded at ${steps} transitions`);
    console.log(`Command argv: ${JSON.stringify([process.execPath, ...args])}`);
    const child = spawn(process.execPath, args, { stdio: 'inherit' });
    child.on('error', error => { console.error(error); process.exitCode = 1; });
    child.on('exit', (code, signal) => {
      if (signal) console.error(`Quint terminated by ${signal}`);
      process.exitCode = code ?? 1;
    });
    for (const signal of ['SIGINT', 'SIGTERM']) {
      process.on(signal, () => child.kill('SIGINT'));
    }
  });
});
