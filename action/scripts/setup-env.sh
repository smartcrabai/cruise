#!/usr/bin/env bash
# Centralizes every environment variable the cruise CLI needs for this run:
# provider API keys, the forced `sdk: pi` backend, model overrides, pi's
# optional custom models.json (generated from `providers`, a raw
# `pi_models_json` overlay, or both merged together), and user-supplied
# extra env vars.
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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROVIDERS_JQ="$SCRIPT_DIR/lib/providers-to-models-json.jq"

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

# Every mktemp'd scratch file gets pushed here so a single EXIT trap covers
# whatever this run actually created, however far it got before erroring.
TMP_FILES=()
trap 'rm -f "${TMP_FILES[@]}"' EXIT
new_tmp() {
  local f
  f="$(mktemp)"
  TMP_FILES+=("$f")
  printf '%s' "$f"
}

# Snapshot of pi's PROVIDER_METADATA table (provider_metadata.rs:61-1657,
# pi_agent_rust 0.1.23): 95 built-in providers, 146 ids counting aliases,
# matched case-insensitively after trimming (provider_metadata.rs:1659-1672).
# A `providers` id that collides with one of these isn't rejected --
# overriding a built-in provider is a legitimate advanced use -- but it
# silently changes two things pi does with that entry (see
# providers-to-models-json.jq's reserved_warning), so this only ever
# produces a warning. Being stale only costs a missed *advisory* warning,
# never a wrong models.json, so it doesn't need to track pi release-for-release.
RESERVED_PROVIDER_IDS='["302ai","abacus","aihubmix","alibaba","alibaba-cn","alibaba-us","amazon-bedrock","anthropic","antigravity","atlas","atlas-cloud","atlascloud","azure","azure-cognitive-services","azure-openai","azure-openai-responses","azure_openai","bailing","baseten","bedrock","berget","cerebras","chatgpt-codex","chutes","cloudflare-ai-gateway","cloudflare-workers-ai","codex","cohere","copilot","cortecs","cursor","cursor-agent","dashscope","deep-infra","deep-seek","deepinfra","deepseek","fastrouter","fireworks","fireworks-ai","firmware","friendli","gemini","gemini-cli","github-copilot","github-copilot-enterprise","github-models","gitlab","gitlab-duo","glm","google","google-antigravity","google-gemini-cli","google-vertex","google-vertex-anthropic","grok","groq","helicone","hf","hugging-face","huggingface","iflowcn","inception","inference","io-net","jiekou","kimi","kimi-code","kimi-coding","kimi-for-coding","llama","llama-cpp","llama-server","llama.cpp","llamacpp","lm-studio","lmstudio","lucidquery","minimax","minimax-cn","minimax-cn-coding-plan","minimax-coding-plan","mistral","mistral-rs","mistral.rs","mistralai","mistralrs","moark","modelscope","moonshot","moonshotai","moonshotai-cn","morph","nano-gpt","nanogpt","nebius","nim","nova","novita","novita-ai","nvidia","nvidia-nim","ollama","ollama-cloud","open-router","openai","openai-codex","opencode","openrouter","ovhcloud","perplexity","poe","pplx","privatemode-ai","qwen","requesty","sap","sap-ai-core","scaleway","silicon-flow","siliconflow","siliconflow-cn","stackit","submodel","synthetic","together","together-ai","togetherai","upstage","v0","venice","vercel","vercel-ai-gateway","vertexai","vivgrid","vultr","wandb","x-ai","xai","xiaomi","zai","zai-coding-plan","zenmux","zhipu","zhipuai","zhipuai-coding-plan"]'

# Populated below, only while `providers` is set. Kept at "" (not merely
# unset) so later `[ -n ... ]`/`[ -s ... ]` checks under `set -u` never trip
# on an unbound variable even when the whole block is skipped.
provider_keyed_ids_file=""
provider_noauth_ids_file=""
provider_api_keys_ids_file=""
provider_api_keys_values_file=""

