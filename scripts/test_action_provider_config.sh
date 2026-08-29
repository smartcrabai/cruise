#!/usr/bin/env bash
# Exercises action/scripts/provision-jcode.sh (credentials and provider
# profiles landing in cruise's jcode home), the credential gate in
# action/scripts/gate.sh, and the `env` reserved-name handling in
# action/scripts/setup-env.sh.
#
# provision-jcode.sh drives two binaries it must not re-implement, so both
# are stubbed:
#   - `cruise`, for `login --status` (which is where the jcode home path
#     comes from) and `login --api-key <provider>` (the dedicated-key path).
#   - `jcode`, for `version --json` and `provider add`.
# The `jcode` stub reproduces the parts of jcode 0.81.2 this script depends
# on -- the `[providers.<name>]` table it appends to $JCODE_HOME/config.toml,
# the owner-only `provider-<name>.env` file it writes for a stdin key, its
# --json report, and its rejection of a non-http base_url -- so the
# assertions below can be made against real files. Every invocation is also
# logged verbatim, so the flags this action passes are pinned independently
# of what the stub then does with them.
. "$(dirname "${BASH_SOURCE[0]}")/lib/action_test_harness.sh"

FAKE_JCODE_HOME="$TMP/jcode-home"
export FAKE_JCODE_HOME
CONFIG_TOML="$FAKE_JCODE_HOME/config.toml"
ENV_DIR="$FAKE_JCODE_HOME/config/jcode"

stub cruise <<'SH'
#!/usr/bin/env bash
printf 'cruise %s\n' "$*" >> "$STUB_LOG"
if [ "$1" = "login" ] && [ "$2" = "--status" ]; then
  echo "cruise jcode home: $FAKE_JCODE_HOME"
  echo "providers: $(ls "$FAKE_JCODE_HOME/config/jcode" 2>/dev/null | tr '\n' ' ')"
  exit 0
fi
if [ "$1" = "login" ] && [ "$2" = "--api-key" ]; then
  provider="$3"
  # Mirrors what `cruise login --api-key` does through `jcode login`:
  # provider-specific env file, owner-only, holding one KEY=value line. The
  # key must arrive in CRUISE_LOGIN_API_KEY, never in the argument list.
  case "$provider" in
    anthropic-api) file=anthropic.env; var=ANTHROPIC_API_KEY ;;
    openai-api) file=openai.env; var=OPENAI_API_KEY ;;
    *) echo "unsupported provider '$provider'" >&2; exit 1 ;;
  esac
  if [ -z "${CRUISE_LOGIN_API_KEY:-}" ]; then
    echo "no key in CRUISE_LOGIN_API_KEY" >&2
    exit 1
  fi
  mkdir -p "$FAKE_JCODE_HOME/config/jcode"
  printf '%s=%s\n' "$var" "$CRUISE_LOGIN_API_KEY" > "$FAKE_JCODE_HOME/config/jcode/$file"
  chmod 600 "$FAKE_JCODE_HOME/config/jcode/$file"
  echo "Stored at $FAKE_JCODE_HOME/config/jcode/$file"
  exit 0
fi
exit 0
SH

