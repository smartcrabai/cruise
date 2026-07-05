#!/usr/bin/env bash
# Resolves the GitHub token used for the rest of the run.
#
# Priority:
#   1. An explicit `github_token` input always wins (used_app=false).
#   2. Otherwise, if a token-exchange URL is configured and this job carries
#      an OIDC token (workflow granted `permissions: id-token: write`),
#      exchange it for a short-lived cruise-agent GitHub App installation
#      token scoped to this repository (used_app=true).
#   3. Otherwise, fall back to the workflow's own GITHUB_TOKEN (used_app=false)
#      -- cruise still runs, but PRs it opens with that token won't trigger
#      other `on: pull_request` workflows (a GitHub Actions anti-recursion
#      rule; see docs/github-actions.md).
#
# The resolved token is masked (`::add-mask::`) before being written to
# GITHUB_OUTPUT and is never otherwise printed.
set -uo pipefail

GH_TOKEN_INPUT="${GH_TOKEN_INPUT:-}"
TOKEN_EXCHANGE_URL="${TOKEN_EXCHANGE_URL:-}"
WORKFLOW_TOKEN="${WORKFLOW_TOKEN:-}"

APP_INSTALL_URL="https://github.com/apps/cruise-agent/installations/new"
OIDC_AUDIENCE="cruise-agent-token-exchange"

emit() { # $1=token $2=used_app
  echo "::add-mask::$1"
  {
    echo "token=$1"
    echo "used_app=$2"
  } >> "$GITHUB_OUTPUT"
}

use_fallback() { # $1=log line, already annotated (::notice::/::warning::) or plain; may be empty
  [ -n "${1:-}" ] && echo "$1"
  if [ -z "$WORKFLOW_TOKEN" ]; then
    echo "::error::cruise: no github_token input, no usable App token exchange, and no workflow token to fall back to" >&2
    exit 1
  fi
  emit "$WORKFLOW_TOKEN" "false"
  exit 0
}

if [ -n "$GH_TOKEN_INPUT" ]; then
  echo "cruise: using the explicit github_token input"
  emit "$GH_TOKEN_INPUT" "false"
  exit 0
fi

if [ -z "$TOKEN_EXCHANGE_URL" ]; then
  use_fallback "cruise: token_exchange_url is empty, using the workflow token"
fi

if [ -z "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-}" ] || [ -z "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ]; then
  use_fallback "cruise: no OIDC token available for this job (add \`permissions: id-token: write\` to use the cruise-agent App); using the workflow token"
fi

oidc_body="$(curl -sf -H "Authorization: Bearer ${ACTIONS_ID_TOKEN_REQUEST_TOKEN}" "${ACTIONS_ID_TOKEN_REQUEST_URL}&audience=${OIDC_AUDIENCE}" 2>/dev/null)"
oidc_jwt="$(printf '%s' "$oidc_body" | jq -r '.value // empty' 2>/dev/null)"

if [ -z "$oidc_jwt" ]; then
  use_fallback "::warning::cruise: failed to obtain an OIDC token for the App token exchange; using the workflow token"
fi

exchange_response="$(curl -s --max-time 15 -w $'\n%{http_code}' \
  -H "Authorization: Bearer ${oidc_jwt}" -X POST "$TOKEN_EXCHANGE_URL" 2>/dev/null)"
curl_exit=$?

if [ "$curl_exit" -ne 0 ] || [ -z "$exchange_response" ]; then
  use_fallback "::warning::cruise: token exchange request failed (curl exit ${curl_exit}); using the workflow token"
fi

http_code="$(printf '%s' "$exchange_response" | tail -n1)"
exchange_body="$(printf '%s' "$exchange_response" | sed '$d')"

case "$http_code" in
  200)
    app_token="$(printf '%s' "$exchange_body" | jq -r '.token // empty' 2>/dev/null)"
    if [ -z "$app_token" ]; then
      use_fallback "::warning::cruise: token exchange returned 200 without a token field; using the workflow token"
    fi
    echo "cruise: using a cruise-agent App installation token"
    emit "$app_token" "true"
    exit 0
    ;;
  404)
    use_fallback "::notice::cruise: the cruise-agent GitHub App is not installed on this repository -- install it at ${APP_INSTALL_URL} to let cruise run with a repository-scoped App token; using the workflow token for this run"
    ;;
  *)
    reason="$(printf '%s' "$exchange_body" | jq -r '.message // .error // empty' 2>/dev/null)"
    use_fallback "::warning::cruise: token exchange failed (HTTP ${http_code:-unknown}${reason:+: $reason}); using the workflow token"
    ;;
esac
