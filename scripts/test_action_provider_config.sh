#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
PASS=0
FAIL=0
pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1 -- $2"; FAIL=$((FAIL + 1)); }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export GITHUB_ENV="$TMP/github_env"
export GITHUB_OUTPUT="$TMP/github_output"
export RUNNER_TEMP="$TMP/runner"
mkdir -p "$RUNNER_TEMP"

run_setup() {
  : > "$GITHUB_ENV"
  rm -rf "$RUNNER_TEMP/pi-agent"
  env -i PATH="$PATH" HOME="$HOME" GITHUB_ENV="$GITHUB_ENV" RUNNER_TEMP="$RUNNER_TEMP" \
    ANTHROPIC_API_KEY_INPUT="${ANTHROPIC_API_KEY_INPUT:-}" OPENAI_API_KEY_INPUT="${OPENAI_API_KEY_INPUT:-}" \
    MODEL_INPUT="${MODEL_INPUT:-}" PLAN_MODEL_INPUT="${PLAN_MODEL_INPUT:-}" PI_MODELS_JSON="${PI_MODELS_JSON:-}" \
    ENV_INPUT="${ENV_INPUT:-}" PROVIDERS_INPUT="${PROVIDERS_INPUT:-}" PROVIDER_API_KEYS_INPUT="${PROVIDER_API_KEYS_INPUT:-}" \
    bash action/scripts/setup-env.sh
}
assert_setup_fails() {
  local name="$1" expected="$2"
  set +e
  output="$(run_setup 2>&1)"
  status=$?
  set -e
  if [ "$status" -ne 0 ] && [[ "$output" == *"$expected"* ]]; then pass "$name"; else fail "$name" "status=$status output=$output"; fi
}

export PROVIDERS_INPUT='{"anthropic":{"api":"anthropic-messages","base_url":"https://anthropic.example/v1/messages","models":["claude"]},"openai":{"api":"openai-completions","base_url":"https://openai.example/v1","models":["gpt"]},"responses":{"api":"openai-responses","base_url":"https://responses.example/v1/responses","models":["o3"]}}'
export PROVIDER_API_KEYS_INPUT=$'responses=response-key==\n# ignored\nanthropic=anthropic-key\nopenai=openai-key'
output="$(run_setup)"
if jq -e '.providers | length == 3 and .anthropic.api == "anthropic-messages" and .openai.baseUrl == "https://openai.example/v1" and (.responses.models[0].id == "o3") and ([.[] .apiKey] | sort == ["env:CRUISE_PROVIDER_API_KEY_0", "env:CRUISE_PROVIDER_API_KEY_1", "env:CRUISE_PROVIDER_API_KEY_2"])' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null \
  && grep -Fqx 'CRUISE_PROVIDER_API_KEY_2=response-key==' "$GITHUB_ENV" \
  && ! grep -Fq 'response-key==' "$RUNNER_TEMP/pi-agent/models.json"; then pass "generates sorted multi-provider registry and preserves equals"; else fail "generates sorted multi-provider registry" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi
if grep -Fqx 'CRUISE_PROVIDER_API_KEY_0=anthropic-key' "$GITHUB_ENV" && grep -Fqx 'CRUISE_PROVIDER_API_KEY_1=openai-key' "$GITHUB_ENV" && grep -Fqx 'CRUISE_PROVIDER_API_KEY_2=response-key==' "$GITHUB_ENV"; then pass "maps sorted provider ids to the correct key by index"; else fail "maps sorted provider ids to the correct key by index" "$(cat "$GITHUB_ENV")"; fi
if printf '%s\n' "$output" | grep -Fq '::add-mask::anthropic-key' && printf '%s\n' "$output" | grep -Fq '::add-mask::openai-key' && printf '%s\n' "$output" | grep -Fq '::add-mask::response-key=='; then pass "masks every generated provider key"; else fail "masks every generated provider key" "$output"; fi
if grep -Fqx "PI_CODING_AGENT_DIR=$RUNNER_TEMP/pi-agent" "$GITHUB_ENV"; then pass "exports PI_CODING_AGENT_DIR on the providers path"; else fail "exports PI_CODING_AGENT_DIR on the providers path" "$(cat "$GITHUB_ENV")"; fi
if ! grep -Fq 'anthropic-key' "$RUNNER_TEMP/pi-agent/models.json" && ! grep -Fq 'openai-key' "$RUNNER_TEMP/pi-agent/models.json"; then pass "no raw provider key literal in generated models.json"; else fail "no raw provider key literal in generated models.json" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'
export PROVIDER_API_KEYS_INPUT='x = mykey'
run_setup
if grep -Fqx 'CRUISE_PROVIDER_API_KEY_0=mykey' "$GITHUB_ENV"; then pass "trims whitespace around provider api key"; else fail "trims whitespace around provider api key" "$(cat "$GITHUB_ENV")"; fi

