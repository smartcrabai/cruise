#!/usr/bin/env bash
# Resolves the cruise workflow config(s) to use for this run.
#
# When the user sets the `config` input, that file is used verbatim for both
# issue mode and PR mode (the caller is responsible for making it work with
# both -- see docs/github-actions.md).
#
# Otherwise, two auto-generated configs are produced:
#   - config_path (issue mode): mirrors cruise's own built-in default
#     workflow (WorkflowConfig::default_builtin() -- command: [claude,
#     --model, "{model}", -p], steps: write-tests -> implement) with
#     `claude_args` appended to the command. The step prompts are read
#     verbatim from prompts/*.md in this repo (see embed_prompt_file below)
#     so they can't drift from cruise's own built-in prompts.
#   - pr_config_path (PR mode): a single `implement` step whose prompt is
#     just `{input}`. PR mode (`cruise exec`) binds the whole task.md built
#     by build-prompt.sh to `{input}` and leaves `{plan}` empty (there is no
#     planning step), so a config built around `{plan}` would silently send
#     the model an empty prompt -- see HIGH-2 in the review this fixed.
set -euo pipefail

CONFIG_INPUT="${CONFIG_INPUT:-}"
WORKSPACE="${GITHUB_WORKSPACE:-$(pwd)}"
OUT_DIR="${RUNNER_TEMP:-/tmp}/cruise"
mkdir -p "$OUT_DIR"

# The action's own checkout (where prompts/*.md live), so the generated
# config can embed the real prompt files instead of a hand-copied duplicate.
# GITHUB_ACTION_PATH is set automatically by GitHub Actions for composite
# actions; fall back to resolving relative to this script for local testing.
resolve_action_root() {
  if [ -n "${GITHUB_ACTION_PATH:-}" ]; then
    printf '%s\n' "$GITHUB_ACTION_PATH"
  else
    (cd "$(dirname "$0")/../.." && pwd)
  fi
}
ACTION_ROOT="$(resolve_action_root)"

# Embeds a prompt file as a YAML block-literal ("|") body indented by $2.
# Reads the file verbatim (no escaping needed inside a block literal) so
# blank lines and any YAML-special characters in the prompt survive
# unchanged; blank lines are emitted with no indentation, which YAML block
# literals accept unconditionally.
embed_prompt_file() { # $1=file $2=indent
  local file="$1" indent="$2"
  if [ ! -f "$file" ]; then
    echo "::error::cruise: prompt file not found at '$file' (GITHUB_ACTION_PATH=${GITHUB_ACTION_PATH:-unset}, ACTION_ROOT=$ACTION_ROOT)" >&2
    exit 1
  fi
  while IFS='' read -r line || [ -n "$line" ]; do
    if [ -n "$line" ]; then
      printf '%s%s\n' "$indent" "$line"
    else
      printf '\n'
    fi
  done < "$file"
}

write_command_block() {
  echo "command:"
  echo "  - claude"
  echo "  - --model"
  echo "  - \"{model}\""
  echo "  - -p"
  # Intentional word-splitting: claude_args is a space-separated flag list,
  # not a single shell-quoted value.
  # shellcheck disable=SC2086
  set -- ${CLAUDE_ARGS:-}
  for arg in "$@"; do
    printf '  - %s\n' "$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$arg")"
  done
}

if [ -n "$CONFIG_INPUT" ]; then
  case "$CONFIG_INPUT" in
    /*) resolved="$CONFIG_INPUT" ;;
    *) resolved="$WORKSPACE/$CONFIG_INPUT" ;;
  esac
  if [ ! -f "$resolved" ]; then
    echo "::error::cruise config not found at '$resolved' (input 'config' = '$CONFIG_INPUT')" >&2
    exit 1
  fi
  echo "cruise: using user-supplied config at $resolved (for both issue and PR mode)"
  pr_resolved="$resolved"
else
  resolved="$OUT_DIR/ci-default.yaml"
  pr_resolved="$OUT_DIR/ci-default-pr.yaml"
  echo "cruise: generating default CI config at $resolved"
  echo "cruise: generating default PR-mode CI config at $pr_resolved"

  {
    write_command_block
    cat <<'YAML'

model: sonnet
plan_model: opus

steps:
  write-tests:
    prompt: |
YAML
    embed_prompt_file "$ACTION_ROOT/prompts/write-test-first.md" "      "
    cat <<'YAML'

  implement:
    prompt: |
YAML
    embed_prompt_file "$ACTION_ROOT/prompts/implement-after-tests.md" "      "
  } > "$resolved"

  {
    write_command_block
    cat <<'YAML'

model: sonnet

steps:
  implement:
    prompt: "{input}"
YAML
  } > "$pr_resolved"
fi

{
  echo "config_path=$resolved"
  echo "pr_config_path=$pr_resolved"
} >> "$GITHUB_OUTPUT"