stub jcode <<'SH'
#!/usr/bin/env bash
printf 'jcode %s\n' "$*" >> "$STUB_LOG"
printf 'jcode-env JCODE_HOME=%s JCODE_NO_TELEMETRY=%s\n' "${JCODE_HOME:-}" "${JCODE_NO_TELEMETRY:-}" >> "$STUB_LOG"
# Global flags precede the subcommand, the same placement cruise (and
# therefore provision-jcode.sh) uses.
while [ "${1:-}" = "--no-update" ]; do shift; done
case "${1:-}" in
  version)
    echo '{"version":"v0.81.2 (fake)","semver":"0.81.2"}'
    exit 0
    ;;
  provider)
    shift 2
    name="$1"; shift
    base_url=""; model=""; ctx=""; auth="bearer"; auth_header=""
    routing=0; set_default=0; keyed=0
    while [ $# -gt 0 ]; do
      case "$1" in
        --base-url) base_url="$2"; shift 2 ;;
        --model) model="$2"; shift 2 ;;
        --context-window) ctx="$2"; shift 2 ;;
        --auth) auth="$2"; shift 2 ;;
        --auth-header) auth_header="$2"; shift 2 ;;
        --provider-routing) routing=1; shift ;;
        --set-default) set_default=1; shift ;;
        --api-key-stdin) keyed=1; shift ;;
        --no-api-key) auth="none"; shift ;;
        --json) shift ;;
        # Any other argument is a flag the script started passing without
        # re-verifying it against real jcode -- fail loudly instead of
        # swallowing it.
        *) echo "Error: unexpected argument '$1'" >&2; exit 1 ;;
      esac
    done
    case "$base_url" in
      https://*|http://localhost*|http://127.0.0.1*) ;;
      *)
        echo "Error: Invalid --base-url '$base_url'. Use https://... or http://localhost/127.0.0.1/private-LAN for local servers." >&2
        exit 1
        ;;
    esac
    if [ -z "$model" ]; then
      echo "Error: --model cannot be empty" >&2
      exit 1
    fi
    if [ -n "$auth_header" ] && [ "$auth" != "api-key" ]; then
      echo "Error: --auth-header requires --auth api-key" >&2
      exit 1
    fi
    key_env=""
    env_file=""
    if [ "$keyed" -eq 1 ]; then
      upper="$(printf '%s' "$name" | tr 'a-z-' 'A-Z_')"
      key_env="JCODE_PROVIDER_${upper}_API_KEY"
      env_file="provider-$name.env"
      IFS= read -r stdin_key || stdin_key=""
      if [ -z "$stdin_key" ]; then
        echo "Error: --api-key-stdin was set, but stdin was empty" >&2
        exit 1
      fi
      mkdir -p "$JCODE_HOME/config/jcode"
      printf '%s=%s\n' "$key_env" "$stdin_key" > "$JCODE_HOME/config/jcode/$env_file"
      chmod 600 "$JCODE_HOME/config/jcode/$env_file"
    fi
    mkdir -p "$JCODE_HOME"
    {
      if [ "$set_default" -eq 1 ]; then
        printf '[provider]\ndefault_provider = "%s"\ndefault_model = "%s"\n\n' "$name" "$model"
      fi
      printf '[providers.%s]\n' "$name"
      printf 'type = "openai-compatible"\n'
      printf 'base_url = "%s"\n' "$base_url"
      printf 'auth = "%s"\n' "$auth"
      [ -n "$auth_header" ] && printf 'auth_header = "%s"\n' "$auth_header"
      if [ -n "$key_env" ]; then
        printf 'api_key_env = "%s"\n' "$key_env"
        printf 'env_file = "%s"\n' "$env_file"
      fi
      printf 'default_model = "%s"\n' "$model"
      if [ "$keyed" -eq 1 ]; then
        printf 'requires_api_key = true\n'
      else
        printf 'requires_api_key = false\n'
      fi
      [ "$routing" -eq 1 ] && printf 'provider_routing = true\n'
      printf '\n[[providers.%s.models]]\nid = "%s"\n' "$name" "$model"
      [ -n "$ctx" ] && printf 'context_window = %s\n' "$ctx"
      printf '\n'
    } >> "$JCODE_HOME/config.toml"
    if [ "$set_default" -eq 1 ]; then default_json=true; else default_json=false; fi
    if [ -n "$key_env" ]; then
      printf '{"status":"ok","profile":"%s","config_path":"%s","api_key_env":"%s","env_file":"%s","api_key_stored":true,"default_set":%s}\n' \
        "$name" "$JCODE_HOME/config.toml" "$key_env" "$env_file" "$default_json"
    else
      printf '{"status":"ok","profile":"%s","config_path":"%s","api_key_env":null,"env_file":null,"api_key_stored":false,"default_set":%s}\n' \
        "$name" "$JCODE_HOME/config.toml" "$default_json"
    fi
    exit 0
    ;;
  *)
    echo "jcode stub: unexpected invocation '$*'" >&2
    exit 1
    ;;
esac
exit 0
SH

run_provision() {
  new_case
  rm -rf "$FAKE_JCODE_HOME"
  mkdir -p "$FAKE_JCODE_HOME"
  ANTHROPIC_API_KEY_INPUT="${ANTHROPIC_API_KEY_INPUT:-}" \
  OPENAI_API_KEY_INPUT="${OPENAI_API_KEY_INPUT:-}" \
  PROVIDERS_INPUT="${PROVIDERS_INPUT:-}" \
  PROVIDER_API_KEYS_INPUT="${PROVIDER_API_KEYS_INPUT:-}" \
    bash action/scripts/provision-jcode.sh 2>&1
}

assert_provision_fails() { # $1=name $2=expected message fragment
  local out status
  out="$(run_provision)"
  status=$?
  if [ "$status" -ne 0 ] && printf '%s\n' "$out" | grep -Fq -e "$2"; then
    pass "$1"
  else
    fail "$1" "status=$status output=$out"
  fi
}

# ===========================================================================
# dedicated provider keys -> cruise's jcode home
# ===========================================================================
export ANTHROPIC_API_KEY_INPUT='sk-ant-secret' OPENAI_API_KEY_INPUT='sk-oai-secret'
unset PROVIDERS_INPUT PROVIDER_API_KEYS_INPUT
output="$(run_provision)"
if grep -Fqx 'ANTHROPIC_API_KEY=sk-ant-secret' "$ENV_DIR/anthropic.env" \
  && grep -Fqx 'OPENAI_API_KEY=sk-oai-secret' "$ENV_DIR/openai.env"; then
  pass "the dedicated keys land in <provider>.env files under cruise's jcode home"
else
  fail "the dedicated keys land in <provider>.env files under cruise's jcode home" "$(ls -l "$ENV_DIR" 2>&1)"
