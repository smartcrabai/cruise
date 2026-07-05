import type { JWTPayload, JWTVerifyGetKey } from "jose";
import { jwtVerify } from "jose";

/** GitHub Actions' fixed OIDC issuer. */
export const OIDC_ISSUER = "https://token.actions.githubusercontent.com";

/**
 * Fixed OIDC audience for this service. Workflows must request their ID
 * token with `audience: cruise-agent-token-exchange`. This value is part of
 * the API contract shared with the calling GitHub Action and must not change
 * without a coordinated rollout.
 */
export const OIDC_AUDIENCE = "cruise-agent-token-exchange";

/**
 * Matches a GitHub `owner/repo` full name.
 * - owner: alphanumeric or hyphen, cannot start/end with a hyphen (GitHub login rules)
 * - repo: alphanumeric, `.`, `_`, `-` (GitHub repository name rules)
 */
const REPOSITORY_PATTERN = /^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?\/[A-Za-z0-9._-]+$/;

export interface ActionsOidcClaims extends JWTPayload {
  repository: string;
}

export class InvalidOidcTokenError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "InvalidOidcTokenError";
  }
}

export interface VerifiedActionsToken {
  owner: string;
  repo: string;
  claims: ActionsOidcClaims;
}

/**
 * Verifies a GitHub Actions OIDC token: signature (via `getKey`), issuer,
 * audience, algorithm, and standard time-based claims (exp/nbf, enforced by
 * `jwtVerify` itself). Also validates and extracts the `repository` claim,
 * which is the *only* source of truth for which repository a minted
 * installation token may be scoped to.
 *
 * `getKey` is injected so production can use `createRemoteJWKSet` against
 * GitHub's real JWKS endpoint while tests use `createLocalJWKSet` with a
 * throwaway keypair.
 */
export async function verifyActionsOidcToken(
  token: string,
  getKey: JWTVerifyGetKey,
): Promise<VerifiedActionsToken> {
  let payload: JWTPayload;
  try {
    const result = await jwtVerify(token, getKey, {
      issuer: OIDC_ISSUER,
      audience: OIDC_AUDIENCE,
      algorithms: ["RS256"],
    });
    payload = result.payload;
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    throw new InvalidOidcTokenError(`OIDC token verification failed: ${reason}`);
  }

  const repository = payload.repository;
  if (typeof repository !== "string" || !REPOSITORY_PATTERN.test(repository)) {
    throw new InvalidOidcTokenError("OIDC token is missing a well-formed 'repository' claim");
  }

  const separatorIndex = repository.indexOf("/");
  const owner = repository.slice(0, separatorIndex);
  const repo = repository.slice(separatorIndex + 1);

  // "." / ".." can't be real GitHub repository names, but if they ever
  // appeared here they would be path-normalized inside the GitHub API URL --
  // reject them outright rather than rely on that being unreachable.
  if (repo === "." || repo === "..") {
    throw new InvalidOidcTokenError("OIDC token is missing a well-formed 'repository' claim");
  }

  return { owner, repo, claims: payload as ActionsOidcClaims };
}