unset PROVIDERS_INPUT PROVIDER_API_KEYS_INPUT
export PI_MODELS_JSON='{"providers":{"custom":{"api":"openai-completions","baseUrl":"https://x","models":[{"id":"m"}]}}}'
run_setup
if [ "$(cat "$RUNNER_TEMP/pi-agent/models.json")" = "$PI_MODELS_JSON" ] && grep -Fqx "PI_CODING_AGENT_DIR=$RUNNER_TEMP/pi-agent" "$GITHUB_ENV"; then pass "preserves raw pi_models_json"; else fail "preserves raw pi_models_json" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi
unset PI_MODELS_JSON

export PROVIDERS_INPUT='not-json'; export PROVIDER_API_KEYS_INPUT='x=k'; assert_setup_fails "rejects malformed JSON" "'providers' must be a non-empty JSON object"
export PROVIDERS_INPUT='[]'; assert_setup_fails "rejects non-object JSON" "'providers' must be a non-empty JSON object"
export PROVIDERS_INPUT='{}'; assert_setup_fails "rejects empty provider map" "'providers' must be a non-empty JSON object"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; unset PROVIDER_API_KEYS_INPUT; assert_setup_fails "rejects one-sided providers" "must be set together"
export PROVIDER_API_KEYS_INPUT='x=k'; unset PROVIDERS_INPUT; assert_setup_fails "rejects one-sided keys" "must be set together"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='y=k'; assert_setup_fails "rejects missing provider key" "missing provider id 'x'"
export PROVIDER_API_KEYS_INPUT=$'x=k\nx=two'; assert_setup_fails "rejects duplicate provider key" "duplicate provider id 'x'"
export PROVIDER_API_KEYS_INPUT=$'x=k\ny=k2'; assert_setup_fails "rejects extra provider key" "unknown provider id 'y'"
export PROVIDERS_INPUT='{"bad id":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='bad id=k'; assert_setup_fails "rejects invalid provider id" "invalid provider id 'bad id'"
export PROVIDERS_INPUT='{"x":{"api":"other","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='x=k'; assert_setup_fails "rejects unsupported api" "unsupported api 'other'"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"ftp://x","models":["m"]}}'; assert_setup_fails "rejects non-http URL" "invalid base_url"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":[]}}'; assert_setup_fails "rejects empty models" "models must be"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m","m"]}}'; assert_setup_fails "rejects duplicate models" "models must be"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"oops":true}}'; assert_setup_fails "rejects unknown fields" "has unknown key 'oops'"

# Fix 1 (DESIGN.md) drops the old mutual exclusion between `providers` and
# `pi_models_json` in favor of a deep merge (pi_models_json wins), so this
# case -- which used to assert a hard failure -- now asserts the merge
# actually happened instead. The full compose semantics (overlay wins,
# provider-only-in-one-side survives, models replaced wholesale) get their
# own dedicated tests further down; this one just re-purposes the original
# minimal fixture to prove the combination is no longer rejected outright.
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='x=k'; export PI_MODELS_JSON='{}'
run_setup
if jq -e '.providers.x.api == "openai-completions" and .providers.x.apiKey == "env:CRUISE_PROVIDER_API_KEY_0"' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "providers combined with pi_models_json now merges instead of erroring"; else fail "providers combined with pi_models_json now merges instead of erroring" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi
unset PI_MODELS_JSON
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='xk'; assert_setup_fails "rejects provider_api_keys line without '='" "expected provider-id=API key"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='x='; assert_setup_fails "rejects empty provider api key" "has an empty API key"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='x=   '; assert_setup_fails "rejects whitespace-only provider api key" "has an empty API key"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='bad id=k'; assert_setup_fails "rejects invalid provider id appearing only in provider_api_keys" "invalid provider id 'bad id' in 'provider_api_keys'"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x\n","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='x=k'; assert_setup_fails "rejects base_url ending in a newline" "invalid base_url"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["   "]}}'; export PROVIDER_API_KEYS_INPUT='x=k'; assert_setup_fails "rejects whitespace-only model id" "models must be"
export PROVIDERS_INPUT=$'{"x":{"api":"openai-completions","base_url":"https://x","models":["m\\n"]}}'; assert_setup_fails "rejects model id ending in a newline" "models must be"

