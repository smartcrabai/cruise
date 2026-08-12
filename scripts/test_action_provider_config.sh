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
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"],"oops":true}}'; assert_setup_fails "rejects unknown fields" "must contain exactly"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='x=k'; export PI_MODELS_JSON='{}'; assert_setup_fails "rejects registry combination" "cannot be combined"
unset PI_MODELS_JSON
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='xk'; assert_setup_fails "rejects provider_api_keys line without '='" "expected provider-id=API key"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='x='; assert_setup_fails "rejects empty provider api key" "has an empty API key"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='x=   '; assert_setup_fails "rejects whitespace-only provider api key" "has an empty API key"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='bad id=k'; assert_setup_fails "rejects invalid provider id appearing only in provider_api_keys" "invalid provider id 'bad id' in 'provider_api_keys'"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x\n","models":["m"]}}'; export PROVIDER_API_KEYS_INPUT='x=k'; assert_setup_fails "rejects base_url ending in a newline" "invalid base_url"
export PROVIDERS_INPUT='{"x":{"api":"openai-completions","base_url":"https://x","models":["   "]}}'; export PROVIDER_API_KEYS_INPUT='x=k'; assert_setup_fails "rejects whitespace-only model id" "models must be"
export PROVIDERS_INPUT=$'{"x":{"api":"openai-completions","base_url":"https://x","models":["m\\n"]}}'; assert_setup_fails "rejects model id ending in a newline" "models must be"

: > "$GITHUB_OUTPUT"
EVENT="$TMP/event.json"; printf '%s' '{"action":"opened","issue":{"number":1,"user":{"login":"alice","id":1},"body":"@cruise run"}}' > "$EVENT"
GH="$TMP/gh"; printf '%s\n' '#!/usr/bin/env bash' 'echo write' > "$GH"; chmod +x "$GH"
export PATH="$TMP:$PATH"; hash -r
export GITHUB_EVENT_NAME=issues GITHUB_EVENT_PATH="$EVENT" GITHUB_REPOSITORY=owner/repo ALLOWED_BOTS= TRIGGER_PHRASE=@cruise
export PROVIDER_API_KEYS_INPUT='x=k'; export ANTHROPIC_API_KEY_INPUT= OPENAI_API_KEY_INPUT= ENV_INPUT=
if bash action/scripts/gate.sh >/dev/null; then pass "gate accepts provider_api_keys credential"; else fail "gate accepts provider_api_keys credential" "exit=$?"; fi
export PROVIDER_API_KEYS_INPUT=; set +e; bash action/scripts/gate.sh >"$TMP/gate.out" 2>&1; status=$?; set -e
if [ "$status" -ne 0 ] && grep -Fq "'provider_api_keys', and 'env' are all empty" "$TMP/gate.out"; then pass "gate retains empty-credential failure"; else fail "gate retains empty-credential failure" "status=$status output=$(cat "$TMP/gate.out")"; fi
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
