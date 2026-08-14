#!/usr/bin/env bash
# Exercises action/scripts/app-token.sh (the github_token -> App-exchange ->
# workflow-token priority ladder) and action/scripts/revoke-token.sh (its
# best-effort cleanup counterpart), driven directly against a stubbed curl.

. "$(dirname "$0")/lib/action_test_harness.sh"

# --- app-token.sh -----------------------------------------------------------
#
# curl is invoked twice on the App-exchange path: a GET-style OIDC request
# (no -X) and a POST exchange request (-X POST). The stub tells them apart by
# scanning argv for a literal "POST" element and drives each from its own set
# of CURL_OIDC_*/CURL_EXCHANGE_* control vars so every branch (success,
# missing field, HTTP status, hard failure, empty response) is reachable
# without a real network call. jq is NOT stubbed -- the real binary parses
# the canned JSON exactly as production does.
stub curl <<'SH'
#!/usr/bin/env bash
printf 'curl %s\n' "$*" >> "$STUB_LOG"
is_post=0
for a in "$@"; do
  case "$a" in
    POST) is_post=1 ;;
  esac
done
if [ "$is_post" = "1" ]; then
  if [ -n "${CURL_EXCHANGE_EXIT:-}" ]; then exit "$CURL_EXCHANGE_EXIT"; fi
  printf '%s' "${CURL_EXCHANGE_BODY:-}"
  if [ -n "${CURL_EXCHANGE_CODE:-}" ]; then printf '\n%s' "$CURL_EXCHANGE_CODE"; fi
  exit 0
else
  if [ -n "${CURL_OIDC_EXIT:-}" ]; then exit "$CURL_OIDC_EXIT"; fi
  printf '%s' "${CURL_OIDC_BODY:-}"
  exit 0
fi
SH

reset_token_env() {
  GH_TOKEN_INPUT=
  TOKEN_EXCHANGE_URL=https://exchange.example/token
  WORKFLOW_TOKEN=workflow-fallback-token
  ACTIONS_ID_TOKEN_REQUEST_TOKEN=oidc-request-token
  ACTIONS_ID_TOKEN_REQUEST_URL=https://actions.example/oidc
  CURL_OIDC_BODY=
  CURL_OIDC_EXIT=
  CURL_EXCHANGE_BODY=
  CURL_EXCHANGE_CODE=
  CURL_EXCHANGE_EXIT=
}

run_token() {
  env -i PATH="$PATH" HOME="$HOME" GITHUB_OUTPUT="$GITHUB_OUTPUT" STUB_LOG="$STUB_LOG" \
    GH_TOKEN_INPUT="$GH_TOKEN_INPUT" TOKEN_EXCHANGE_URL="$TOKEN_EXCHANGE_URL" \
    WORKFLOW_TOKEN="$WORKFLOW_TOKEN" \
    ACTIONS_ID_TOKEN_REQUEST_TOKEN="$ACTIONS_ID_TOKEN_REQUEST_TOKEN" \
    ACTIONS_ID_TOKEN_REQUEST_URL="$ACTIONS_ID_TOKEN_REQUEST_URL" \
    CURL_OIDC_BODY="$CURL_OIDC_BODY" CURL_OIDC_EXIT="$CURL_OIDC_EXIT" \
    CURL_EXCHANGE_BODY="$CURL_EXCHANGE_BODY" CURL_EXCHANGE_CODE="$CURL_EXCHANGE_CODE" \
    CURL_EXCHANGE_EXIT="$CURL_EXCHANGE_EXIT" \
    bash action/scripts/app-token.sh
}

# Runs app-token.sh, capturing combined stdout+stderr into $output and its
# exit status into $status without tripping this suite's own `set -e`.
run_token_capture() {
  output="$(run_token 2>&1)"
  status=$?
}

# A token value must appear in stdout exactly once, as the "::add-mask::TOKEN"
# line, and nowhere else -- i.e. never printed unmasked, and never printed
# again (unmasked or not) after being masked.
assert_only_masked_occurrence() { # $1=name $2=output $3=token
  local name="$1" out_text="$2" token="$3" matches
  matches="$(printf '%s\n' "$out_text" | grep -F "$token" || true)"
  if [ "$matches" = "::add-mask::$token" ]; then pass "$name"; else fail "$name" "lines containing the token: $matches"; fi
}

# --- explicit github_token input: highest priority, no exchange attempted --
new_case
reset_token_env
GH_TOKEN_INPUT=explicit-pat-token
run_token_capture
assert_eq "token: explicit github_token input wins and sets token output" "explicit-pat-token" "$(out token)"
assert_eq "token: explicit github_token input sets used_app=false" "false" "$(out used_app)"
assert_status "token: explicit github_token input exits 0" 0 "$status"
assert_eq "token: explicit github_token input never calls curl" "" "$(cat "$STUB_LOG")"
assert_only_masked_occurrence "token: explicit github_token input is masked and never printed unmasked" "$output" "explicit-pat-token"

