// Test-only utilities for building throwaway OIDC issuers. Never imported
// from production code (src/index.ts), so this stays out of the deployed
// bundle.
import type { JWTPayload, JWTVerifyGetKey } from "jose";
import { SignJWT, createLocalJWKSet, exportJWK, generateKeyPair } from "jose";
import { OIDC_AUDIENCE, OIDC_ISSUER } from "./oidc";

export interface SignOverrides {
  issuer?: string;
  audience?: string;
  alg?: string;
  kid?: string;
  issuedAt?: number;
  expiresAt?: number;
}

export interface TestOidcIssuer {
  getKey: JWTVerifyGetKey;
  sign(claims: JWTPayload, overrides?: SignOverrides): Promise<string>;
}

/**
 * Creates a throwaway RSA keypair plus a `createLocalJWKSet` exposing only
 * its public half, standing in for GitHub's real JWKS endpoint in tests.
 */
export async function createTestOidcIssuer(): Promise<TestOidcIssuer> {
  const { publicKey, privateKey } = await generateKeyPair("RS256", { extractable: true });
  const kid = "test-key";
  const jwk = await exportJWK(publicKey);
  jwk.kid = kid;
  jwk.alg = "RS256";
  const getKey = createLocalJWKSet({ keys: [jwk] });

  return {
    getKey,
    async sign(claims, overrides = {}) {
      const now = Math.floor(Date.now() / 1000);
      return new SignJWT(claims)
        .setProtectedHeader({ alg: overrides.alg ?? "RS256", kid: overrides.kid ?? kid })
        .setIssuedAt(overrides.issuedAt ?? now)
        .setExpirationTime(overrides.expiresAt ?? now + 300)
        .setIssuer(overrides.issuer ?? OIDC_ISSUER)
        .setAudience(overrides.audience ?? OIDC_AUDIENCE)
        .sign(privateKey);
    },
  };
}

/** Signs a token with a *different* key than any configured JWKS trusts. */
export async function signWithUntrustedKey(claims: JWTPayload): Promise<string> {
  const { privateKey } = await generateKeyPair("RS256", { extractable: true });
  const now = Math.floor(Date.now() / 1000);
  return new SignJWT(claims)
    .setProtectedHeader({ alg: "RS256", kid: "untrusted" })
    .setIssuedAt(now)
    .setExpirationTime(now + 300)
    .setIssuer(OIDC_ISSUER)
    .setAudience(OIDC_AUDIENCE)
    .sign(privateKey);
}
