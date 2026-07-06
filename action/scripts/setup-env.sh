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
if [ -n "$PI_MODELS_JSON" ]; then
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
