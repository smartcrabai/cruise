#!/usr/bin/env bash
# Centralizes the environment variables the cruise CLI itself reads for this
# run: the model overrides and the user-supplied extra env vars.
#
# Model *credentials* are deliberately not here: they live in cruise's own
# jcode home, written by provision-jcode.sh once both binaries are installed.
#
# Everything is exported via $GITHUB_ENV conditionally (skipping empty
# values) rather than declared as static `env:` entries in action.yml,
# because several of these variables are NOT safe to set to an empty string:
# CRUISE_CONFIG (see resolve-config.sh) is read via a plain env-var lookup by
# cruise that treats "set but empty" differently from "unset" (an empty value
# is treated as a real, nonexistent path rather than "fall back to the
# default").
#
# No CRUISE_SDK is exported: cruise's own default -- `sdk: jcode` when neither
# `sdk` nor `command` is set -- is what this action relies on, so a workflow
# config that names a different backend keeps working instead of being
# silently overridden.
set -euo pipefail

MODEL_INPUT="${MODEL_INPUT:-}"
PLAN_MODEL_INPUT="${PLAN_MODEL_INPUT:-}"
ENV_INPUT="${ENV_INPUT:-}"

export_env() { # $1=name $2=value
  echo "$1=$2" >> "$GITHUB_ENV"
}

# --- cruise's XDG dirs, identical for EVERY later step. cruise's jcode home
# sits under XDG_DATA_HOME (src/paths.rs falls back to
# $HOME/.local/share/cruise when it is unset), so provision-jcode.sh writing
# credentials and the run step reading them must resolve the same path --
# and the RUNNER_TEMP base keeps those credentials off a self-hosted
# runner's real $HOME. $GITHUB_ENV written here, ahead of the install and
# provision steps, is the only channel that reaches all of them. ---
CRUISE_DIR="${RUNNER_TEMP:-/tmp}/cruise"
mkdir -p "$CRUISE_DIR/data" "$CRUISE_DIR/xdg-config" "$CRUISE_DIR/xdg-state"
export_env XDG_DATA_HOME "$CRUISE_DIR/data"
export_env XDG_CONFIG_HOME "$CRUISE_DIR/xdg-config"
export_env XDG_STATE_HOME "$CRUISE_DIR/xdg-state"

# --- force_exec is never honored here: action commands decide the mode. ---
export_env CRUISE_FORCE_EXEC false

# --- model references: cruise's jcode format ("provider/model[:effort]" or a
# bare model id); empty means "use the default provider/model configured in
# cruise's jcode home". cruise's own env-override reader already ignores an
# empty CRUISE_MODEL/CRUISE_PLAN_MODEL, but we still skip the export entirely
# for clarity. ---
[ -n "$MODEL_INPUT" ] && export_env CRUISE_MODEL "$MODEL_INPUT"
[ -n "$PLAN_MODEL_INPUT" ] && export_env CRUISE_PLAN_MODEL "$PLAN_MODEL_INPUT"

# --- user-supplied extra env vars ("KEY=VALUE" per line, blank lines and
# "#"-prefixed lines ignored). Reserved names are skipped (with a warning)
# instead of silently letting a workflow author override token/auth/path
# plumbing this action depends on. XDG_DATA_HOME is reserved because it is
# what places cruise's jcode home, which provision-jcode.sh has already
# populated by the time the run starts. ---
RESERVED_KEYS="GITHUB_TOKEN GH_TOKEN PATH HOME SHELL GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL XDG_DATA_HOME XDG_CONFIG_HOME XDG_STATE_HOME"

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
