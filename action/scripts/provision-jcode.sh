#!/usr/bin/env bash
# Puts this run's model credentials into cruise's own jcode home: the
# dedicated `anthropic_api_key` / `openai_api_key` inputs become jcode
# provider logins, and every `providers` entry becomes a
# `[providers.<name>]` OpenAI-compatible profile in that home's config.toml.
#
# Both halves are delegated rather than re-implemented:
#   - `cruise login --api-key <provider>` is cruise's own non-interactive
#     credential entry point. It hands the key to `jcode login`, so the
#     storage location and file mode are jcode's, and the key travels in
#     CRUISE_LOGIN_API_KEY instead of an argument list.
#   - `jcode provider add` writes the `[providers.<name>]` table and stores
#     the profile's key in a private, owner-only env file, referenced from
#     config.toml by `api_key_env` only.
# Nothing here writes TOML or a credential file itself, so this action cannot
# drift from the schema jcode actually reads, and no key is ever written to a
# cruise config file.
#
# The home is whatever cruise reports (`cruise login --status` prints it on
# its first line): cruise derives it from its own data directory, and a
# second, hand-computed copy of that path here would silently desync from it.
set -euo pipefail

ANTHROPIC_API_KEY_INPUT="${ANTHROPIC_API_KEY_INPUT:-}"
OPENAI_API_KEY_INPUT="${OPENAI_API_KEY_INPUT:-}"
PROVIDERS_INPUT="${PROVIDERS_INPUT:-}"
PROVIDER_API_KEYS_INPUT="${PROVIDER_API_KEYS_INPUT:-}"

# Field allowlist for a `providers` entry, mapped 1:1 onto `jcode provider
# add` flags below. Anything else is a typo or a jcode option this action
# does not expose, and is rejected rather than silently dropped.
PROVIDER_KEYS='["base_url","model","context_window","auth","auth_header","provider_routing","no_auth","default"]'

provider_config_error() {
  echo "::error::cruise: invalid provider configuration: $1" >&2
  exit 1
}

TMP_FILES=()
# bash 3.2 (macOS /bin/bash) treats expanding an empty array under `set -u`
# as an unbound variable, so the `${...@+...}` guard keeps an early exit
# before the first new_tmp from turning into trap noise.
trap 'rm -f ${TMP_FILES[@]+"${TMP_FILES[@]}"}' EXIT
new_tmp() {
  local f
  f="$(mktemp)"
  TMP_FILES+=("$f")
  printf '%s' "$f"
}

# --- cruise's jcode home ---------------------------------------------------
if ! status_out="$(cruise login --status 2>&1)"; then
  echo "::error::cruise: could not read cruise's jcode home via \`cruise login --status\`: $status_out" >&2
  exit 1
fi
JCODE_HOME_DIR="$(printf '%s\n' "$status_out" | sed -n '1s/^cruise jcode home: //p')"
if [ -z "$JCODE_HOME_DIR" ]; then
  echo "::error::cruise: \`cruise login --status\` did not report a jcode home: $status_out" >&2
  exit 1
fi
echo "cruise: provisioning credentials in $JCODE_HOME_DIR"

# Every jcode invocation runs against that home with telemetry and the
# auto-update check off, matching how cruise itself drives jcode.
# run_jcode places --no-update first, the same position cruise's own
# jcode_command uses (src/backend/jcode.rs), instead of relying on jcode's
# parser accepting a global flag after the subcommand arguments.
run_jcode() {
  JCODE_HOME="$JCODE_HOME_DIR" JCODE_NO_TELEMETRY=1 jcode --no-update "$@"
}

# Same implementation as install.sh's version_at_least -- these step scripts
# are standalone by design, with no shared shell library between them.
version_at_least() { # $1=have $2=minimum
  local have="${1%%-*}" want="$2"
  local h_major h_minor h_patch w_major w_minor w_patch
  IFS=. read -r h_major h_minor h_patch <<EOF
$have
EOF
  IFS=. read -r w_major w_minor w_patch <<EOF
$want
EOF
  h_major="${h_major:-0}"; h_minor="${h_minor:-0}"; h_patch="${h_patch:-0}"
  case "$h_major$h_minor$h_patch" in
    *[!0-9]*|"") return 1 ;;
  esac
  if [ "$h_major" -ne "$w_major" ]; then [ "$h_major" -gt "$w_major" ]; return; fi
  if [ "$h_minor" -ne "$w_minor" ]; then [ "$h_minor" -gt "$w_minor" ]; return; fi
  [ "$h_patch" -ge "$w_patch" ]
}

