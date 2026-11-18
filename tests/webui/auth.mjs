import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';

export function parseWebUiLoginUrl(output) {
  const match = output.match(/^cruise daemon WebUI login: (https?:\/\/[^\s]+)\r?$/m);
  if (!match) return null;
  const loginUrl = new URL(match[1]);
  assert.equal(loginUrl.pathname, '/login', 'daemon startup login URL path');
  assert(loginUrl.searchParams.get('nonce'), 'daemon startup login URL nonce');
  assert.equal(loginUrl.searchParams.has('token'), false, 'daemon startup login URL must not contain the API token');
  return match[1];
}

export function requestFreshWebUiLoginUrl(binary, cwd, env) {
  const result = spawnSync(binary, ['webui'], {
    cwd,
    env,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    timeout: 20000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, `cruise webui failed: ${result.stderr}`);
  const loginUrl = result.stdout.trim();
  const parsed = new URL(loginUrl);
  assert.equal(parsed.pathname, '/login', 'cruise webui login URL path');
  assert(parsed.searchParams.get('nonce'), 'cruise webui login URL nonce');
  assert.equal(parsed.searchParams.has('token'), false, 'cruise webui login URL must not contain the API token');
  return loginUrl;
}

export function waitForWebUiLogin(server, getLogs = () => '') {
  return new Promise((resolve, reject) => {
    let output = '';
    let settled = false;
    const finish = (callback, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      server.stdout.off('data', onData);
      server.off('error', onError);
      server.off('exit', onExit);
      callback(value);
    };
    const onData = chunk => {
      output += chunk;
      try {
        const loginUrl = parseWebUiLoginUrl(output);
        if (loginUrl) finish(resolve, { loginUrl, base: new URL(loginUrl).origin });
      } catch (error) {
        finish(reject, error);
      }
    };
    const onError = error => finish(reject, error);
    const onExit = code => finish(reject, new Error(`server exited ${code}: ${getLogs()}`));
    const timeout = setTimeout(() => finish(reject, new Error(`server startup timeout: ${getLogs()}`)), 20000);
    server.stdout.on('data', onData);
    server.once('error', onError);
    server.once('exit', onExit);
  });
}

export async function loginWebUi(page, loginUrl) {
  const nonce = new URL(loginUrl).searchParams.get('nonce');
  assert(nonce, 'login URL must include a single-use nonce');
  assert.equal(new URL(loginUrl).searchParams.has('token'), false, 'login URL must not contain the API token');
  const response = await page.goto(loginUrl);
  assert(response, 'login navigation must return a response');
  assert.equal(response.status(), 200, 'login must redirect to the WebUI');
  assert.equal(page.url(), `${new URL(loginUrl).origin}/`, 'login must redirect to the WebUI root');
  const cookie = (await page.context().cookies(loginUrl)).find(item => item.name === 'cruise_webui');
  assert(cookie, 'login must set the WebUI authentication cookie');
  assert.notEqual(cookie.value, nonce, 'WebUI cookie must contain a separate session ID');
  assert.equal(cookie.httpOnly, true, 'WebUI authentication cookie must be HttpOnly');
  assert.equal(cookie.sameSite, 'Strict', 'WebUI authentication cookie must be SameSite=Strict');
  assert.equal(cookie.path, '/', 'WebUI authentication cookie must cover all routes');
  return `cruise_webui=${cookie.value}`;
}