fi
# The credential files are jcode's, so their mode must be owner-only. 600 is
# what `cruise login --api-key` produces through jcode.
if [ "$(ls -l "$ENV_DIR/anthropic.env" | cut -c1-10)" = "-rw-------" ]; then
  pass "a stored provider key file is owner-only"
else
  fail "a stored provider key file is owner-only" "$(ls -l "$ENV_DIR/anthropic.env")"
fi
if grep -Fq 'cruise login --api-key anthropic-api' "$STUB_LOG" \
  && grep -Fq 'cruise login --api-key openai-api' "$STUB_LOG"; then
  pass "each dedicated key goes through 'cruise login --api-key <provider>'"
else
  fail "each dedicated key goes through 'cruise login --api-key <provider>'" "$(cat "$STUB_LOG")"
fi
# The key must travel in CRUISE_LOGIN_API_KEY (the stub hard-fails without
# it) and never as an argument, where a process listing would expose it.
if ! grep -Fq 'sk-ant-secret' "$STUB_LOG" && ! grep -Fq 'sk-oai-secret' "$STUB_LOG"; then
  pass "no dedicated key literal ever reaches an argument list"
else
  fail "no dedicated key literal ever reaches an argument list" "$(cat "$STUB_LOG")"
fi
if printf '%s\n' "$output" | grep -Fq '::add-mask::sk-ant-secret' \
  && printf '%s\n' "$output" | grep -Fq '::add-mask::sk-oai-secret'; then
  pass "masks every dedicated provider key"
else
  fail "masks every dedicated provider key" "$output"
fi
# No config.toml is written when there is nothing but dedicated keys: the
# profile tables are the only thing this action puts there.
if [ ! -f "$CONFIG_TOML" ]; then
  pass "a dedicated-key-only run writes no config.toml"
else
  fail "a dedicated-key-only run writes no config.toml" "$(cat "$CONFIG_TOML")"
fi

# Every jcode invocation must be bound to cruise's home with telemetry off.
if grep -Fq "jcode-env JCODE_HOME=$FAKE_JCODE_HOME JCODE_NO_TELEMETRY=1" "$STUB_LOG"; then
  pass "jcode runs against cruise's own JCODE_HOME with telemetry disabled"
else
  fail "jcode runs against cruise's own JCODE_HOME with telemetry disabled" "$(cat "$STUB_LOG")"
fi
if grep -Fqx 'jcode --no-update version --json' "$STUB_LOG"; then
  pass "the jcode version probe suppresses the auto-update check"
else
  fail "the jcode version probe suppresses the auto-update check" "$(cat "$STUB_LOG")"
fi
# The logged version is the bare `semver` field, not the decorated
# "v0.81.2 (fake)" human string -- cruise's floor check reads the same field.
if printf '%s\n' "$output" | grep -Fqx 'cruise: jcode 0.81.2'; then
  pass "the version log prints jcode's semver field"
else
  fail "the version log prints jcode's semver field" "$output"
fi

# A cruise that reports no jcode home is a hard failure, not a silently
# guessed path: everything below writes into that directory.
new_case
stub cruise <<'SH'
#!/usr/bin/env bash
printf 'cruise %s\n' "$*" >> "$STUB_LOG"
echo "some other output"
SH
out="$(ANTHROPIC_API_KEY_INPUT= OPENAI_API_KEY_INPUT= PROVIDERS_INPUT= PROVIDER_API_KEYS_INPUT= \
  bash action/scripts/provision-jcode.sh 2>&1)"
status=$?
if [ "$status" -ne 0 ] && printf '%s\n' "$out" | grep -Fq 'did not report a jcode home'; then
  pass "a cruise that reports no jcode home fails the step"
else
  fail "a cruise that reports no jcode home fails the step" "status=$status output=$out"
fi

# ===========================================================================
# providers -> [providers.<name>] profiles in cruise's jcode config.toml
# ===========================================================================
# Restore the full cruise stub (the case above deliberately replaced it).
stub cruise <<'SH'
#!/usr/bin/env bash
printf 'cruise %s\n' "$*" >> "$STUB_LOG"
if [ "$1" = "login" ] && [ "$2" = "--status" ]; then
  echo "cruise jcode home: $FAKE_JCODE_HOME"
  echo "providers: $(ls "$FAKE_JCODE_HOME/config/jcode" 2>/dev/null | tr '\n' ' ')"
  exit 0
fi
if [ "$1" = "login" ] && [ "$2" = "--api-key" ]; then
  provider="$3"
  case "$provider" in
    anthropic-api) file=anthropic.env; var=ANTHROPIC_API_KEY ;;
    openai-api) file=openai.env; var=OPENAI_API_KEY ;;
    *) echo "unsupported provider '$provider'" >&2; exit 1 ;;
  esac
  if [ -z "${CRUISE_LOGIN_API_KEY:-}" ]; then
    echo "no key in CRUISE_LOGIN_API_KEY" >&2
    exit 1
  fi
  mkdir -p "$FAKE_JCODE_HOME/config/jcode"
  printf '%s=%s\n' "$var" "$CRUISE_LOGIN_API_KEY" > "$FAKE_JCODE_HOME/config/jcode/$file"
  chmod 600 "$FAKE_JCODE_HOME/config/jcode/$file"
  echo "Stored at $FAKE_JCODE_HOME/config/jcode/$file"
  exit 0
