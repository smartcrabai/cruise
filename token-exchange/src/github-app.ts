import { SignJWT, importPKCS8 } from "jose";

const USER_AGENT = "cruise-token-exchange";
const GITHUB_API_VERSION = "2022-11-28";

/** App JWT lifetime, per GitHub's documented +/-1 minute clock-drift allowance. */
const APP_JWT_BACK_DATE_SECONDS = 60;
const APP_JWT_LIFETIME_SECONDS = 540; // 9 minutes; GitHub's hard cap is 10 minutes.

export class PrivateKeyFormatError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "PrivateKeyFormatError";
  }
}

export class NotInstalledError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "NotInstalledError";
  }
}

export class GithubApiError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "GithubApiError";
  }
}

export interface GithubAppCredentials {
  appId: string;
  /** PKCS8 PEM. GitHub's downloaded .pem is PKCS1 and must be converted first. */
  privateKeyPem: string;
}

/**
 * Builds the short-lived App JWT used to authenticate as the GitHub App
 * itself (as opposed to a specific installation). `now` is injectable for
 * deterministic tests.
 */
export async function createAppJwt(
  creds: GithubAppCredentials,
  now: number = Math.floor(Date.now() / 1000),
): Promise<string> {
  if (creds.privateKeyPem.includes("BEGIN RSA PRIVATE KEY")) {
    throw new PrivateKeyFormatError(
      "GITHUB_APP_PRIVATE_KEY is PKCS1 (starts with 'BEGIN RSA PRIVATE KEY'). Convert it to PKCS8 " +
        "with: openssl pkcs8 -topk8 -inform PEM -in <downloaded>.pem -out key-pkcs8.pem -nocrypt " +
        "and re-upload the contents of key-pkcs8.pem as the secret.",
    );
  }

  let key: CryptoKey;
  try {
    key = await importPKCS8(creds.privateKeyPem, "RS256");
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    throw new PrivateKeyFormatError(`GITHUB_APP_PRIVATE_KEY could not be imported as a PKCS8 RSA key: ${reason}`);
  }

  return new SignJWT({})
    .setProtectedHeader({ alg: "RS256" })
    .setIssuedAt(now - APP_JWT_BACK_DATE_SECONDS)
    .setExpirationTime(now + APP_JWT_LIFETIME_SECONDS)
    .setIssuer(creds.appId)
    .sign(key);
}

/** Injectable fetch signature so GitHub API calls can be mocked in tests. */
export type GithubFetch = (input: string, init?: RequestInit) => Promise<Response>;

export interface InstallationInfo {
  installationId: number;
}

/** Resolves the installation ID for the App on a specific repository. */
export async function resolveInstallation(
  owner: string,
  repo: string,
  appJwt: string,
  fetchGithub: GithubFetch,
): Promise<InstallationInfo> {
  const res = await fetchGithub(`https://api.github.com/repos/${owner}/${repo}/installation`, {
    method: "GET",
    headers: {
      Authorization: `Bearer ${appJwt}`,
      Accept: "application/vnd.github+json",
      "User-Agent": USER_AGENT,
      "X-GitHub-Api-Version": GITHUB_API_VERSION,
    },
  });

  if (res.status === 404) {
    throw new NotInstalledError(
      `cruise-agent is not installed on ${owner}/${repo}. Install it at ` +
        "https://github.com/apps/cruise-agent/installations/new",
    );
  }
  if (!res.ok) {
    throw new GithubApiError(`GitHub installation lookup for ${owner}/${repo} failed with status ${res.status}`);
  }

  const body = (await res.json()) as { id?: unknown };
  if (typeof body.id !== "number") {
    throw new GithubApiError(
      `GitHub returned a malformed installation lookup response for ${owner}/${repo} (missing/non-numeric "id")`,
    );
  }
  return { installationId: body.id };
}

export interface InstallationToken {
  token: string;
  expiresAt: string;
}

/**
 * Mints a repository-scoped installation access token, explicitly narrowing
 * both the repository list and the permission set regardless of what the App
 * is otherwise configured to allow.
 */
export async function mintInstallationToken(
  installationId: number,
  repo: string,
  appJwt: string,
  fetchGithub: GithubFetch,
): Promise<InstallationToken> {
  const res = await fetchGithub(`https://api.github.com/app/installations/${installationId}/access_tokens`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${appJwt}`,
      Accept: "application/vnd.github+json",
      "User-Agent": USER_AGENT,
      "X-GitHub-Api-Version": GITHUB_API_VERSION,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      repositories: [repo],
      permissions: {
        contents: "write",
        pull_requests: "write",
        issues: "write",
      },
    }),
  });

  if (!res.ok) {
    throw new GithubApiError(`GitHub installation token creation failed with status ${res.status}`);
  }

  const body = (await res.json()) as { token?: unknown; expires_at?: unknown };
  if (typeof body.token !== "string" || typeof body.expires_at !== "string") {
    throw new GithubApiError("GitHub returned a malformed installation token response (missing/non-string token or expires_at)");
  }
  return { token: body.token, expiresAt: body.expires_at };
}