# --- providers + pi_models_json composition (DESIGN.md Fix 1): the overlay
# wins on a conflicting field, a provider present on only one side survives
# either way, and `models` is replaced wholesale rather than concatenated
# (jq's `*` merge does a wholesale array replacement, not a concat).
export PROVIDERS_INPUT='{"p1":{"api":"openai-completions","base_url":"https://p1.example","models":["m1","m2"]},"p3":{"api":"openai-completions","base_url":"https://p3.example","models":["m3only"]}}'
export PROVIDER_API_KEYS_INPUT=$'p1=p1key\np3=p3key'
export PI_MODELS_JSON='{"providers":{"p1":{"baseUrl":"https://p1-overlay.example","models":[{"id":"m3"}]},"p2":{"api":"openai-completions","baseUrl":"https://p2.example","apiKey":"env:P2_KEY","models":[{"id":"m2raw"}]}}}'
run_setup
if jq -e '
    .providers.p1.baseUrl == "https://p1-overlay.example"
    and .providers.p1.api == "openai-completions"
    and .providers.p1.apiKey == "env:CRUISE_PROVIDER_API_KEY_0"
    and .providers.p1.models == [{"id":"m3"}]
    and .providers.p2.baseUrl == "https://p2.example"
    and .providers.p2.apiKey == "env:P2_KEY"
    and .providers.p3.baseUrl == "https://p3.example"
    and .providers.p3.apiKey == "env:CRUISE_PROVIDER_API_KEY_1"
    and .providers.p3.models[0].id == "m3only"
  ' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "pi_models_json overlay wins on conflict, one-sided providers survive, models replaced wholesale"; else fail "pi_models_json overlay wins on conflict, one-sided providers survive, models replaced wholesale" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi
unset PI_MODELS_JSON

# pi_models_json alone: valid JSON but not an object, and an object missing
# 'providers' -- both must now fail (ModelsConfig.providers is required by
# pi's own deserializer, even as {}; see setup-env.sh's two distinct checks).
unset PROVIDERS_INPUT PROVIDER_API_KEYS_INPUT
export PI_MODELS_JSON='[]'; assert_setup_fails "rejects pi_models_json that is valid JSON but not an object" "is not valid JSON"
export PI_MODELS_JSON='{}'; assert_setup_fails "rejects a pi_models_json object with no providers key" "has no 'providers' object"
unset PI_MODELS_JSON

# --- no_auth: true (DESIGN.md Fix 2): omit from provider_api_keys; no
# apiKey is emitted; a non-empty 'authorization' header is injected unless
# the entry already has its own non-blank auth-override header (checked
# case-insensitively, in 'headers').
export PROVIDERS_INPUT='{"na":{"api":"openai-completions","base_url":"https://na.example","models":["m"],"no_auth":true}}'
unset PROVIDER_API_KEYS_INPUT
run_setup
if jq -e '(.providers.na | has("apiKey") | not) and .providers.na.headers.authorization == "no-auth"' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "no_auth provider omitted from provider_api_keys succeeds with no apiKey and an injected authorization header"; else fail "no_auth provider omitted from provider_api_keys succeeds with no apiKey and an injected authorization header" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"na":{"api":"anthropic-messages","base_url":"https://na.example","models":["m"],"no_auth":true,"headers":{"x-api-key":"mykey"}}}'
run_setup
if jq -e '(.providers.na | has("apiKey") | not) and .providers.na.headers == {"x-api-key":"mykey"}' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "no_auth suppresses header injection when a non-authorization override header is already present"; else fail "no_auth suppresses header injection when a non-authorization override header is already present" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"na":{"api":"openai-completions","base_url":"https://na.example","models":["m"],"no_auth":true,"headers":{"Authorization":"tok"}}}'
run_setup
if jq -e '(.providers.na | has("apiKey") | not) and .providers.na.headers == {"Authorization":"tok"}' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "no_auth suppresses header injection for a differently-cased override header name"; else fail "no_auth suppresses header injection for a differently-cased override header name" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"na":{"api":"openai-completions","base_url":"https://na.example","models":["m"],"no_auth":true}}'
export PROVIDER_API_KEYS_INPUT='na=somekey'
assert_setup_fails "rejects a no_auth provider listed in provider_api_keys" "is 'no_auth' and must not appear in 'provider_api_keys'"

unset PROVIDER_API_KEYS_INPUT
export PROVIDERS_INPUT='{"na":{"api":"bedrock-converse-stream","base_url":"https://bedrock.example","models":["m"],"no_auth":true}}'
assert_setup_fails "rejects no_auth combined with bedrock-converse-stream" "cannot set no_auth with api 'bedrock-converse-stream'"

