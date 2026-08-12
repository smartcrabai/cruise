#!/usr/bin/env bash
# Centralizes every environment variable the cruise CLI needs for this run:
# provider API keys, the forced `sdk: pi` backend, model overrides, pi's
# optional custom models.json, and user-supplied extra env vars.
#
# Everything is exported via $GITHUB_ENV conditionally (skipping empty
# values) rather than declared as static `env:` entries in action.yml,
# because several of these variables are NOT safe to set to an empty string:
#   - CRUISE_CONFIG (see resolve-config.sh) and PI_CODING_AGENT_DIR are read
#     via a plain env-var lookup by cruise/pi that treats "set but empty"
#     differently from "unset" (an empty value is treated as a real,
#     nonexistent path rather than "fall back to the default").
#   - Setting ANTHROPIC_API_KEY/OPENAI_API_KEY to "" when the corresponding
#     input was left blank could shadow a credential pi would otherwise
#     resolve from its own stored auth (~/.pi/agent/auth.json) or another
#     ambient env var.
set -euo pipefail

ANTHROPIC_API_KEY_INPUT="${ANTHROPIC_API_KEY_INPUT:-}"
OPENAI_API_KEY_INPUT="${OPENAI_API_KEY_INPUT:-}"
MODEL_INPUT="${MODEL_INPUT:-}"
PLAN_MODEL_INPUT="${PLAN_MODEL_INPUT:-}"
PI_MODELS_JSON="${PI_MODELS_JSON:-}"
ENV_INPUT="${ENV_INPUT:-}"
RUNNER_TEMP_DIR="${RUNNER_TEMP:-/tmp}"
PROVIDERS_INPUT="${PROVIDERS_INPUT:-}"
PROVIDER_API_KEYS_INPUT="${PROVIDER_API_KEYS_INPUT:-}"

provider_config_error() {
  echo "::error::cruise: invalid provider configuration: $1" >&2
  exit 1
}