fi
exit 0
SH

unset ANTHROPIC_API_KEY_INPUT OPENAI_API_KEY_INPUT
export ANTHROPIC_API_KEY_INPUT= OPENAI_API_KEY_INPUT=
export PROVIDERS_INPUT='{"gw-two":{"base_url":"https://two.example/v1","model":"m2"},"gw-one":{"base_url":"https://one.example/v1","model":"m1","context_window":128000,"default":true}}'
export PROVIDER_API_KEYS_INPUT=$'gw-two=two-key==\n# ignored\ngw-one=one-key'
output="$(run_provision)"
if grep -Fqx '[providers.gw-one]' "$CONFIG_TOML" && grep -Fqx '[providers.gw-two]' "$CONFIG_TOML" \
  && grep -Fqx 'base_url = "https://one.example/v1"' "$CONFIG_TOML" \
  && grep -Fqx 'base_url = "https://two.example/v1"' "$CONFIG_TOML" \
  && grep -Fqx 'default_model = "m1"' "$CONFIG_TOML" \
  && grep -Fqx 'context_window = 128000' "$CONFIG_TOML"; then
  pass "each providers entry becomes a [providers.<name>] profile in cruise's jcode config.toml"
else
  fail "each providers entry becomes a [providers.<name>] profile in cruise's jcode config.toml" "$(cat "$CONFIG_TOML" 2>&1)"
fi
if grep -Fqx 'JCODE_PROVIDER_GW_ONE_API_KEY=one-key' "$ENV_DIR/provider-gw-one.env" \
  && grep -Fqx 'JCODE_PROVIDER_GW_TWO_API_KEY=two-key==' "$ENV_DIR/provider-gw-two.env"; then
  pass "each profile key lands in its own env file, equals signs preserved"
else
  fail "each profile key lands in its own env file, equals signs preserved" "$(ls -l "$ENV_DIR" 2>&1)"
fi
# config.toml references the key by environment-variable name only. A key
# literal in there would be committed-adjacent plaintext in a file cruise
# and jcode both read on every run.
if grep -Fqx 'api_key_env = "JCODE_PROVIDER_GW_ONE_API_KEY"' "$CONFIG_TOML" \
  && ! grep -Fq 'one-key' "$CONFIG_TOML" && ! grep -Fq 'two-key' "$CONFIG_TOML"; then
  pass "no raw profile key literal in the generated config.toml"
else
  fail "no raw profile key literal in the generated config.toml" "$(cat "$CONFIG_TOML")"
fi
if printf '%s\n' "$output" | grep -Fq '::add-mask::one-key' \
  && printf '%s\n' "$output" | grep -Fq '::add-mask::two-key=='; then
  pass "masks every profile API key"
else
  fail "masks every profile API key" "$output"
fi
# Keys go in on stdin (`--api-key-stdin`), so they must not appear in any
# recorded argument list.
if grep -Fq 'jcode --no-update provider add gw-one --base-url https://one.example/v1 --model m1 --context-window 128000 --set-default --api-key-stdin --json' "$STUB_LOG"; then
  pass "a keyed profile is added with --api-key-stdin and its declared options"
else
  fail "a keyed profile is added with --api-key-stdin and its declared options" "$(cat "$STUB_LOG")"
fi
if ! grep -Fq 'one-key' "$STUB_LOG" && ! grep -Fq 'two-key' "$STUB_LOG"; then
  pass "no profile key literal ever reaches an argument list"
else
  fail "no profile key literal ever reaches an argument list" "$(cat "$STUB_LOG")"
fi
if grep -Fqx 'default_provider = "gw-one"' "$CONFIG_TOML"; then
  pass "'default: true' makes the profile jcode's startup default provider"
else
  fail "'default: true' makes the profile jcode's startup default provider" "$(cat "$CONFIG_TOML")"
fi
if ! grep -Fq -- '--set-default' <<< "$(grep 'jcode provider add gw-two' "$STUB_LOG")"; then
  pass "a non-default profile is added without --set-default"
else
  fail "a non-default profile is added without --set-default" "$(cat "$STUB_LOG")"
fi

# Whitespace around a provider_api_keys value is trimmed, as before.
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m"}}'
export PROVIDER_API_KEYS_INPUT='x = mykey'
run_provision >/dev/null
if grep -Fqx 'JCODE_PROVIDER_X_API_KEY=mykey' "$ENV_DIR/provider-x.env"; then
  pass "trims whitespace around a profile api key"
else
  fail "trims whitespace around a profile api key" "$(cat "$ENV_DIR/provider-x.env" 2>&1)"
