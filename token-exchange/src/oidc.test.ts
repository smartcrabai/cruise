import { describe, expect, it } from "vitest";
import { InvalidOidcTokenError, verifyActionsOidcToken } from "./oidc";
import { createTestOidcIssuer, signWithUntrustedKey } from "./test-helpers";

describe("verifyActionsOidcToken", () => {
  it("accepts a validly signed token and splits the repository claim", async () => {
    const issuer = await createTestOidcIssuer();
    const token = await issuer.sign({ repository: "acme/widgets" });

    const result = await verifyActionsOidcToken(token, issuer.getKey);

    expect(result.owner).toBe("acme");
    expect(result.repo).toBe("widgets");
    expect(result.claims.repository).toBe("acme/widgets");
  });

  it("rejects tokens signed by a key outside the trusted JWKS", async () => {
    const issuer = await createTestOidcIssuer();
    const forged = await signWithUntrustedKey({ repository: "acme/widgets" });

    await expect(verifyActionsOidcToken(forged, issuer.getKey)).rejects.toBeInstanceOf(InvalidOidcTokenError);
  });

  it("rejects a token with the wrong audience", async () => {
    const issuer = await createTestOidcIssuer();
    const token = await issuer.sign({ repository: "acme/widgets" }, { audience: "someone-elses-service" });

    await expect(verifyActionsOidcToken(token, issuer.getKey)).rejects.toBeInstanceOf(InvalidOidcTokenError);
  });

  it("rejects a token with the wrong issuer", async () => {
    const issuer = await createTestOidcIssuer();
    const token = await issuer.sign({ repository: "acme/widgets" }, { issuer: "https://not-github.example" });

    await expect(verifyActionsOidcToken(token, issuer.getKey)).rejects.toBeInstanceOf(InvalidOidcTokenError);
  });

  it("rejects an expired token", async () => {
    const issuer = await createTestOidcIssuer();
    const now = Math.floor(Date.now() / 1000);
    const token = await issuer.sign({ repository: "acme/widgets" }, { issuedAt: now - 600, expiresAt: now - 10 });

    await expect(verifyActionsOidcToken(token, issuer.getKey)).rejects.toBeInstanceOf(InvalidOidcTokenError);
  });

  it("rejects a token missing the repository claim", async () => {
    const issuer = await createTestOidcIssuer();
    const token = await issuer.sign({});

    await expect(verifyActionsOidcToken(token, issuer.getKey)).rejects.toThrow(/repository/);
  });

  it.each([
    "no-slash-here",
    "owner/",
    "/repo",
    "-owner/repo",
    "owner/repo/extra-segment",
    "owner name/repo",
    "owner/repo name",
    "owner/.",
    "owner/..",
  ])("rejects a malformed repository claim: %s", async (repository) => {
    const issuer = await createTestOidcIssuer();
    const token = await issuer.sign({ repository });

    await expect(verifyActionsOidcToken(token, issuer.getKey)).rejects.toBeInstanceOf(InvalidOidcTokenError);
  });
});