export PROVIDERS_INPUT='{"na1":{"api":"openai-completions","base_url":"https://na1.example","models":["m"],"no_auth":true},"na2":{"api":"anthropic-messages","base_url":"https://na2.example","models":["m"],"no_auth":true}}'
run_setup
if jq -e '
    (.providers.na1 | has("apiKey") | not) and .providers.na1.headers.authorization == "no-auth"
    and (.providers.na2 | has("apiKey") | not) and .providers.na2.headers.authorization == "no-auth"
  ' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "all providers no_auth succeeds with provider_api_keys entirely unset"; else fail "all providers no_auth succeeds with provider_api_keys entirely unset" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"keyed":{"api":"openai-completions","base_url":"https://keyed.example","models":["m"]},"na":{"api":"openai-completions","base_url":"https://na.example","models":["m"],"no_auth":true}}'
export PROVIDER_API_KEYS_INPUT='keyed=thekey'
run_setup
if jq -e '.providers.keyed.apiKey == "env:CRUISE_PROVIDER_API_KEY_0" and (.providers.na | has("apiKey") | not)' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null \
  && grep -Fqx 'CRUISE_PROVIDER_API_KEY_0=thekey' "$GITHUB_ENV" \
  && ! grep -Fq 'CRUISE_PROVIDER_API_KEY_1' "$GITHUB_ENV"; then pass "a keyed + no_auth mix assigns CRUISE_PROVIDER_API_KEY_0 to the keyed provider only"; else fail "a keyed + no_auth mix assigns CRUISE_PROVIDER_API_KEY_0 to the keyed provider only" "$(cat "$GITHUB_ENV")"; fi
unset PROVIDER_API_KEYS_INPUT

# --- no_auth header-override scoping (DESIGN.md Fix 2, refined): the header
# name(s) that suppress an adapter's own "Missing API key" check are NOT the
# same across adapters -- 'authorization' alone for openai-completions /
# openai-responses / cohere-chat / bedrock-converse-stream, plus 'x-api-key'
# for anthropic-messages, 'x-goog-api-key' for google-generative-ai, and
# 'api-key' for azure-openai-responses. Treating these as one flat union
# would let e.g. a cohere-chat provider's own 'api-key' header (an adapter
# that never looks at that name) count as an override, silently skip the
# authorization injection, and hand pi a config that hard-errors "Missing
# API key" at request time -- the exact failure this validation exists to
# prevent. Both this scoping and the bedrock-converse-stream rejection below
# must also consider a per-model 'api' override, since dispatch can pick a
# different adapter than the provider-level one.
export PROVIDERS_INPUT='{"na":{"api":"cohere-chat","base_url":"https://na.example","models":["m"],"no_auth":true,"headers":{"api-key":"tok"}}}'
unset PROVIDER_API_KEYS_INPUT
run_setup
if jq -e '(.providers.na | has("apiKey") | not) and .providers.na.headers.authorization == "no-auth" and .providers.na.headers["api-key"] == "tok"' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "cohere-chat's own 'api-key' header does not suppress injection (wrong adapter for that name)"; else fail "cohere-chat's own 'api-key' header does not suppress injection (wrong adapter for that name)" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"na":{"api":"openai-completions","base_url":"https://na.example","models":["m"],"no_auth":true,"headers":{"x-api-key":"tok"}}}'
run_setup
if jq -e '(.providers.na | has("apiKey") | not) and .providers.na.headers.authorization == "no-auth" and .providers.na.headers["x-api-key"] == "tok"' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "openai-completions's 'x-api-key' header does not suppress injection (wrong adapter for that name)"; else fail "openai-completions's 'x-api-key' header does not suppress injection (wrong adapter for that name)" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"na":{"api":"anthropic-messages","base_url":"https://na.example","models":["m"],"no_auth":true,"headers":{"x-api-key":"tok"}}}'
run_setup
if jq -e '.providers.na.headers | has("authorization") | not' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "anthropic-messages's own 'x-api-key' header does suppress injection (right adapter for that name)"; else fail "anthropic-messages's own 'x-api-key' header does suppress injection (right adapter for that name)" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"na":{"api":"google-generative-ai","base_url":"https://na.example","models":["m"],"no_auth":true,"headers":{"X-Goog-Api-Key":"tok"}}}'
run_setup
if jq -e '.providers.na.headers | has("authorization") | not' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "google-generative-ai's differently-cased 'X-Goog-Api-Key' header suppresses injection (case-insensitive match)"; else fail "google-generative-ai's differently-cased 'X-Goog-Api-Key' header suppresses injection (case-insensitive match)" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"na":{"api":"azure-openai-responses","base_url":"https://na.example","models":["m"],"no_auth":true,"headers":{"api-key":"tok"}}}'
run_setup
if jq -e '.providers.na.headers | has("authorization") | not' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "azure-openai-responses's own 'api-key' header suppresses injection (right adapter for that name)"; else fail "azure-openai-responses's own 'api-key' header suppresses injection (right adapter for that name)" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