jcode_version_json="$(run_jcode version --json)"
# cruise itself reads the bare `semver` field for its floor check; the
# decorated `version` string ("v0.81.2 (<hash)") is for humans.
jcode_semver="$(printf '%s' "$jcode_version_json" | jq -r '.semver // empty')"
if [ -z "$jcode_semver" ]; then
  echo "::error::cruise: \`jcode version --json\` reported no 'semver' field: $jcode_version_json" >&2
  exit 1
fi
echo "cruise: jcode $jcode_semver"
# The floor cruise enforces at run time (MIN_JCODE_VERSION in
# src/backend/jcode.rs). Check it now: install-jcode.sh skips the install
# entirely when a jcode is already on PATH, so a self-hosted runner's older
# binary would otherwise fail later as a raw jcode error from
# `provider add`.
MIN_JCODE_VERSION="0.81.1"
if ! version_at_least "$jcode_semver" "$MIN_JCODE_VERSION"; then
  echo "::error::cruise: jcode $jcode_semver is too old for this version of the action (requires jcode v$MIN_JCODE_VERSION or newer, the version cruise's \`sdk: jcode\` backend is verified against) -- pin a newer 'jcode_version', or update the jcode on this runner" >&2
  exit 1
fi

# --- dedicated provider keys ----------------------------------------------
# Masked defensively even though values sourced from `secrets.*` in the
# calling workflow are already auto-masked by the runner.
store_dedicated_key() { # $1=jcode provider id $2=key
  local login_out
  echo "::add-mask::$2"
  if ! login_out="$(CRUISE_LOGIN_API_KEY="$2" cruise login --api-key "$1" 2>&1)"; then
    echo "::error::cruise: \`cruise login --api-key $1\` failed: $login_out" >&2
    exit 1
  fi
  echo "cruise: stored the $1 API key in cruise's jcode home"
}

# --- provider profiles ----------------------------------------------------
# Structural validation only: profile-name shape (a name starting with "-"
# would reach `jcode provider add` as a flag rather than an argument), the
# field allowlist, field types, and the single-default rule. Every *value*
# domain -- base_url scheme, empty model, auth style, context window, the
# auth_header/auth pairing -- is left to `jcode provider add`, which rejects
# each with its own precise message.
validate_providers() {
  local validation_error
  validation_error="$(printf '%s' "$PROVIDERS_INPUT" | jq -r --argjson allowed "$PROVIDER_KEYS" '
    def q: "\u0027";
    def bad_type($n; $field; $want): "provider " + q + $n + q + ": " + $field + " must be " + $want;
    [ to_entries[]
      | .key as $n
      | .value as $v
      | if ($n | test("^[A-Za-z0-9][A-Za-z0-9_-]*$") | not) or ($n | length) > 64 then
          "invalid provider profile name " + q + $n + q + " (ASCII letters, numbers, " + q + "-" + q
            + " and " + q + "_" + q + " only, starting with a letter or number, at most 64 characters)"
        elif ($v | type) != "object" then
          "provider " + q + $n + q + " must be a JSON object"
        elif ((([$v | keys[]] - $allowed)) | length) > 0 then
          "provider " + q + $n + q + " has unknown key " + q + (([$v | keys[]] - $allowed)[0]) + q
        elif ($v.base_url | type) != "string" then
          bad_type($n; "base_url"; "a string")
        elif ($v.model | type) != "string" then
          bad_type($n; "model"; "a string")
        elif ($v | has("context_window")) and ($v.context_window | type) != "number" then
          bad_type($n; "context_window"; "a number")
        elif ($v | has("auth")) and ((["bearer", "api-key"] | index($v.auth)) == null) then
          bad_type($n; "auth"; q + "bearer" + q + " or " + q + "api-key" + q)
        elif ($v | has("auth_header")) and ($v.auth_header | type) != "string" then
          bad_type($n; "auth_header"; "a string")
        elif ($v | has("provider_routing")) and ($v.provider_routing | type) != "boolean" then
          bad_type($n; "provider_routing"; "a boolean")
        elif ($v | has("no_auth")) and ($v.no_auth | type) != "boolean" then
          bad_type($n; "no_auth"; "a boolean")
        elif ($v | has("default")) and ($v.default | type) != "boolean" then
          bad_type($n; "default"; "a boolean")
        else empty
        end
    ]
    + (if ([to_entries[] | select((.value | type) == "object" and .value.default == true)] | length) > 1
       then ["at most one " + q + "providers" + q + " entry may set " + q + "default: true" + q] else [] end)
    | .[0] // empty
  ')"
  if [ -n "$validation_error" ]; then
    provider_config_error "$validation_error"
  fi
}

