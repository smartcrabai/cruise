import type { JWTVerifyGetKey } from "jose";
import { createRemoteJWKSet } from "jose";
import {
  GithubApiError,
  NotInstalledError,
  PrivateKeyFormatError,
  createAppJwt,
  mintInstallationToken,
  resolveInstallation,
  type GithubFetch,
} from "./github-app";
import { InvalidOidcTokenError, OIDC_ISSUER, verifyActionsOidcToken } from "./oidc";

const JWKS_URL = new URL(`${OIDC_ISSUER}/.well-known/jwks`);

interface ErrorBody {
  error: "invalid_oidc" | "not_installed" | "github_error" | "method_not_allowed" | "not_found";
  message: string;
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function errorResponse(status: number, error: ErrorBody["error"], message: string): Response {
  return jsonResponse(status, { error, message } satisfies ErrorBody);
}

/**
 * Dependencies the request handler needs beyond `env`. Injected so tests can
 * supply a local JWKS (throwaway keypair) and a stubbed GitHub fetch instead
 * of talking to the real OIDC provider and GitHub API.
 */
export interface HandlerDeps {
  getKey: JWTVerifyGetKey;
  fetchGithub: GithubFetch;
}

/**
 * Builds the `/token` + `/healthz` request handler. Kept as a factory (rather
 * than a module-level function reaching for module-level dependencies) so
 * production and tests can each supply their own `HandlerDeps`.
 */
export function createHandler(deps: HandlerDeps) {
  return async function handleRequest(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === "/healthz") {
      return jsonResponse(200, { ok: true });
    }

    if (url.pathname !== "/token") {
      return errorResponse(404, "not_found", "Unknown route");
    }

    if (request.method !== "POST") {
      return errorResponse(405, "method_not_allowed", "Only POST is supported on /token");
    }

    const authHeader = request.headers.get("Authorization") ?? "";
    const bearerMatch = /^Bearer (.+)$/.exec(authHeader);
    const oidcToken = bearerMatch?.[1];
    if (!oidcToken) {
      return errorResponse(401, "invalid_oidc", "Missing or malformed Authorization header");
    }

    let owner: string;
    let repo: string;
    try {
      const verified = await verifyActionsOidcToken(oidcToken, deps.getKey);
      owner = verified.owner;
      repo = verified.repo;
    } catch (err) {
      if (err instanceof InvalidOidcTokenError) {
        return errorResponse(401, "invalid_oidc", err.message);
      }
      throw err;
    }

    if (!env.GITHUB_APP_ID || !env.GITHUB_APP_PRIVATE_KEY) {
      return errorResponse(
        502,
        "github_error",
        "Worker is missing GITHUB_APP_ID / GITHUB_APP_PRIVATE_KEY configuration",
      );
    }

    try {
      const appJwt = await createAppJwt({
        appId: env.GITHUB_APP_ID,
        privateKeyPem: env.GITHUB_APP_PRIVATE_KEY,
      });
      const { installationId } = await resolveInstallation(owner, repo, appJwt, deps.fetchGithub);
      const { token, expiresAt } = await mintInstallationToken(installationId, repo, appJwt, deps.fetchGithub);

      return jsonResponse(200, {
        token,
        expires_at: expiresAt,
        installation_id: installationId,
      });
    } catch (err) {
      if (err instanceof NotInstalledError) {
        return errorResponse(404, "not_installed", err.message);
      }
      if (err instanceof PrivateKeyFormatError || err instanceof GithubApiError) {
        return errorResponse(502, "github_error", err.message);
      }
      // Do not leak unexpected error internals (may include stack traces
      // referencing secret material's call sites) to the caller.
      return errorResponse(502, "github_error", "Unexpected error while exchanging the token");
    }
  };
}

/**
 * Production dependencies. `createRemoteJWKSet` is constructed once at
 * module scope so its internal key cache (per jose's own guidance) is reused
 * across requests -- this is a read-only cache of GitHub's public signing
 * keys, not request-scoped state.
 */
const productionDeps: HandlerDeps = {
  getKey: createRemoteJWKSet(JWKS_URL),
  fetchGithub: (input, init) => fetch(input, init),
};

const handleRequest = createHandler(productionDeps);

export default {
  fetch: handleRequest,
} satisfies ExportedHandler<Env>;