# A per-model 'api' override can dispatch a different adapter than the
# provider-level one, so the recognized override set must be the
# INTERSECTION across every api that could run for this provider -- not
# just the provider-level api alone. Here the provider is anthropic-messages
# (recognizes x-api-key) but its one model overrides to openai-completions
# (does not), so the intersection drops to {authorization} and 'x-api-key'
# must NOT suppress the injection.
export PROVIDERS_INPUT='{"na":{"api":"anthropic-messages","base_url":"https://na.example","models":[{"id":"m","api":"openai-completions"}],"no_auth":true,"headers":{"x-api-key":"tok"}}}'
run_setup
if jq -e '.providers.na.headers.authorization == "no-auth"' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "a model-level api override narrows the recognized override set to the intersection across all effective adapters"; else fail "a model-level api override narrows the recognized override set to the intersection across all effective adapters" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

# build_provider() merges compat.customHeaders into the same
# override-detection pool as top-level `headers`, so an override supplied
# there must suppress injection exactly the same way.
export PROVIDERS_INPUT='{"na":{"api":"anthropic-messages","base_url":"https://na.example","models":["m"],"no_auth":true,"compat":{"custom_headers":{"x-api-key":"tok"}}}}'
run_setup
if jq -e '.providers.na.compat.customHeaders["x-api-key"] == "tok" and (.providers.na.headers // {} | has("authorization") | not)' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "an override supplied via compat.custom_headers also suppresses injection"; else fail "an override supplied via compat.custom_headers also suppresses injection" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

# has_nonblank_override() trims before checking length, matching pi's own
# first_non_empty_header_value_case_insensitive (which does the same) -- a
# whitespace-only value must NOT count as a real override, so injection must
# still happen.
export PROVIDERS_INPUT='{"na":{"api":"openai-completions","base_url":"https://na.example","models":["m"],"no_auth":true,"headers":{"authorization":"   "}}}'
run_setup
if jq -e '.providers.na.headers.authorization == "no-auth"' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "a whitespace-only override value does not count as an override and injection still happens"; else fail "a whitespace-only override value does not count as an override and injection still happens" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

# The bedrock-converse-stream rejection must also see a model-level api
# override: the provider-level api here is openai-completions (allowed with
# no_auth), but one model overrides to bedrock-converse-stream, which is
# still reachable through this provider and must still be rejected.
export PROVIDERS_INPUT='{"na":{"api":"openai-completions","base_url":"https://na.example","models":[{"id":"m","api":"bedrock-converse-stream"}],"no_auth":true}}'
assert_setup_fails "rejects no_auth when only a per-model api override is bedrock-converse-stream" "cannot set no_auth with api 'bedrock-converse-stream'"

# --- api allowlist (DESIGN.md Fix 3a): the 4 newly-allowed adapters
# generate successfully; the 3 deliberately-excluded ones fail, naming the
# api pi cannot drive from a static per-provider key.
export PROVIDERS_INPUT='{"p_ggi":{"api":"google-generative-ai","base_url":"https://ggi.example","models":["m"]},"p_azure":{"api":"azure-openai-responses","base_url":"https://azure.example","models":["m"]},"p_bedrock":{"api":"bedrock-converse-stream","base_url":"https://bedrock.example","models":["m"]},"p_cohere":{"api":"cohere-chat","base_url":"https://cohere.example","models":["m"]}}'
export PROVIDER_API_KEYS_INPUT=$'p_ggi=k1\np_azure=k2\np_bedrock=k3\np_cohere=k4'
run_setup
if jq -e '
    .providers.p_ggi.api == "google-generative-ai"
    and .providers.p_azure.api == "azure-openai-responses"
    and .providers.p_bedrock.api == "bedrock-converse-stream"
    and .providers.p_cohere.api == "cohere-chat"
  ' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "google-generative-ai, azure-openai-responses, bedrock-converse-stream, and cohere-chat all generate successfully"; else fail "google-generative-ai, azure-openai-responses, bedrock-converse-stream, and cohere-chat all generate successfully" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"x":{"api":"openai-codex-responses","base_url":"https://x","models":["m"]}}'
export PROVIDER_API_KEYS_INPUT='x=k'
assert_setup_fails "rejects openai-codex-responses (needs a ChatGPT OAuth JWT)" "has api 'openai-codex-responses'"