fi

# --- optional fields map onto jcode's own flags ---------------------------
export PROVIDERS_INPUT='{"hdr":{"base_url":"https://h.example/v1","model":"m","auth":"api-key","auth_header":"X-Key","provider_routing":true}}'
export PROVIDER_API_KEYS_INPUT='hdr=k'
run_provision >/dev/null
if grep -Fq 'jcode --no-update provider add hdr --base-url https://h.example/v1 --model m --auth api-key --auth-header X-Key --provider-routing --api-key-stdin --json' "$STUB_LOG" \
  && grep -Fqx 'auth = "api-key"' "$CONFIG_TOML" && grep -Fqx 'auth_header = "X-Key"' "$CONFIG_TOML" \
  && grep -Fqx 'provider_routing = true' "$CONFIG_TOML"; then
  pass "auth/auth_header/provider_routing map onto the matching jcode flags"
else
  fail "auth/auth_header/provider_routing map onto the matching jcode flags" "$(cat "$STUB_LOG")"
fi

# --- no_auth: omitted from provider_api_keys, added with --no-api-key -----
export PROVIDERS_INPUT='{"na":{"base_url":"http://localhost:1234/v1","model":"m","no_auth":true}}'
unset PROVIDER_API_KEYS_INPUT
export PROVIDER_API_KEYS_INPUT=
run_provision >/dev/null
if grep -Fq 'jcode --no-update provider add na --base-url http://localhost:1234/v1 --model m --no-api-key --json' "$STUB_LOG" \
  && grep -Fqx 'auth = "none"' "$CONFIG_TOML" \
  && ! grep -Fq 'api_key_env' "$CONFIG_TOML" \
  && [ ! -f "$ENV_DIR/provider-na.env" ]; then
  pass "a no_auth profile is added key-less, with no api_key_env and no env file"
else
  fail "a no_auth profile is added key-less, with no api_key_env and no env file" "$(cat "$CONFIG_TOML" 2>&1)"
fi

export PROVIDERS_INPUT='{"keyed":{"base_url":"https://keyed.example/v1","model":"m"},"na":{"base_url":"http://localhost:1/v1","model":"m","no_auth":true}}'
export PROVIDER_API_KEYS_INPUT='keyed=thekey'
run_provision >/dev/null
if grep -Fqx 'JCODE_PROVIDER_KEYED_API_KEY=thekey' "$ENV_DIR/provider-keyed.env" \
  && [ ! -f "$ENV_DIR/provider-na.env" ]; then
  pass "a keyed + no_auth mix stores a key for the keyed profile only"
else
  fail "a keyed + no_auth mix stores a key for the keyed profile only" "$(ls -l "$ENV_DIR" 2>&1)"
fi

# ===========================================================================
# providers / provider_api_keys rejections
# ===========================================================================
export PROVIDERS_INPUT='not-json'; export PROVIDER_API_KEYS_INPUT='x=k'
assert_provision_fails "rejects malformed JSON" "'providers' must be a non-empty JSON object"
export PROVIDERS_INPUT='[]'
assert_provision_fails "rejects non-object JSON" "'providers' must be a non-empty JSON object"
export PROVIDERS_INPUT='{}'
assert_provision_fails "rejects empty provider map" "'providers' must be a non-empty JSON object"
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m"}}'; export PROVIDER_API_KEYS_INPUT=
assert_provision_fails "rejects one-sided providers" "must be set together"
export PROVIDER_API_KEYS_INPUT='x=k'; export PROVIDERS_INPUT=
assert_provision_fails "rejects one-sided keys" "must be set together"
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m"}}'; export PROVIDER_API_KEYS_INPUT='y=k'
assert_provision_fails "rejects missing provider key" "missing provider profile name 'x'"
export PROVIDER_API_KEYS_INPUT=$'x=k\nx=two'
assert_provision_fails "rejects duplicate provider key" "duplicate provider profile name 'x'"
export PROVIDER_API_KEYS_INPUT=$'x=k\ny=k2'
assert_provision_fails "rejects extra provider key" "unknown provider profile name 'y'"
export PROVIDER_API_KEYS_INPUT='xk'
assert_provision_fails "rejects provider_api_keys line without '='" "expected profile-name=API key"
export PROVIDER_API_KEYS_INPUT='x='
assert_provision_fails "rejects empty provider api key" "has an empty API key"
export PROVIDER_API_KEYS_INPUT='x=   '
assert_provision_fails "rejects whitespace-only provider api key" "has an empty API key"
export PROVIDER_API_KEYS_INPUT='bad id=k'
assert_provision_fails "rejects invalid profile name appearing only in provider_api_keys" \
  "invalid provider profile name 'bad id' in 'provider_api_keys'"

