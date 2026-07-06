#!/usr/bin/env bash
# Posts the initial "cruise is working..." tracking comment on the issue.
set -euo pipefail

REPO="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
ENTITY_NUMBER="${ENTITY_NUMBER:?ENTITY_NUMBER is required}"
COMMAND="${COMMAND:-run}"
TRIGGER_ACTOR="${TRIGGER_ACTOR:-someone}"

job_url="${GITHUB_SERVER_URL:-https://github.com}/${REPO}/actions/runs/${GITHUB_RUN_ID:-}"

case "$COMMAND" in
  run) verb="planning and opening a pull request" ;;
  exec) verb="executing directly on the default branch (no PR)" ;;
  plan) verb="drafting a plan" ;;
  fix) verb="revising the plan" ;;
  *) verb="working on this" ;;
esac

body="$(cat <<EOF
🧭 **cruise** is on it, @${TRIGGER_ACTOR} -- ${verb}... [View run](${job_url})
EOF
)"

comment_id="$(gh api "repos/${REPO}/issues/${ENTITY_NUMBER}/comments" -f "body=${body}" --jq '.id')"

echo "comment_id=$comment_id" >> "$GITHUB_OUTPUT"
