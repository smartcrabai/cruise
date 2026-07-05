import { exportPKCS8, generateKeyPair } from "jose";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { createHandler, type HandlerDeps } from "./index";
import { createTestOidcIssuer, signWithUntrustedKey, type TestOidcIssuer } from "./test-helpers";

let appPrivateKeyPem: string;
let issuer: TestOidcIssuer;

const BASE_ENV: Env = {
  GITHUB_APP_ID: "app-id-123",
  GITHUB_APP_PRIVATE_KEY: "", // filled in beforeAll
};

beforeAll(async () => {
  const { privateKey } = await generateKeyPair("RS256", { extractable: true });
  appPrivateKeyPem = await exportPKCS8(privateKey);
  BASE_ENV.GITHUB_APP_PRIVATE_KEY = appPrivateKeyPem;
  issuer = await createTestOidcIssuer();
});

/** Builds a GithubFetch stub that branches on the request URL. */
function githubFetchStub(opts: {
  installation?: { status: number; body?: unknown };
  accessToken?: { status: number; body?: unknown };
}): HandlerDeps["fetchGithub"] {
  return vi.fn(async (url: string) => {
    // NB: "/installations/{id}/access_tokens" contains "/installation" as a
    // substring, so the access-tokens check must be tried first (or the
    // installation-lookup check must be an exact suffix match).
    if (url.includes("/access_tokens")) {
      const { status, body } = opts.accessToken ?? {
        status: 201,
        body: { token: "ghs_test_token", expires_at: "2026-07-04T12:00:00Z" },
      };
      return new Response(body === undefined ? undefined : JSON.stringify(body), { status });
    }
    if (url.endsWith("/installation")) {
      const { status, body } = opts.installation ?? { status: 200, body: { id: 42 } };
      return new Response(body === undefined ? undefined : JSON.stringify(body), { status });
    }
    throw new Error(`unexpected fetch to ${url}`);
  });
}

function tokenRequest(authorization?: string): Request {
  const headers = new Headers();
  if (authorization !== undefined) {
    headers.set("Authorization", authorization);
  }
  return new Request("https://token-exchange.example/token", { method: "POST", headers });
}

describe("GET /healthz", () => {
  it("returns 200 ok without requiring authentication", async () => {
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub: githubFetchStub({}) });
    const res = await handler(new Request("https://token-exchange.example/healthz"), BASE_ENV);

    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
  });
});

