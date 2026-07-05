#!/usr/bin/env bash
# Posts the initial "cruise is working..." tracking comment on the
# issue/PR. Works for both issues and PRs: GitHub's Issues API comment
# endpoint accepts a PR number too (a PR is an issue under the hood).
set -euo pipefail

REPO="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
ENTITY_NUMBER="${ENTITY_NUMBER:?ENTITY_NUMBER is required}"
MODE="${MODE:-issue}"
TRIGGER_ACTOR="${TRIGGER_ACTOR:-someone}"

job_url="${GITHUB_SERVER_URL:-https://github.com}/${REPO}/actions/runs/${GITHUB_RUN_ID:-}"

body="$(cat <<EOF
🧭 **cruise** is on it, @${TRIGGER_ACTOR} -- working on this ${MODE}... [View run](${job_url})
EOF
)"

comment_id="$(gh api "repos/${REPO}/issues/${ENTITY_NUMBER}/comments" -f "body=${body}" --jq '.id')"

echo "comment_id=$comment_id" >> "$GITHUB_OUTPUT"