# --- token_exchange_url disabled: falls straight to the workflow token -----
new_case
reset_token_env
TOKEN_EXCHANGE_URL=
run_token_capture
assert_eq "token: empty token_exchange_url falls back to the workflow token" "workflow-fallback-token" "$(out token)"
assert_eq "token: empty token_exchange_url fallback sets used_app=false" "false" "$(out used_app)"
assert_contains "token: empty token_exchange_url fallback explains why" "$output" "token_exchange_url is empty"
assert_eq "token: empty token_exchange_url fallback never calls curl" "" "$(cat "$STUB_LOG")"

# --- missing OIDC prerequisites: either half missing takes the same branch -
new_case
reset_token_env
ACTIONS_ID_TOKEN_REQUEST_TOKEN=
run_token_capture
assert_eq "token: missing ACTIONS_ID_TOKEN_REQUEST_TOKEN falls back" "workflow-fallback-token" "$(out token)"
assert_contains "token: missing OIDC request token names the id-token permission fix" "$output" "permissions: id-token: write"
# Known defect (filed): app-token.sh:54-56 logs this fallback with no
# ::warning::/::notice:: annotation, though docs/github-actions.md:287
# implies one for this "otherwise" bucket -- not asserting the (buggy)
# annotation-free output here.
assert_eq "token: missing OIDC request token fallback sets used_app=false" "false" "$(out used_app)"

new_case
reset_token_env
ACTIONS_ID_TOKEN_REQUEST_URL=
run_token_capture
assert_eq "token: missing ACTIONS_ID_TOKEN_REQUEST_URL alone also falls back" "workflow-fallback-token" "$(out token)"
assert_contains "token: missing OIDC request URL names the id-token permission fix" "$output" "permissions: id-token: write"

# --- OIDC request succeeds but carries no usable JWT ------------------------
new_case
reset_token_env
CURL_OIDC_BODY='{"no_value_here":true}'
run_token_capture
assert_eq "token: OIDC response missing .value falls back" "workflow-fallback-token" "$(out token)"
assert_contains "token: OIDC response missing .value warns" "$output" "::warning::cruise: failed to obtain an OIDC token"
refute_contains "token: OIDC response missing .value never attempts the exchange POST" "$(cat "$STUB_LOG")" "POST"

# --- full success path: OIDC ok, exchange 200 with a token ------------------
new_case
reset_token_env
CURL_OIDC_BODY='{"value":"fake-oidc-jwt"}'
CURL_EXCHANGE_BODY='{"token":"app-install-token"}'
CURL_EXCHANGE_CODE=200
run_token_capture
assert_eq "token: successful App exchange returns the App token" "app-install-token" "$(out token)"
assert_eq "token: successful App exchange sets used_app=true" "true" "$(out used_app)"
assert_status "token: successful App exchange exits 0" 0 "$status"
assert_only_masked_occurrence "token: successful App exchange masks the App token and never prints it unmasked" "$output" "app-install-token"
assert_contains "token: OIDC request appends the cruise-agent audience" "$(cat "$STUB_LOG")" "https://actions.example/oidc&audience=cruise-agent-token-exchange"
assert_contains "token: exchange request authenticates with the OIDC JWT as a bearer token" "$(cat "$STUB_LOG")" "Authorization: Bearer fake-oidc-jwt"
assert_contains "token: exchange request POSTs to token_exchange_url" "$(cat "$STUB_LOG")" "-X POST https://exchange.example/token"

# --- HTTP 200 but no token field in the body --------------------------------
new_case
reset_token_env
CURL_OIDC_BODY='{"value":"fake-oidc-jwt"}'
CURL_EXCHANGE_BODY='{"unexpected":true}'
CURL_EXCHANGE_CODE=200
run_token_capture
assert_eq "token: HTTP 200 without a token field falls back" "workflow-fallback-token" "$(out token)"
assert_contains "token: HTTP 200 without a token field warns" "$output" "::warning::cruise: token exchange returned 200 without a token field"

# --- HTTP 404: App not installed --------------------------------------------
new_case
reset_token_env
CURL_OIDC_BODY='{"value":"fake-oidc-jwt"}'
CURL_EXCHANGE_CODE=404
run_token_capture
assert_eq "token: HTTP 404 falls back" "workflow-fallback-token" "$(out token)"
assert_contains "token: HTTP 404 notices with the App install URL" "$output" "::notice::"
assert_contains "token: HTTP 404 notice names the install link" "$output" "https://github.com/apps/cruise-agent/installations/new"

# --- other HTTP statuses: message extraction priority (.message // .error) -
new_case
reset_token_env
CURL_OIDC_BODY='{"value":"fake-oidc-jwt"}'
CURL_EXCHANGE_BODY='{"message":"rate limited"}'
CURL_EXCHANGE_CODE=500
run_token_capture
assert_contains "token: HTTP 500 with .message warns with the status and message" "$output" "::warning::cruise: token exchange failed (HTTP 500: rate limited)"