# jcode's profile-name rule (ASCII letters/numbers/-/_, starting with a
# letter or number, <= 64 chars) is enforced here rather than left to jcode:
# a name starting with "-" would reach `jcode provider add` as a flag, and a
# "." -- which the previous, pi-shaped `providers` input allowed -- is not a
# legal jcode profile name at all.
export PROVIDERS_INPUT='{"bad id":{"base_url":"https://x/v1","model":"m"}}'; export PROVIDER_API_KEYS_INPUT='bad id=k'
assert_provision_fails "rejects a profile name with a space" "invalid provider profile name 'bad id'"
export PROVIDERS_INPUT='{"my.gw":{"base_url":"https://x/v1","model":"m"}}'; export PROVIDER_API_KEYS_INPUT='my.gw=k'
assert_provision_fails "rejects a profile name containing a dot" "invalid provider profile name 'my.gw'"
export PROVIDERS_INPUT='{"-lead":{"base_url":"https://x/v1","model":"m"}}'; export PROVIDER_API_KEYS_INPUT='-lead=k'
assert_provision_fails "rejects a profile name starting with a dash" "invalid provider profile name '-lead'"

export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m","oops":true}}'; export PROVIDER_API_KEYS_INPUT='x=k'
assert_provision_fails "rejects unknown fields" "has unknown key 'oops'"
export PROVIDERS_INPUT='{"x":["nope"]}'
assert_provision_fails "rejects a non-object provider entry" "provider 'x' must be a JSON object"
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1"}}'
assert_provision_fails "rejects a missing model" "model must be a string"
export PROVIDERS_INPUT='{"x":{"model":"m"}}'
assert_provision_fails "rejects a missing base_url" "base_url must be a string"
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m","context_window":"big"}}'
assert_provision_fails "rejects a non-numeric context_window" "context_window must be a number"
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m","auth":"bogus"}}'
assert_provision_fails "rejects an auth style jcode has no flag value for" "auth must be 'bearer' or 'api-key'"
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m","no_auth":"yes"}}'
assert_provision_fails "rejects a non-boolean no_auth" "no_auth must be a boolean"
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m","default":"yes"}}'
assert_provision_fails "rejects a non-boolean default" "default must be a boolean"
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m","provider_routing":1}}'
assert_provision_fails "rejects a non-boolean provider_routing" "provider_routing must be a boolean"

# Only one profile can be jcode's startup default, so two of them is a
# configuration error rather than a last-one-wins race.
export PROVIDERS_INPUT='{"a":{"base_url":"https://a/v1","model":"m","default":true},"b":{"base_url":"https://b/v1","model":"m","default":true}}'
export PROVIDER_API_KEYS_INPUT=$'a=k\nb=k'
assert_provision_fails "rejects two providers claiming default: true" \
  "at most one 'providers' entry may set 'default: true'"

export PROVIDERS_INPUT='{"na":{"base_url":"http://localhost:1/v1","model":"m","no_auth":true}}'
export PROVIDER_API_KEYS_INPUT='na=somekey'
assert_provision_fails "rejects a no_auth profile listed in provider_api_keys" \
  "is 'no_auth' and must not appear in 'provider_api_keys'"

# Value domains this action deliberately leaves to `jcode provider add` must
# still surface as a clear, profile-named ::error:: rather than a bare
# non-zero exit somewhere in the log.
export PROVIDERS_INPUT='{"x":{"base_url":"ftp://x","model":"m"}}'; export PROVIDER_API_KEYS_INPUT='x=k'
assert_provision_fails "surfaces jcode's non-http base_url rejection as a named error" \
  "::error::cruise: \`jcode provider add x\` failed"
export PROVIDERS_INPUT='{"x":{"base_url":"ftp://x","model":"m"}}'
assert_provision_fails "keeps jcode's own diagnosis in the error" "Invalid --base-url 'ftp://x'"
export PROVIDERS_INPUT='{"x":{"base_url":"https://x/v1","model":"m","auth":"bearer","auth_header":"X-K"}}'
assert_provision_fails "surfaces jcode's auth_header/auth pairing rejection" \
  "--auth-header requires --auth api-key"

unset PROVIDERS_INPUT PROVIDER_API_KEYS_INPUT ANTHROPIC_API_KEY_INPUT OPENAI_API_KEY_INPUT

# A jcode below cruise's floor -- e.g. pre-existing on a self-hosted runner,
# where install-jcode.sh skips the install -- is refused with the floor
# named, before any credential is written. (No stub restore below: the
# remaining sections never invoke jcode.)
stub jcode <<'SH'
#!/usr/bin/env bash
printf 'jcode %s\n' "$*" >> "$STUB_LOG"
while [ "${1:-}" = "--no-update" ]; do shift; done
if [ "${1:-}" = "version" ]; then
  echo '{"version":"v0.80.9 (fake)","semver":"0.80.9"}'
  exit 0
fi
echo "jcode stub: unexpected invocation '$*'" >&2
exit 1
SH
assert_provision_fails "a jcode below cruise's floor is refused with the floor named" \
  "jcode 0.80.9 is too old for this version of the action (requires jcode v0.81.1 or newer"