export PROVIDERS_INPUT='{"x":{"api":"google-gemini-cli","base_url":"https://x","models":["m"]}}'
assert_setup_fails "rejects google-gemini-cli (needs a GCP OAuth token)" "has api 'google-gemini-cli'"

export PROVIDERS_INPUT='{"x":{"api":"google-vertex","base_url":"https://x","models":["m"]}}'
assert_setup_fails "rejects google-vertex (needs a GCP-minted access token)" "has api 'google-vertex'"

# --- model objects (DESIGN.md Fix 3b): cost's all-four-or-none rule, the
# input allowlist, u32 bounds on context_window/max_tokens, cross-type
# duplicate-id detection, and the model-object key allowlist.
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":[{"id":"m","cost":{"input":1,"output":2,"cache_read":0.1,"cache_write":0.2}}]}}'
export PROVIDER_API_KEYS_INPUT='x=k'
run_setup
if jq -e '.providers.x.models[0].cost == {"input":1,"output":2,"cacheRead":0.1,"cacheWrite":0.2}' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "a full cost object succeeds and is emitted in camelCase"; else fail "a full cost object succeeds and is emitted in camelCase" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":[{"id":"m","cost":{"input":1,"output":2,"cache_read":0.1}}]}}'
assert_setup_fails "rejects a partial cost object (3 of 4 keys)" "must contain exactly 'input', 'output', 'cache_read', and 'cache_write'"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":[{"id":"m","cost":{"input":1,"output":2,"cache_read":0.1,"cache_write":0.2,"extra":9}}]}}'
assert_setup_fails "rejects an extra key inside cost" "must contain exactly 'input', 'output', 'cache_read', and 'cache_write'"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":[{"id":"m","input":["text","image"]}]}}'
run_setup
if jq -e '.providers.x.models[0].input == ["text","image"]' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "input: [text, image] succeeds"; else fail "input: [text, image] succeeds" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":[{"id":"m","input":["video"]}]}}'
assert_setup_fails "rejects an input value pi silently drops ('video')" "input contains an unsupported value 'video'"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":[{"id":"m","context_window":0}]}}'
assert_setup_fails "rejects context_window: 0" "context_window must be an integer > 0"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":[{"id":"m","max_tokens":1.5}]}}'
assert_setup_fails "rejects a non-integer max_tokens" "max_tokens must be an integer > 0"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m",{"id":"m"}]}}'
assert_setup_fails "rejects a string and an object model sharing the same id" "models must be a non-empty array of unique, non-empty model ids"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":[{"id":"m","bogus":true}]}}'
assert_setup_fails "rejects an unknown key in a model object" "model has unknown key 'bogus'"

# --- headers (DESIGN.md Fix 3b): '!'-prefixed values would run through
# pi's shell-exec resolver and are rejected outright; 'env:' stays allowed
# and passes through verbatim; header names must have no whitespace or ':';
# an empty value is rejected (pi's own resolver would drop it silently).
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"headers":{"X-Test":"!echo hi"}}}'
assert_setup_fails "rejects a header value starting with '!'" "must not start with '!'"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"headers":{"X-Test":"env:MY_VAR"}}}'
run_setup
if jq -e '.providers.x.headers["X-Test"] == "env:MY_VAR"' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "an env: header value succeeds and appears verbatim"; else fail "an env: header value succeeds and appears verbatim" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"headers":{"X Test":"v"}}}'
assert_setup_fails "rejects a header name containing a space" "invalid header name"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"headers":{"X:Test":"v"}}}'
assert_setup_fails "rejects a header name containing a colon" "invalid header name"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"headers":{"X-Test":""}}}'
assert_setup_fails "rejects an empty header value" "invalid value for header 'X-Test'"

# Control characters (CR/LF included) must be rejected in both header names
# and values: reqwest refuses to build a header from them, so pi would
# otherwise fail deep in the request path with a message that points
# nowhere near the workflow input that caused it. A literal '\r\n' cannot be
# spelled inside a single-quoted shell literal, so these three build the
# JSON with `jq -n --arg`, which encodes a real embedded control byte as the
# standard JSON '\r'/'\t' escape -- decoded back to the actual control byte
# when setup-env.sh parses PROVIDERS_INPUT, exactly as a workflow YAML value
# containing one would arrive.
export PROVIDERS_INPUT="$(jq -n --arg v $'tok\r\nmore' '{x:{api:"openai-completions",base_url:"https://x",models:["m"],headers:{"X-Test":$v}}}')"
export PROVIDER_API_KEYS_INPUT='x=k'
assert_setup_fails "rejects a header value containing an embedded CR+LF" "no control characters"