new_case
reset_token_env
CURL_OIDC_BODY='{"value":"fake-oidc-jwt"}'
CURL_EXCHANGE_BODY='{"error":"boom"}'
CURL_EXCHANGE_CODE=503
run_token_capture
assert_contains "token: HTTP 503 with only .error falls back to the error field" "$output" "::warning::cruise: token exchange failed (HTTP 503: boom)"

new_case
reset_token_env
CURL_OIDC_BODY='{"value":"fake-oidc-jwt"}'
CURL_EXCHANGE_BODY='not valid json at all'
CURL_EXCHANGE_CODE=500
run_token_capture
assert_contains "token: HTTP 500 with an unparsable body still warns with the bare status" "$output" "::warning::cruise: token exchange failed (HTTP 500); using the workflow token"

# --- curl transport failures on the exchange call ---------------------------
new_case
reset_token_env
CURL_OIDC_BODY='{"value":"fake-oidc-jwt"}'
CURL_EXCHANGE_EXIT=22
run_token_capture
assert_eq "token: curl hard failure on the exchange call falls back" "workflow-fallback-token" "$(out token)"
assert_contains "token: curl hard failure names the curl exit code" "$output" "::warning::cruise: token exchange request failed (curl exit 22)"

new_case
reset_token_env
CURL_OIDC_BODY='{"value":"fake-oidc-jwt"}'
run_token_capture
assert_eq "token: an empty exchange response (curl exit 0, no body) still falls back" "workflow-fallback-token" "$(out token)"
assert_contains "token: an empty exchange response is reported the same as a curl failure" "$output" "::warning::cruise: token exchange request failed (curl exit 0)"

# --- terminal failure: nothing left to fall back to -------------------------
new_case
reset_token_env
ACTIONS_ID_TOKEN_REQUEST_TOKEN=
WORKFLOW_TOKEN=
run_token_capture
assert_nonzero_status "token: no github_token, no usable exchange, and no workflow token fails the step" "$status" "output=$output"
assert_contains "token: exhausted fallback reports a clear ::error::" "$output" "::error::cruise: no github_token input, no usable App token exchange, and no workflow token to fall back to"
refute_contains "token: exhausted fallback writes no token output" "$(cat "$GITHUB_OUTPUT")" "token="

# --- revoke-token.sh ---------------------------------------------------------
#
# The docs/task description frames this as a `gh api` call, but the script
# actually shells out to `curl -sf -X DELETE ... https://api.github.com/installation/token`
# directly (action/scripts/revoke-token.sh:15) -- there is no `gh` invocation
# at all. That's not a bug (curl is already a hard dependency of app-token.sh
# and the rest of the action), just a description/reality mismatch; stubbing
# curl (not gh) here to match what the script actually calls.
stub curl <<'SH'
#!/usr/bin/env bash
printf 'curl %s\n' "$*" >> "$STUB_LOG"
exit "${CURL_REVOKE_EXIT:-0}"
SH

run_revoke() {
  env -i PATH="$PATH" HOME="$HOME" GITHUB_OUTPUT="$GITHUB_OUTPUT" STUB_LOG="$STUB_LOG" \
    TOKEN="${TOKEN:-}" CURL_REVOKE_EXIT="${CURL_REVOKE_EXIT:-}" \
    bash action/scripts/revoke-token.sh
}
run_revoke_capture() {
  output="$(run_revoke 2>&1)"
  status=$?
}

new_case
TOKEN=
run_revoke_capture
assert_status "revoke: empty TOKEN exits 0" 0 "$status"
assert_eq "revoke: empty TOKEN never calls curl" "" "$(cat "$STUB_LOG")"
assert_contains "revoke: empty TOKEN logs a no-op" "$output" "no App installation token"

new_case
TOKEN=app-install-token-to-revoke
CURL_REVOKE_EXIT=0
run_revoke_capture
assert_status "revoke: a set TOKEN exits 0 on success" 0 "$status"
assert_contains "revoke: a set TOKEN issues DELETE against the installation-token endpoint" "$(cat "$STUB_LOG")" "-X DELETE"
assert_contains "revoke: a set TOKEN targets the installation token API" "$(cat "$STUB_LOG")" "https://api.github.com/installation/token"
assert_contains "revoke: a set TOKEN authenticates with 'token TOKEN'" "$(cat "$STUB_LOG")" "Authorization: token app-install-token-to-revoke"
assert_contains "revoke: success logs a confirmation" "$output" "cruise: revoked"

new_case
TOKEN=app-install-token-to-revoke
CURL_REVOKE_EXIT=1
run_revoke_capture
# revoke-token.sh's own last line is an unconditional `exit 0` (line 20), so
# a failing curl never fails the script -- action.yml's `continue-on-error:
# true` on this step (action.yml:310) is a second, redundant safety net, not
# the only thing keeping a revoke failure from failing the job.
assert_status "revoke: a failing curl does not make the script exit non-zero" 0 "$status" "status=$status"
assert_contains "revoke: a failing curl warns instead of erroring" "$output" "::warning::cruise: failed to revoke the cruise-agent App installation token"

finish