if [ -n "$PROVIDERS_INPUT" ] || [ -n "$PROVIDER_API_KEYS_INPUT" ]; then
  if [ -z "$PROVIDERS_INPUT" ]; then
    # provider_api_keys with no providers to attach a key to is always a
    # mistake -- there's no no_auth-style relaxation that makes sense here.
    provider_config_error "'providers' and 'provider_api_keys' must be set together"
  fi
  if ! printf '%s' "$PROVIDERS_INPUT" | jq -e 'type == "object" and length > 0' >/dev/null 2>&1; then
    provider_config_error "'providers' must be a non-empty JSON object"
  fi

  # Full per-provider schema validation (id shape, api/base_url/models,
  # headers, compat, cost, no_auth, ...) now lives entirely in
  # providers-to-models-json.jq (invoked further down, once we know the
  # apiKey env-var references). All this loop needs up front is the
  # keyed/no_auth split, to decide whether provider_api_keys must cover a
  # given id -- so it stays deliberately loose about everything else
  # (a malformed provider entry, e.g. non-object, just falls into "keyed"
  # and gets a precise error from the .jq validator later).
  provider_keyed_ids_file="$(new_tmp)"
  provider_noauth_ids_file="$(new_tmp)"
  printf '%s' "$PROVIDERS_INPUT" | jq -r '
    to_entries[] | select((((.value|type) != "object")) or (.value.no_auth != true)) | .key
  ' | sort > "$provider_keyed_ids_file"
  printf '%s' "$PROVIDERS_INPUT" | jq -r '
    to_entries[] | select(((.value|type) == "object") and (.value.no_auth == true)) | .key
  ' | sort > "$provider_noauth_ids_file"

  if [ -z "$PROVIDER_API_KEYS_INPUT" ] && [ -s "$provider_keyed_ids_file" ]; then
    # provider_api_keys may only be omitted when EVERY provider declared
    # no_auth: true; any keyed provider still needs a credential source.
    provider_config_error "'providers' and 'provider_api_keys' must be set together"
  fi

  if [ -n "$PROVIDER_API_KEYS_INPUT" ]; then
    provider_api_keys_ids_file="$(new_tmp)"
    provider_api_keys_values_file="$(new_tmp)"
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
      if grep -Fqx "$provider_id" "$provider_api_keys_ids_file"; then
        provider_config_error "duplicate provider id '$provider_id' in 'provider_api_keys'"
      fi
      printf '%s\n' "$provider_id" >> "$provider_api_keys_ids_file"
      printf '%s\t%s\n' "$provider_id" "$provider_key" >> "$provider_api_keys_values_file"
    done <<< "$PROVIDER_API_KEYS_INPUT"

    # Every keyed (non-no_auth) provider must have a line above...
    while IFS= read -r provider_id; do
      [ -z "$provider_id" ] && continue
      if ! grep -Fqx "$provider_id" "$provider_api_keys_ids_file"; then
        provider_config_error "missing provider id '$provider_id' in 'provider_api_keys'"
      fi
    done < "$provider_keyed_ids_file"
    # ...and every line above must resolve back to a real, keyed provider:
    # an id absent from `providers` entirely is a typo, and a `no_auth`
    # id showing up here is a contradiction -- the whole point of no_auth
    # is that pi is never handed this credential.
    while IFS= read -r provider_id; do
      if grep -Fqx "$provider_id" "$provider_noauth_ids_file"; then
        provider_config_error "provider '$provider_id' is 'no_auth' and must not appear in 'provider_api_keys'"
      fi
      if ! grep -Fqx "$provider_id" "$provider_keyed_ids_file"; then
        provider_config_error "unknown provider id '$provider_id' in 'provider_api_keys'"
      fi
    done < "$provider_api_keys_ids_file"
  fi
fi

export_env() { # $1=name $2=value
  echo "$1=$2" >> "$GITHUB_ENV"
}

# --- sdk: pi is always forced, regardless of what any config declares. ---
export_env CRUISE_SDK pi
# --- force_exec is never honored here: action commands decide the mode. ---
export_env CRUISE_FORCE_EXEC false

# --- provider API keys (at least one credential source is guaranteed
# non-empty; gate.sh hard-fails otherwise). Masked defensively even though
# values sourced from `secrets.*` in the calling workflow are already
# auto-masked by the runner. ---
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