# ===========================================================================
# gate.sh credential sources
# ===========================================================================
: > "$GITHUB_OUTPUT"
EVENT="$TMP/event.json"
must printf '%s' '{"action":"opened","issue":{"number":1,"user":{"login":"alice","id":1},"body":"@cruise run"}}' > "$EVENT"
GH="$TMP/gh"
must printf '%s\n' '#!/usr/bin/env bash' 'echo write' > "$GH"
must chmod +x "$GH"
export PATH="$TMP:$PATH"; hash -r
export GITHUB_EVENT_NAME=issues GITHUB_EVENT_PATH="$EVENT" GITHUB_REPOSITORY=owner/repo ALLOWED_BOTS= TRIGGER_PHRASE=@cruise
# Explicitly (re)set every credential-source *_INPUT var gate.sh reads,
# including PROVIDERS_INPUT -- it carries a leftover non-empty value from the
# providers cases above otherwise, which would make the "all empty" case
# below pass for the wrong reason (it'd still see a non-empty PROVIDERS_INPUT
# and never actually hit gate.sh's empty-credential branch at all).
export PROVIDER_API_KEYS_INPUT='x=k'; export ANTHROPIC_API_KEY_INPUT= OPENAI_API_KEY_INPUT= ENV_INPUT= PROVIDERS_INPUT=
if bash action/scripts/gate.sh >/dev/null; then pass "gate accepts provider_api_keys credential"; else fail "gate accepts provider_api_keys credential" "exit=$?"; fi

# gate.sh also accepts `providers` as a credential source on its own: a
# no_auth-only `providers` config never shows up in `provider_api_keys` at
# all.
export PROVIDER_API_KEYS_INPUT=; export PROVIDERS_INPUT='{"x":{"base_url":"http://localhost:1/v1","model":"m","no_auth":true}}'
if bash action/scripts/gate.sh >/dev/null; then pass "gate accepts a non-empty providers credential source"; else fail "gate accepts a non-empty providers credential source" "exit=$?"; fi

export PROVIDERS_INPUT=
bash action/scripts/gate.sh >"$TMP/gate.out" 2>&1; status=$?
if [ "$status" -ne 0 ] && grep -Fq "'provider_api_keys', 'providers', and 'env' are all empty" "$TMP/gate.out"; then pass "gate retains empty-credential failure with the updated message"; else fail "gate retains empty-credential failure with the updated message" "status=$status output=$(cat "$TMP/gate.out")"; fi

# ===========================================================================
# setup-env.sh: model overrides and the `env` input's reserved names
# ===========================================================================
# This loop had no coverage at all, which is exactly how action.yml's and
# docs/github-actions.md's reserved-name lists drifted from RESERVED_KEYS
# without anything noticing: both claimed ANTHROPIC_API_KEY/OPENAI_API_KEY
# were reserved when the code has never treated them that way, and gate.sh
# deliberately counts a non-empty `env` as a credential source so a provider
# key CAN be passed there. These cases pin the real behaviour so the prose
# can be checked against something executable.
run_setup() {
  new_case
  MODEL_INPUT="${MODEL_INPUT:-}" PLAN_MODEL_INPUT="${PLAN_MODEL_INPUT:-}" ENV_INPUT="${ENV_INPUT:-}" \
    bash action/scripts/setup-env.sh 2>&1
}

unset PROVIDERS_INPUT PROVIDER_API_KEYS_INPUT ANTHROPIC_API_KEY_INPUT OPENAI_API_KEY_INPUT ENV_INPUT

# The model inputs are plain CRUISE_MODEL/CRUISE_PLAN_MODEL overrides in
# cruise's jcode `provider/model[:effort]` reference format, forwarded
# verbatim -- effort suffix and all -- for cruise itself to split.
export MODEL_INPUT='claude/claude-opus-4-6:xhigh' PLAN_MODEL_INPUT='openai-api/gpt-5.5'
run_setup >/dev/null
if grep -Fqx 'CRUISE_MODEL=claude/claude-opus-4-6:xhigh' "$GITHUB_ENV" \
  && grep -Fqx 'CRUISE_PLAN_MODEL=openai-api/gpt-5.5' "$GITHUB_ENV"; then
  pass "model/plan_model are exported verbatim as provider/model[:effort] references"
else
  fail "model/plan_model are exported verbatim as provider/model[:effort] references" "$(cat "$GITHUB_ENV")"
fi
# No CRUISE_SDK: this action relies on cruise's own default backend, so a
# repository config naming a different one keeps working.
if ! grep -q '^CRUISE_SDK=' "$GITHUB_ENV"; then
  pass "setup-env exports no CRUISE_SDK"
else
  fail "setup-env exports no CRUISE_SDK" "$(cat "$GITHUB_ENV")"
fi
if grep -Fqx 'CRUISE_FORCE_EXEC=false' "$GITHUB_ENV"; then
  pass "setup-env pins CRUISE_FORCE_EXEC=false"
else
  fail "setup-env pins CRUISE_FORCE_EXEC=false" "$(cat "$GITHUB_ENV")"