provider_keys_file=""
if [ -n "$PROVIDERS_INPUT" ] || [ -n "$PROVIDER_API_KEYS_INPUT" ]; then
  if [ -z "$PROVIDERS_INPUT" ] || [ -z "$PROVIDER_API_KEYS_INPUT" ]; then
    echo "::error::cruise: 'providers' and 'provider_api_keys' must be set together" >&2
    exit 1
  fi
  if [ -n "$PI_MODELS_JSON" ]; then
    echo "::error::cruise: 'providers' cannot be combined with 'pi_models_json'" >&2
    exit 1
  fi
  if ! printf '%s' "$PROVIDERS_INPUT" | jq -e 'type == "object" and length > 0' >/dev/null 2>&1; then
    provider_config_error "'providers' must be a non-empty JSON object"
  fi
  provider_keys_file="$(mktemp)"
  trap 'rm -f "$provider_keys_file" "$provider_keys_file.providers" "$provider_keys_file.values" "$provider_keys_file.generated" "$provider_keys_file.generated.tmp"' EXIT
  while IFS= read -r provider_id; do
    if [[ ! "$provider_id" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
      provider_config_error "invalid provider id '$provider_id' in 'providers'"
    fi
    if ! printf '%s' "$PROVIDERS_INPUT" | jq -e --arg id "$provider_id" '.[$id] | type == "object" and (keys | sort == ["api", "base_url", "models"])' >/dev/null 2>&1; then
      provider_config_error "provider '$provider_id' must contain exactly 'api', 'base_url', and 'models'"
    fi
    api="$(printf '%s' "$PROVIDERS_INPUT" | jq -r --arg id "$provider_id" '.[$id].api')"
    case "$api" in
      openai-completions|openai-responses|anthropic-messages) ;;
      *) provider_config_error "provider '$provider_id' has unsupported api '$api'" ;;
    esac
    if ! printf '%s' "$PROVIDERS_INPUT" | jq -e --arg id "$provider_id" '.[$id].base_url | type == "string" and test("^https?://[^[:space:]]+\\z")' >/dev/null 2>&1; then
      provider_config_error "provider '$provider_id' has invalid base_url (expected non-empty http:// or https:// URL)"
    fi
    if ! printf '%s' "$PROVIDERS_INPUT" | jq -e --arg id "$provider_id" '.[$id].models | type == "array" and length > 0 and all(.[]; type == "string" and test("^[^[:space:]]+\\z")) and (length == (unique | length))' >/dev/null 2>&1; then
      provider_config_error "provider '$provider_id' models must be a non-empty array of unique, non-empty strings"
    fi
    printf '%s\n' "$provider_id" >> "$provider_keys_file.providers"
  done < <(printf '%s' "$PROVIDERS_INPUT" | jq -r 'keys[]')
  line_no=0
  while IFS= read -r line || [ -n "$line" ]; do
    line_no=$((line_no + 1))
    case "$line" in \#*) continue ;; esac
    if [ -z "$(printf '%s' "$line" | tr -d '[:space:]')" ]; then
      continue
    fi
    if [ "${line#*=}" = "$line" ]; then
      provider_config_error "invalid 'provider_api_keys' entry on line $line_no (expected provider-id=API key)"
    fi
    provider_id="${line%%=*}"
    provider_key="${line#*=}"
    provider_key="${provider_key%$'\r'}"
    provider_key="$(printf '%s' "$provider_key" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
    provider_id="$(printf '%s' "$provider_id" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
    provider_id="${provider_id%$'\r'}"
    if [[ ! "$provider_id" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
      provider_config_error "invalid provider id '$provider_id' in 'provider_api_keys'"
    fi
    if [ -z "$provider_key" ]; then
      provider_config_error "provider '$provider_id' has an empty API key in 'provider_api_keys'"
    fi
    if grep -Fqx "$provider_id" "$provider_keys_file"; then
      provider_config_error "duplicate provider id '$provider_id' in 'provider_api_keys'"
    fi
    printf '%s\n' "$provider_id" >> "$provider_keys_file"
    printf '%s\t%s\n' "$provider_id" "$provider_key" >> "$provider_keys_file.values"
  done <<< "$PROVIDER_API_KEYS_INPUT"
  while IFS= read -r provider_id; do
    if ! grep -Fqx "$provider_id" "$provider_keys_file"; then
      provider_config_error "missing provider id '$provider_id' in 'provider_api_keys'"
    fi
  done < "$provider_keys_file.providers"
  while IFS= read -r provider_id; do
    if ! grep -Fqx "$provider_id" "$provider_keys_file.providers"; then
      provider_config_error "unknown provider id '$provider_id' in 'provider_api_keys'"
    fi
  done < "$provider_keys_file"
fi

export_env() { # $1=name $2=value
  echo "$1=$2" >> "$GITHUB_ENV"
}

# --- sdk: pi is always forced, regardless of what any config declares. ---
export_env CRUISE_SDK pi

# --- provider API keys (at least one is guaranteed non-empty; gate.sh
# hard-fails otherwise). Masked defensively even though values sourced from
# `secrets.*` in the calling workflow are already auto-masked by the runner. ---
if [ -n "$ANTHROPIC_API_KEY_INPUT" ]; then
  echo "::add-mask::$ANTHROPIC_API_KEY_INPUT"
  export_env ANTHROPIC_API_KEY "$ANTHROPIC_API_KEY_INPUT"
fi
if [ -n "$OPENAI_API_KEY_INPUT" ]; then
  echo "::add-mask::$OPENAI_API_KEY_INPUT"
  export_env OPENAI_API_KEY "$OPENAI_API_KEY_INPUT"
fi

# --- model references: pi format ("provider/model[:thinking]" or a bare
# model id); empty means "let pi auto-select". cruise's own env-override
# reader already ignores an empty CRUISE_MODEL/CRUISE_PLAN_MODEL, but we
# still skip the export entirely for clarity. ---
[ -n "$MODEL_INPUT" ] && export_env CRUISE_MODEL "$MODEL_INPUT"
[ -n "$PLAN_MODEL_INPUT" ] && export_env CRUISE_PLAN_MODEL "$PLAN_MODEL_INPUT"

# --- optional custom pi models.json (OpenAI-compatible endpoints, custom
# providers, model registry overrides -- see docs/github-actions.md). ---
if [ -n "$PROVIDERS_INPUT" ]; then
  agent_dir="$RUNNER_TEMP_DIR/pi-agent"
  mkdir -p "$agent_dir"
  generated="$provider_keys_file.generated"
  printf '%s\n' '{"providers":{}}' > "$generated"
  provider_index=0
  while IFS= read -r provider_id; do
    provider_key="$(awk -F '\t' -v id="$provider_id" '$1 == id { sub(/^[^\t]*\t/, ""); print; exit }' "$provider_keys_file.values")"
    echo "::add-mask::$provider_key"
    export_env "CRUISE_PROVIDER_API_KEY_$provider_index" "$provider_key"
    api="$(printf '%s' "$PROVIDERS_INPUT" | jq -r --arg id "$provider_id" '.[$id].api')"
    base_url="$(printf '%s' "$PROVIDERS_INPUT" | jq -r --arg id "$provider_id" '.[$id].base_url')"
    models="$(printf '%s' "$PROVIDERS_INPUT" | jq -c --arg id "$provider_id" '.[$id].models | map({id: .})')"
    jq --arg id "$provider_id" --arg api "$api" --arg base_url "$base_url" --arg keyref "env:CRUISE_PROVIDER_API_KEY_$provider_index" --argjson models "$models" '.providers[$id] = {api: $api, baseUrl: $base_url, apiKey: $keyref, models: $models}' "$generated" > "$generated.tmp"
    mv "$generated.tmp" "$generated"
    provider_index=$((provider_index + 1))
  done < <(sort "$provider_keys_file.providers")
  mv "$generated" "$agent_dir/models.json"
  export_env PI_CODING_AGENT_DIR "$agent_dir"
  echo "cruise: wrote providers to $agent_dir/models.json (PI_CODING_AGENT_DIR set)"
elif [ -n "$PI_MODELS_JSON" ]; then
  if ! printf '%s' "$PI_MODELS_JSON" | jq empty >/dev/null 2>&1; then
    echo "::error::cruise: 'pi_models_json' is not valid JSON" >&2
    exit 1
  fi
  agent_dir="$RUNNER_TEMP_DIR/pi-agent"
  mkdir -p "$agent_dir"
  printf '%s' "$PI_MODELS_JSON" > "$agent_dir/models.json"
  export_env PI_CODING_AGENT_DIR "$agent_dir"
  echo "cruise: wrote pi_models_json to $agent_dir/models.json (PI_CODING_AGENT_DIR set)"
fi

# --- user-supplied extra env vars ("KEY=VALUE" per line, blank lines and
# "#"-prefixed lines ignored). Reserved names are skipped (with a warning)
# instead of silently letting a workflow author override token/auth/path
# plumbing this action depends on. ---
RESERVED_KEYS="GITHUB_TOKEN GH_TOKEN PI_CODING_AGENT_DIR PATH HOME SHELL GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL XDG_DATA_HOME XDG_CONFIG_HOME XDG_STATE_HOME"

# Prints a non-empty reason if $1 is reserved (and should be skipped), empty
# otherwise. CRUISE_* gets its own message pointing at the dedicated inputs
# (model/plan_model/config) instead of the generic "managed by the action"
# wording, since users reaching for e.g. `env: CRUISE_MODEL=...` almost
# always want the `model` input instead.
reserved_reason() { # $1=key
  case "$1" in
    CRUISE_*)
      echo "reserved -- override cruise settings via this action's dedicated inputs (model/plan_model/config) instead of a raw CRUISE_* env var"
      return
      ;;
    GITHUB_* | ACTIONS_* | RUNNER_*)
      echo "reserved (managed by the GitHub Actions runner)"
      return
      ;;
  esac
  local k
  for k in $RESERVED_KEYS; do
    if [ "$1" = "$k" ]; then
      echo "reserved (managed by the action itself)"
      return
    fi
  done
}

if [ -n "$ENV_INPUT" ]; then
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      \#*) continue ;;
    esac
    if [ -z "$(printf '%s' "$line" | tr -d '[:space:]')" ]; then
      continue
    fi
    if [ "${line#*=}" = "$line" ]; then
      echo "::warning::cruise: ignoring malformed 'env' entry (expected KEY=VALUE): $line"
      continue
    fi
    key="${line%%=*}"
    value="${line#*=}"
    # Strip a trailing CR (e.g. the `env` input was pasted/generated with
    # CRLF line endings) from both the key and the value.
    key="${key%$'\r'}"
    value="${value%$'\r'}"
    key="$(printf '%s' "$key" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
    if ! printf '%s' "$key" | grep -qE '^[A-Za-z_][A-Za-z0-9_]*$'; then
      echo "::warning::cruise: ignoring 'env' entry with an invalid variable name: '$key'"
      continue
    fi
    reason="$(reserved_reason "$key")"
    if [ -n "$reason" ]; then
      echo "::warning::cruise: ignoring 'env' entry for '$key' ($reason)"
      continue
    fi
    if [ -n "$value" ]; then
      echo "::add-mask::$value"
    fi
    export_env "$key" "$value"
  done <<< "$ENV_INPUT"
fi