# --- pi models.json: generate from `providers` (if set), then deep-merge a
# `pi_models_json` overlay on top of it (if also set) -- it is the raw
# escape hatch, so it wins on any key it touches. jq's `*` is a recursive
# object merge with wholesale array replacement (verified with jq 1.7.1): an
# override of `models` replaces the list rather than appending duplicates.
# An explicit JSON `null` in the overlay doesn't delete a key at the JSON
# level, but pi reads every ProviderConfig field as Option<T>, so `null`
# still means "unset this" once pi parses it. `providers` and
# `pi_models_json` used to be mutually exclusive; they compose now. ---
if [ -n "$PROVIDERS_INPUT" ] || [ -n "$PI_MODELS_JSON" ]; then
  agent_dir="$RUNNER_TEMP_DIR/pi-agent"
  mkdir -p "$agent_dir"

  if [ -n "$PI_MODELS_JSON" ] && ! printf '%s' "$PI_MODELS_JSON" | jq -e 'type == "object"' >/dev/null 2>&1; then
    echo "::error::cruise: 'pi_models_json' is not valid JSON (must be a JSON object)" >&2
    exit 1
  fi

  if [ -n "$PROVIDERS_INPUT" ]; then
    # Assign CRUISE_PROVIDER_API_KEY_N indices over the sorted keyed
    # (non-no_auth) ids, exactly as before, then hand the id -> env-var-name
    # map to the .jq validator via --argjson so it can emit
    # "env:CRUISE_PROVIDER_API_KEY_N" apiKey references itself. A provider
    # id absent from this map is, by construction, a no_auth one.
    keyrefs_lines_file="$(new_tmp)"
    provider_index=0
    while IFS= read -r provider_id; do
      [ -z "$provider_id" ] && continue
      envvar="CRUISE_PROVIDER_API_KEY_$provider_index"
      provider_key="$(awk -F '\t' -v id="$provider_id" '$1 == id { sub(/^[^\t]*\t/, ""); print; exit }' "$provider_api_keys_values_file")"
      echo "::add-mask::$provider_key"
      export_env "$envvar" "$provider_key"
      printf '%s\t%s\n' "$provider_id" "$envvar" >> "$keyrefs_lines_file"
      provider_index=$((provider_index + 1))
    done < "$provider_keyed_ids_file"
    keyrefs="$(jq -R -n '[inputs | select(length > 0) | split("\t") | {(.[0]): .[1]}] | add // {}' "$keyrefs_lines_file")"

    validator_out="$(printf '%s' "$PROVIDERS_INPUT" | jq --argjson keyrefs "$keyrefs" --argjson reserved "$RESERVED_PROVIDER_IDS" -f "$PROVIDERS_JQ")"
    error_msg="$(printf '%s' "$validator_out" | jq -r '.error // empty')"
    if [ -n "$error_msg" ]; then
      provider_config_error "$error_msg"
    fi
    while IFS= read -r warning; do
      [ -z "$warning" ] && continue
      echo "::warning::cruise: $warning"
    done < <(printf '%s' "$validator_out" | jq -r '.warnings[]? // empty')
    generated_json="$(printf '%s' "$validator_out" | jq -c '.models_json')"

    if [ -n "$PI_MODELS_JSON" ]; then
      final_json="$(jq -c -n --argjson base "$generated_json" --argjson overlay "$PI_MODELS_JSON" '$base * $overlay')"
      log_suffix=", merged with pi_models_json on top"
    else
      final_json="$generated_json"
      log_suffix=""
    fi

    # ModelsConfig.providers (models.rs:211-215) is not an Option and has no
    # #[serde(default)], so pi's own parse fails outright with "missing
    # field providers" on anything else -- catch a pi_models_json overlay
    # that clobbered it here instead of deep inside the pi run.
    if ! printf '%s' "$final_json" | jq -e '.providers | type == "object"' >/dev/null 2>&1; then
      echo "::error::cruise: generated pi models.json has no 'providers' object (check whether 'pi_models_json' overwrote it)" >&2
      exit 1
    fi
    printf '%s' "$final_json" > "$agent_dir/models.json"
    export_env PI_CODING_AGENT_DIR "$agent_dir"
    echo "cruise: wrote providers to $agent_dir/models.json$log_suffix (PI_CODING_AGENT_DIR set)"
  else
    # pi_models_json alone: written verbatim (byte-for-byte) -- no jq
    # round-trip here, since that would reformat/reorder the user's exact
    # file content instead of passing it through as the documented raw
    # escape hatch.
    if ! printf '%s' "$PI_MODELS_JSON" | jq -e '.providers | type == "object"' >/dev/null 2>&1; then
      echo "::error::cruise: 'pi_models_json' has no 'providers' object (pi's ModelsConfig.providers is required, even as {})" >&2
      exit 1
    fi
    printf '%s' "$PI_MODELS_JSON" > "$agent_dir/models.json"
    export_env PI_CODING_AGENT_DIR "$agent_dir"
    echo "cruise: wrote pi_models_json to $agent_dir/models.json (PI_CODING_AGENT_DIR set)"
  fi
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