export PROVIDERS_INPUT="$(jq -n --arg n $'X-\x01Test' '{x:{api:"openai-completions",base_url:"https://x",models:["m"],headers:{($n):"v"}}}')"
assert_setup_fails "rejects a header name containing a control character" "invalid header name"

export PROVIDERS_INPUT="$(jq -n --arg v $'go\tod' '{x:{api:"openai-completions",base_url:"https://x",models:["m"],headers:{"X-Test":$v}}}')"
assert_setup_fails "rejects a header value containing a tab" "no control characters"

# --- compat (DESIGN.md Fix 3b): every field name is accepted and emitted
# camelCase via an explicit field map -- never a generic gsub, since
# custom_headers/thinking_level_map keys and open_router_routing/
# vercel_gateway_routing values are opaque pass-through data that a
# programmatic snake->camel transform would have no way to leave alone.
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"compat":{"supports_store":true,"supports_developer_role":true,"supports_reasoning_effort":true,"supports_usage_in_streaming":true,"supports_tools":true,"supports_streaming":true,"supports_parallel_tool_calls":true,"force_adaptive_thinking":true,"max_tokens_field":"max_completion_tokens","system_role_name":"developer","stop_reason_field":"finish_reason","thinking_format":"zai","custom_headers":{"anthropic-beta":"context-1m"},"thinking_level_map":{"high":"high"},"open_router_routing":{"provider":{"order":["x"]}},"vercel_gateway_routing":{"priority":1}}}}'
run_setup
EXPECTED_COMPAT='{"supportsStore":true,"supportsDeveloperRole":true,"supportsReasoningEffort":true,"supportsUsageInStreaming":true,"supportsTools":true,"supportsStreaming":true,"supportsParallelToolCalls":true,"forceAdaptiveThinking":true,"maxTokensField":"max_completion_tokens","systemRoleName":"developer","stopReasonField":"finish_reason","thinkingFormat":"zai","customHeaders":{"anthropic-beta":"context-1m"},"thinkingLevelMap":{"high":"high"},"openRouterRouting":{"provider":{"order":["x"]}},"vercelGatewayRouting":{"priority":1}}'
if jq -e --argjson expected "$EXPECTED_COMPAT" '.providers.x.compat == $expected' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "all 16 compat fields are accepted and emitted camelCase"; else fail "all 16 compat fields are accepted and emitted camelCase" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"compat":{"bogus_key":true}}}'
assert_setup_fails "rejects an unknown compat key" "unknown key 'bogus_key'"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"compat":{"supports_tools":"yes"}}}'
assert_setup_fails "rejects a wrong-typed compat field" "supports_tools must be a boolean"

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"compat":{"open_router_routing":{"provider":{"order":["openai","anthropic"]},"some_snake_key":"keep_me_snake"}}}}'
run_setup
if jq -e '.providers.x.compat.openRouterRouting == {"provider":{"order":["openai","anthropic"]},"some_snake_key":"keep_me_snake"}' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "open_router_routing passes through with inner keys unchanged (anti-gsub)"; else fail "open_router_routing passes through with inner keys unchanged (anti-gsub)" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"compat":{"custom_headers":{"Anthropic-Beta":"context-1m","X-Custom-Header":"v"}}}}'
run_setup
if jq -e '.providers.x.compat.customHeaders == {"Anthropic-Beta":"context-1m","X-Custom-Header":"v"}' "$RUNNER_TEMP/pi-agent/models.json" >/dev/null; then pass "compat.custom_headers keys keep their original casing and hyphens"; else fail "compat.custom_headers keys keep their original casing and hyphens" "$(cat "$RUNNER_TEMP/pi-agent/models.json")"; fi

# --- reserved built-in id collisions (DESIGN.md Fix 3c): a warning, not a
# hard failure, and only for an id that actually matches pi's built-in
# PROVIDER_METADATA snapshot (by canonical id or by alias).
export PROVIDERS_INPUT='{"anthropic":{"api":"anthropic-messages","base_url":"https://x","models":["m"]}}'
export PROVIDER_API_KEYS_INPUT='anthropic=k'
output="$(run_setup)"
if printf '%s\n' "$output" | grep -Fq "::warning::cruise: provider 'anthropic' collides with a pi built-in provider id"; then pass "a reserved canonical id (anthropic) emits a collision warning but still succeeds"; else fail "a reserved canonical id (anthropic) emits a collision warning but still succeeds" "$output"; fi

