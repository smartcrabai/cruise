#!/usr/bin/env bash
# Best-effort revocation of the short-lived cruise-agent App installation
# token minted by app-token.sh, mirroring anthropics/claude-code-action's
# cleanup step. Never fails the job: a revoke failure just means the token
# lives out its (short) natural expiry instead.
set -uo pipefail

TOKEN="${TOKEN:-}"

if [ -z "$TOKEN" ]; then
  echo "cruise: no App installation token to revoke"
  exit 0
fi

if curl -sf -X DELETE -H "Authorization: token ${TOKEN}" https://api.github.com/installation/token >/dev/null 2>&1; then
  echo "cruise: revoked the cruise-agent App installation token"
else
  echo "::warning::cruise: failed to revoke the cruise-agent App installation token (it will expire on its own)"
fi
exit 0