# One `jcode provider add` flag per line, so a value containing spaces
# survives the read loop that turns them back into an argument list.
profile_flags() { # $1=profile name
  printf '%s' "$PROVIDERS_INPUT" | jq -r --arg name "$1" '
    .[$name] as $v
    | ["--base-url", $v.base_url, "--model", $v.model]
    + (if $v | has("context_window") then ["--context-window", ($v.context_window | tostring)] else [] end)
    + (if $v | has("auth") then ["--auth", $v.auth] else [] end)
    + (if $v | has("auth_header") then ["--auth-header", $v.auth_header] else [] end)
    + (if $v.provider_routing == true then ["--provider-routing"] else [] end)
    + (if $v.default == true then ["--set-default"] else [] end)
    + (if $v.no_auth == true then ["--no-api-key"] else ["--api-key-stdin"] end)
    | .[]
  '
}

configure_provider_profiles() {
  local provider_keyed_ids_file provider_noauth_ids_file
  local provider_api_keys_ids_file provider_api_keys_normalized
  local line line_no provider_id provider_key add_out add_status
  local args

  if [ -z "$PROVIDERS_INPUT" ] && [ -z "$PROVIDER_API_KEYS_INPUT" ]; then
    return 0
  fi
  if [ -z "$PROVIDERS_INPUT" ]; then
    # provider_api_keys with no profile to attach a key to is always a
    # mistake -- there is no no_auth-style relaxation that makes sense here.
    provider_config_error "'providers' and 'provider_api_keys' must be set together"
  fi
  if ! printf '%s' "$PROVIDERS_INPUT" | jq -e 'type == "object" and length > 0' >/dev/null 2>&1; then
    provider_config_error "'providers' must be a non-empty JSON object"
  fi
  validate_providers

  # Keyed (credential-taking) profiles vs `no_auth` ones: only the former
  # need a `provider_api_keys` line.
  provider_keyed_ids_file="$(new_tmp)"
  provider_noauth_ids_file="$(new_tmp)"
  printf '%s' "$PROVIDERS_INPUT" | jq -r '
    to_entries[] | select(.value.no_auth != true) | .key
  ' | sort > "$provider_keyed_ids_file"
  printf '%s' "$PROVIDERS_INPUT" | jq -r '
    to_entries[] | select(.value.no_auth == true) | .key
  ' | sort > "$provider_noauth_ids_file"

  if [ -z "$PROVIDER_API_KEYS_INPUT" ] && [ -s "$provider_keyed_ids_file" ]; then
    # provider_api_keys may only be omitted when EVERY profile declared
    # no_auth: true; any keyed profile still needs a credential source.
    provider_config_error "'providers' and 'provider_api_keys' must be set together"
  fi

  provider_api_keys_ids_file="$(new_tmp)"
  # The keys themselves never touch a file: the parse below normalizes each
  # line into an in-memory `id<TAB>key` list that the per-profile lookup
  # re-reads, so no mktemp'd file ever holds a credential.
  provider_api_keys_normalized=""
  if [ -n "$PROVIDER_API_KEYS_INPUT" ]; then
    line_no=0
    while IFS= read -r line || [ -n "$line" ]; do
      line_no=$((line_no + 1))
      case "$line" in \#*) continue ;; esac
      if [ -z "$(printf '%s' "$line" | tr -d '[:space:]')" ]; then
        continue
      fi
      if [ "${line#*=}" = "$line" ]; then
        provider_config_error "invalid 'provider_api_keys' entry on line $line_no (expected profile-name=API key)"
      fi
      provider_id="${line%%=*}"
      provider_key="${line#*=}"
      provider_key="${provider_key%$'\r'}"
      provider_key="$(printf '%s' "$provider_key" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
      provider_id="$(printf '%s' "$provider_id" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
      provider_id="${provider_id%$'\r'}"
      if [[ ! "$provider_id" =~ ^[A-Za-z0-9][A-Za-z0-9_-]*$ ]]; then
        provider_config_error "invalid provider profile name '$provider_id' in 'provider_api_keys'"
      fi
      if [ -z "$provider_key" ]; then
        provider_config_error "provider '$provider_id' has an empty API key in 'provider_api_keys'"
      fi
      if grep -Fqx "$provider_id" "$provider_api_keys_ids_file"; then
        provider_config_error "duplicate provider profile name '$provider_id' in 'provider_api_keys'"
      fi
      printf '%s\n' "$provider_id" >> "$provider_api_keys_ids_file"
      provider_api_keys_normalized+="$provider_id"$'\t'"$provider_key"$'\n'
    done <<< "$PROVIDER_API_KEYS_INPUT"

    # Every keyed (non-no_auth) profile must have a line above...
    while IFS= read -r provider_id; do
      [ -z "$provider_id" ] && continue
      if ! grep -Fqx "$provider_id" "$provider_api_keys_ids_file"; then
        provider_config_error "missing provider profile name '$provider_id' in 'provider_api_keys'"
      fi
    done < "$provider_keyed_ids_file"
    # ...and every line above must resolve back to a real, keyed profile: a
    # name absent from `providers` entirely is a typo, and a `no_auth` name
    # showing up here is a contradiction -- the whole point of no_auth is
    # that jcode is never handed this credential.
    while IFS= read -r provider_id; do
      [ -z "$provider_id" ] && continue
      if grep -Fqx "$provider_id" "$provider_noauth_ids_file"; then
        provider_config_error "provider '$provider_id' is 'no_auth' and must not appear in 'provider_api_keys'"
      fi
      if ! grep -Fqx "$provider_id" "$provider_keyed_ids_file"; then
        provider_config_error "unknown provider profile name '$provider_id' in 'provider_api_keys'"
      fi
    done < "$provider_api_keys_ids_file"
  fi

  # Profiles are added in sorted order so the log -- and any failure -- is
  # reproducible regardless of the input object's key order.
  while IFS= read -r provider_id; do
    [ -z "$provider_id" ] && continue
    args=()
    while IFS= read -r flag; do
      args+=("$flag")
    done < <(profile_flags "$provider_id")

    add_status=0
    if grep -Fqx "$provider_id" "$provider_noauth_ids_file"; then
      add_out="$(run_jcode provider add "$provider_id" "${args[@]}" --json </dev/null 2>&1)" || add_status=$?
    else
      provider_key="$(printf '%s' "$provider_api_keys_normalized" | awk -F '\t' -v id="$provider_id" '$1 == id { sub(/^[^\t]*\t/, ""); print; exit }')"
      echo "::add-mask::$provider_key"
      add_out="$(printf '%s\n' "$provider_key" | run_jcode provider add "$provider_id" "${args[@]}" --json 2>&1)" || add_status=$?
    fi
    if [ "$add_status" -ne 0 ]; then
      echo "::error::cruise: \`jcode provider add $provider_id\` failed: $add_out" >&2
      exit 1
    fi
    printf '%s' "$add_out" | jq -r '
      "cruise: added jcode provider profile \u0027" + .profile + "\u0027 to " + .config_path
      + " (key env: " + (.api_key_env // "none") + ", default: " + (.default_set | tostring) + ")"
    '
  done < <(printf '%s' "$PROVIDERS_INPUT" | jq -r 'keys_unsorted[]' | sort)
}

if [ -n "$ANTHROPIC_API_KEY_INPUT" ]; then
  store_dedicated_key anthropic-api "$ANTHROPIC_API_KEY_INPUT"
fi
if [ -n "$OPENAI_API_KEY_INPUT" ]; then
  store_dedicated_key openai-api "$OPENAI_API_KEY_INPUT"
fi
configure_provider_profiles

# What the run will actually see: the provider list cruise resolves from the
# home this step just populated.
# Observational only: with pipefail, a failing status print would otherwise
# fail the step after the credentials were already written, and a retried
# job would re-run every `jcode provider add` above.
cruise login --status 2>&1 | sed -n '2,$p' || true
