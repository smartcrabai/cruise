import { exportPKCS8, generateKeyPair } from "jose";
import { describe, expect, it, vi } from "vitest";
import {
  GithubApiError,
  NotInstalledError,
  PrivateKeyFormatError,
  createAppJwt,
  mintInstallationToken,
  resolveInstallation,
} from "./github-app";

async function generatePkcs8(): Promise<string> {
  const { privateKey } = await generateKeyPair("RS256", { extractable: true });
  return exportPKCS8(privateKey);
}

function base64UrlDecode(input: string): string {
  const padded = input.padEnd(input.length + ((4 - (input.length % 4)) % 4), "=");
  const base64 = padded.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(base64);
  const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

function decodePayload(jwt: string): Record<string, unknown> {
  const parts = jwt.split(".");
  const payloadB64 = parts[1];
  if (!payloadB64) {
    throw new Error("malformed JWT");
  }
  return JSON.parse(base64UrlDecode(payloadB64)) as Record<string, unknown>;
}

describe("createAppJwt", () => {
  it("signs a JWT with iss/iat/exp derived from the app id and clock", async () => {
    const privateKeyPem = await generatePkcs8();
    const now = 1_700_000_000;

    const jwt = await createAppJwt({ appId: "12345", privateKeyPem }, now);
    const payload = decodePayload(jwt);

    expect(payload.iss).toBe("12345");
    expect(payload.iat).toBe(now - 60);
    expect(payload.exp).toBe(now + 540);
  });

  it("rejects PKCS1 keys with an actionable conversion message", async () => {
    const pkcs1 = "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK...\n-----END RSA PRIVATE KEY-----\n";

    await expect(createAppJwt({ appId: "1", privateKeyPem: pkcs1 })).rejects.toBeInstanceOf(PrivateKeyFormatError);
    await expect(createAppJwt({ appId: "1", privateKeyPem: pkcs1 })).rejects.toThrow(/pkcs8/i);
  });

  it("rejects keys that cannot be parsed as PKCS8 at all", async () => {
    await expect(createAppJwt({ appId: "1", privateKeyPem: "not a pem key" })).rejects.toBeInstanceOf(
      PrivateKeyFormatError,
    );
  });
});

describe("resolveInstallation", () => {
  it("returns the installation id and sends the expected request", async () => {
    const fetchGithub = vi.fn(async (_url: string, _init?: RequestInit) => new Response(JSON.stringify({ id: 42 }), { status: 200 }));

    const result = await resolveInstallation("acme", "widgets", "app-jwt-value", fetchGithub);

    expect(result).toEqual({ installationId: 42 });
    expect(fetchGithub).toHaveBeenCalledTimes(1);
    const [calledUrl, init] = fetchGithub.mock.calls[0]!;
    expect(calledUrl).toBe("https://api.github.com/repos/acme/widgets/installation");
    const headers = init?.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer app-jwt-value");
    expect(headers["User-Agent"]).toBeTruthy();
  });

  it("throws NotInstalledError with the install URL on 404", async () => {
    const fetchGithub = vi.fn(async () => new Response("not found", { status: 404 }));

    await expect(resolveInstallation("acme", "widgets", "app-jwt", fetchGithub)).rejects.toBeInstanceOf(
      NotInstalledError,
    );
    await expect(resolveInstallation("acme", "widgets", "app-jwt", fetchGithub)).rejects.toThrow(
      /github\.com\/apps\/cruise-agent\/installations\/new/,
    );
  });

  it("throws GithubApiError on unexpected failures", async () => {
    const fetchGithub = vi.fn(async () => new Response("boom", { status: 500 }));

    await expect(resolveInstallation("acme", "widgets", "app-jwt", fetchGithub)).rejects.toBeInstanceOf(
      GithubApiError,
    );
  });

  it("throws GithubApiError when a 2xx response is missing a numeric id", async () => {
    const fetchGithub = vi.fn(async () => new Response(JSON.stringify({ id: "not-a-number" }), { status: 200 }));

    await expect(resolveInstallation("acme", "widgets", "app-jwt", fetchGithub)).rejects.toBeInstanceOf(
      GithubApiError,
    );
  });

  it("throws GithubApiError when a 2xx response body has no id field at all", async () => {
    const fetchGithub = vi.fn(async () => new Response(JSON.stringify({}), { status: 200 }));

    await expect(resolveInstallation("acme", "widgets", "app-jwt", fetchGithub)).rejects.toBeInstanceOf(
      GithubApiError,
    );
  });
});

describe("mintInstallationToken", () => {
  it("requests a token narrowly scoped to the repo and required permissions", async () => {
    const fetchGithub = vi.fn(
      async (_url: string, _init?: RequestInit) =>
        new Response(JSON.stringify({ token: "ghs_abc", expires_at: "2026-07-04T12:00:00Z" }), {
          status: 201,
        }),
    );

    const result = await mintInstallationToken(42, "widgets", "app-jwt", fetchGithub);

    expect(result).toEqual({ token: "ghs_abc", expiresAt: "2026-07-04T12:00:00Z" });
    const [calledUrl, init] = fetchGithub.mock.calls[0]!;
    expect(calledUrl).toBe("https://api.github.com/app/installations/42/access_tokens");
    const body = JSON.parse(init?.body as string) as {
      repositories: string[];
      permissions: Record<string, string>;
    };
    expect(body.repositories).toEqual(["widgets"]);
    expect(body.permissions).toEqual({ contents: "write", pull_requests: "write", issues: "write" });
  });

  it("throws GithubApiError when GitHub rejects the request", async () => {
    const fetchGithub = vi.fn(async () => new Response("boom", { status: 403 }));

    await expect(mintInstallationToken(42, "widgets", "app-jwt", fetchGithub)).rejects.toBeInstanceOf(
      GithubApiError,
    );
  });

  it("throws GithubApiError when a 2xx response is missing the token field", async () => {
    const fetchGithub = vi.fn(
      async () => new Response(JSON.stringify({ expires_at: "2026-07-04T12:00:00Z" }), { status: 201 }),
    );

    await expect(mintInstallationToken(42, "widgets", "app-jwt", fetchGithub)).rejects.toBeInstanceOf(
      GithubApiError,
    );
  });

  it("throws GithubApiError when a 2xx response has a non-string expires_at", async () => {
    const fetchGithub = vi.fn(
      async () => new Response(JSON.stringify({ token: "ghs_abc", expires_at: 12345 }), { status: 201 }),
    );

    await expect(mintInstallationToken(42, "widgets", "app-jwt", fetchGithub)).rejects.toBeInstanceOf(
      GithubApiError,
    );
  });
});