export PROVIDERS_INPUT='{"copilot":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'
export PROVIDER_API_KEYS_INPUT='copilot=k'
output="$(run_setup)"
if printf '%s\n' "$output" | grep -Fq "::warning::cruise: provider 'copilot' collides with a pi built-in provider id"; then pass "a reserved alias id (copilot) emits a collision warning but still succeeds"; else fail "a reserved alias id (copilot) emits a collision warning but still succeeds" "$output"; fi

export PROVIDERS_INPUT='{"my-totally-custom-gateway":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'
export PROVIDER_API_KEYS_INPUT='my-totally-custom-gateway=k'
output="$(run_setup)"
if printf '%s\n' "$output" | grep -Fq 'collides with a pi built-in provider id'; then fail "a non-colliding id produces no collision warning" "$output"; else pass "a non-colliding id produces no collision warning"; fi

# A colliding id combined with no_auth must be a hard failure, not just the
# advisory warning above: request dispatch matches the provider id before
# the 'api' field (providers/mod.rs:182-240), so which adapter actually runs
# -- and therefore which header would suppress its missing-key error --
# cannot be determined here, for either a reserved canonical id or a
# reserved alias.
unset PROVIDER_API_KEYS_INPUT
export PROVIDERS_INPUT='{"anthropic":{"api":"anthropic-messages","base_url":"https://x","models":["m"],"no_auth":true}}'
assert_setup_fails "rejects no_auth on a reserved canonical id (anthropic)" "cannot set no_auth because its id collides"

export PROVIDERS_INPUT='{"copilot":{"api":"openai-completions","base_url":"https://x","models":["m"],"no_auth":true}}'
assert_setup_fails "rejects no_auth on a reserved alias id (copilot)" "cannot set no_auth because its id collides"

unset PROVIDERS_INPUT PROVIDER_API_KEYS_INPUT PI_MODELS_JSON

: > "$GITHUB_OUTPUT"
EVENT="$TMP/event.json"; printf '%s' '{"action":"opened","issue":{"number":1,"user":{"login":"alice","id":1},"body":"@cruise run"}}' > "$EVENT"
GH="$TMP/gh"; printf '%s\n' '#!/usr/bin/env bash' 'echo write' > "$GH"; chmod +x "$GH"
export PATH="$TMP:$PATH"; hash -r
export GITHUB_EVENT_NAME=issues GITHUB_EVENT_PATH="$EVENT" GITHUB_REPOSITORY=owner/repo ALLOWED_BOTS= TRIGGER_PHRASE=@cruise
# Explicitly (re)set every credential-source *_INPUT var gate.sh reads,
# including PROVIDERS_INPUT/PI_MODELS_JSON -- both carry leftover non-empty
# values from the providers/pi_models_json cases above otherwise, which
# would make the "all empty" case below pass for the wrong reason (it'd
# still see a non-empty PROVIDERS_INPUT and never actually hit gate.sh's
# empty-credential branch at all).
export PROVIDER_API_KEYS_INPUT='x=k'; export ANTHROPIC_API_KEY_INPUT= OPENAI_API_KEY_INPUT= ENV_INPUT= PROVIDERS_INPUT= PI_MODELS_JSON=
if bash action/scripts/gate.sh >/dev/null; then pass "gate accepts provider_api_keys credential"; else fail "gate accepts provider_api_keys credential" "exit=$?"; fi

# gate.sh now also accepts `providers` and `pi_models_json` as credential
# sources on their own (DESIGN.md Fix 1's knock-on): a no_auth-only
# `providers` config, or a `pi_models_json` with its own literal apiKey,
# never shows up in `provider_api_keys` at all.
export PROVIDER_API_KEYS_INPUT=; export PROVIDERS_INPUT='{"x":{"api":"openai-completions"}}'
if bash action/scripts/gate.sh >/dev/null; then pass "gate accepts a non-empty providers credential source"; else fail "gate accepts a non-empty providers credential source" "exit=$?"; fi

export PROVIDERS_INPUT=; export PI_MODELS_JSON='{"providers":{}}'
if bash action/scripts/gate.sh >/dev/null; then pass "gate accepts a non-empty pi_models_json credential source"; else fail "gate accepts a non-empty pi_models_json credential source" "exit=$?"; fi

export PI_MODELS_JSON=; set +e; bash action/scripts/gate.sh >"$TMP/gate.out" 2>&1; status=$?; set -e
if [ "$status" -ne 0 ] && grep -Fq "'provider_api_keys', 'providers', 'pi_models_json', and 'env' are all empty" "$TMP/gate.out"; then pass "gate retains empty-credential failure with the updated message"; else fail "gate retains empty-credential failure with the updated message" "status=$status output=$(cat "$TMP/gate.out")"; fi
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