describe("POST /token", () => {
  it("returns 405 for non-POST methods", async () => {
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub: githubFetchStub({}) });
    const res = await handler(new Request("https://token-exchange.example/token", { method: "GET" }), BASE_ENV);

    expect(res.status).toBe(405);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBeTruthy();
  });

  it("returns 401 invalid_oidc when the Authorization header is missing", async () => {
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub: githubFetchStub({}) });
    const res = await handler(tokenRequest(), BASE_ENV);

    expect(res.status).toBe(401);
    expect(await res.json()).toMatchObject({ error: "invalid_oidc" });
  });

  it("returns 401 invalid_oidc when the token signature is untrusted", async () => {
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub: githubFetchStub({}) });
    const forged = await signWithUntrustedKey({ repository: "acme/widgets" });
    const res = await handler(tokenRequest(`Bearer ${forged}`), BASE_ENV);

    expect(res.status).toBe(401);
    expect(await res.json()).toMatchObject({ error: "invalid_oidc" });
  });

  it("returns 401 invalid_oidc when the repository claim is malformed", async () => {
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub: githubFetchStub({}) });
    const token = await issuer.sign({ repository: "not-a-valid-repo-claim" });
    const res = await handler(tokenRequest(`Bearer ${token}`), BASE_ENV);

    expect(res.status).toBe(401);
    expect(await res.json()).toMatchObject({ error: "invalid_oidc" });
  });

  it("returns 404 not_installed when the App is not installed on the repo", async () => {
    const fetchGithub = githubFetchStub({ installation: { status: 404 } });
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub });
    const token = await issuer.sign({ repository: "acme/widgets" });
    const res = await handler(tokenRequest(`Bearer ${token}`), BASE_ENV);

    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: string; message: string };
    expect(body.error).toBe("not_installed");
    expect(body.message).toContain("https://github.com/apps/cruise-agent/installations/new");
  });

  it("returns 200 with a repo-scoped installation token on the happy path", async () => {
    const fetchGithub = githubFetchStub({
      installation: { status: 200, body: { id: 99 } },
      accessToken: { status: 201, body: { token: "ghs_success", expires_at: "2026-07-04T18:00:00Z" } },
    });
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub });
    const token = await issuer.sign({ repository: "acme/widgets" });
    const res = await handler(tokenRequest(`Bearer ${token}`), BASE_ENV);

    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({
      token: "ghs_success",
      expires_at: "2026-07-04T18:00:00Z",
      installation_id: 99,
    });
  });

  it("returns 502 github_error when GitHub fails to mint the access token", async () => {
    const fetchGithub = githubFetchStub({
      installation: { status: 200, body: { id: 99 } },
      accessToken: { status: 500 },
    });
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub });
    const token = await issuer.sign({ repository: "acme/widgets" });
    const res = await handler(tokenRequest(`Bearer ${token}`), BASE_ENV);

    expect(res.status).toBe(502);
    expect(await res.json()).toMatchObject({ error: "github_error" });
  });

  it("returns 502 github_error when GitHub's installation lookup 2xx body is malformed", async () => {
    const fetchGithub = githubFetchStub({
      installation: { status: 200, body: { id: "not-a-number" } },
    });
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub });
    const token = await issuer.sign({ repository: "acme/widgets" });
    const res = await handler(tokenRequest(`Bearer ${token}`), BASE_ENV);

    expect(res.status).toBe(502);
    expect(await res.json()).toMatchObject({ error: "github_error" });
  });

  it("returns 502 github_error when GitHub's access-token 2xx body is malformed", async () => {
    const fetchGithub = githubFetchStub({
      installation: { status: 200, body: { id: 99 } },
      accessToken: { status: 201, body: { expires_at: "2026-07-04T18:00:00Z" } },
    });
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub });
    const token = await issuer.sign({ repository: "acme/widgets" });
    const res = await handler(tokenRequest(`Bearer ${token}`), BASE_ENV);

    expect(res.status).toBe(502);
    expect(await res.json()).toMatchObject({ error: "github_error" });
  });

  it("returns 502 github_error when the private key is misconfigured (PKCS1)", async () => {
    const fetchGithub = githubFetchStub({});
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub });
    const token = await issuer.sign({ repository: "acme/widgets" });
    const badEnv: Env = {
      GITHUB_APP_ID: "app-id-123",
      GITHUB_APP_PRIVATE_KEY: "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK...\n-----END RSA PRIVATE KEY-----\n",
    };
    const res = await handler(tokenRequest(`Bearer ${token}`), badEnv);

    expect(res.status).toBe(502);
    expect(await res.json()).toMatchObject({ error: "github_error" });
  });

  it("returns 502 github_error when app credentials are missing", async () => {
    const fetchGithub = githubFetchStub({});
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub });
    const token = await issuer.sign({ repository: "acme/widgets" });
    const emptyEnv: Env = { GITHUB_APP_ID: "", GITHUB_APP_PRIVATE_KEY: "" };
    const res = await handler(tokenRequest(`Bearer ${token}`), emptyEnv);

    expect(res.status).toBe(502);
    expect(await res.json()).toMatchObject({ error: "github_error" });
  });
});

describe("unknown routes", () => {
  it("returns 404 for paths other than /token and /healthz", async () => {
    const handler = createHandler({ getKey: issuer.getKey, fetchGithub: githubFetchStub({}) });
    const res = await handler(new Request("https://token-exchange.example/nope"), BASE_ENV);

    expect(res.status).toBe(404);
  });
});