fi
export MODEL_INPUT= PLAN_MODEL_INPUT=
run_setup >/dev/null
if ! grep -q '^CRUISE_MODEL=' "$GITHUB_ENV" && ! grep -q '^CRUISE_PLAN_MODEL=' "$GITHUB_ENV"; then
  pass "empty model inputs are not exported at all"
else
  fail "empty model inputs are not exported at all" "$(cat "$GITHUB_ENV")"
fi

env_case() { # $1=unused $2=ENV_INPUT
  export ENV_INPUT="$2"
  env_out="$(run_setup)"
}

# Not reserved: both dedicated-input names pass through to $GITHUB_ENV.
env_case "" 'ANTHROPIC_API_KEY=from-env'
if grep -Fqx 'ANTHROPIC_API_KEY=from-env' "$GITHUB_ENV" && ! printf '%s\n' "$env_out" | grep -Fq 'ignoring'; then
  pass "env: ANTHROPIC_API_KEY is NOT reserved (matches RESERVED_KEYS, not the old docs)"
else
  fail "env: ANTHROPIC_API_KEY is NOT reserved" "$env_out / $(cat "$GITHUB_ENV")"
fi

env_case "" 'OPENAI_API_KEY=from-env'
if grep -Fqx 'OPENAI_API_KEY=from-env' "$GITHUB_ENV"; then pass "env: OPENAI_API_KEY is NOT reserved"; else fail "env: OPENAI_API_KEY is NOT reserved" "$(cat "$GITHUB_ENV")"; fi

# Reserved: every literal name in RESERVED_KEYS plus the four prefix rules.
for key in GITHUB_TOKEN GH_TOKEN PATH HOME SHELL \
           GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL \
           XDG_DATA_HOME XDG_CONFIG_HOME XDG_STATE_HOME \
           CRUISE_MODEL CRUISE_SDK CRUISE_CONFIG GITHUB_ANYTHING ACTIONS_ANYTHING RUNNER_ANYTHING; do
  env_case "" "$key=should-be-dropped"
  if printf '%s\n' "$env_out" | grep -Fq "::warning::cruise: ignoring 'env' entry for '$key'" \
     && ! grep -Fqx "$key=should-be-dropped" "$GITHUB_ENV"; then
    pass "env: $key is reserved and skipped with a warning"
  else
    fail "env: $key is reserved and skipped with a warning" "$env_out"
  fi
done

# CRUISE_* gets its own message pointing at the dedicated inputs.
env_case "" 'CRUISE_MODEL=x'
if printf '%s\n' "$env_out" | grep -Fq 'dedicated inputs (model/plan_model/config)'; then pass "env: CRUISE_* warning names the dedicated inputs"; else fail "env: CRUISE_* warning names the dedicated inputs" "$env_out"; fi

# An ordinary provider key passes through and is masked -- the pattern
# gate.sh's error message explicitly advertises (e.g. KIMI_API_KEY via env).
env_case "" 'KIMI_API_KEY=kimi-secret'
if grep -Fqx 'KIMI_API_KEY=kimi-secret' "$GITHUB_ENV" && printf '%s\n' "$env_out" | grep -Fq '::add-mask::kimi-secret'; then
  pass "env: a provider key passes through and is masked"
else
  fail "env: a provider key passes through and is masked" "$env_out / $(cat "$GITHUB_ENV")"
fi
unset ENV_INPUT

# --- action.yml and docs must describe the SAME reserved names the code
# enforces. This is the check that would have caught the drift above; it
# reads RESERVED_KEYS out of the script rather than hardcoding a second copy.
reserved_line="$(grep '^RESERVED_KEYS=' action/scripts/setup-env.sh | sed 's/^RESERVED_KEYS="//; s/"$//')"
if [ -z "$reserved_line" ]; then
  echo "FATAL: could not extract RESERVED_KEYS from action/scripts/setup-env.sh" >&2
  exit 1
fi
env_desc="$(sed -n '/^  env:/,/^  providers:/p' action.yml)"
drift=""
for key in $reserved_line; do
  case "$key" in
    GIT_AUTHOR_*|GIT_COMMITTER_*|XDG_*) continue ;;  # covered by a collective phrase
  esac
  printf '%s' "$env_desc" | grep -Fq "$key" || drift="$drift $key(missing-from-action.yml)"
  grep -Fq "\`$key\`" docs/github-actions.md || drift="$drift $key(missing-from-docs)"
done
for key in ANTHROPIC_API_KEY OPENAI_API_KEY; do
  case " $reserved_line " in *" $key "*) drift="$drift $key(unexpectedly-in-RESERVED_KEYS)" ;; esac
  printf '%s' "$env_desc" | grep -Fq "NOT reserved" || drift="$drift action.yml-missing-NOT-reserved-note"
  break
done
if [ -z "$drift" ]; then pass "action.yml and docs list the same reserved names the code enforces"; else fail "action.yml and docs list the same reserved names the code enforces" "drift:$drift"; fi

finish
