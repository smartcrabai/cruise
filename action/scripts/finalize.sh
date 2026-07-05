#!/usr/bin/env bash
# Rewrites the tracking comment with the final result. Always runs (even if
# `run.sh` failed or the gate said no), so the issue/PR thread always ends up
# with a clear status instead of being stuck on "cruise is working...".
set -uo pipefail

REPO="${GITHUB_REPOSITORY:-}"
PROCEED="${PROCEED:-false}"

if [ "$PROCEED" != "true" ]; then
  echo "cruise: nothing to finalize (gate did not proceed)"
  echo "conclusion=skipped" >> "$GITHUB_OUTPUT"
  exit 0
fi

COMMENT_ID="${COMMENT_ID:-}"
RUN_OUTCOME="${RUN_OUTCOME:-failure}"
START_TS="${START_TS:-}"
SESSION_ID="${SESSION_ID:-}"
PR_URL="${PR_URL:-}"
FAIL_REASON="${FAIL_REASON:-}"

now_ts="$(date +%s)"
duration=0
if [ -n "$START_TS" ]; then
  duration=$(( now_ts - START_TS ))
fi

job_url="${GITHUB_SERVER_URL:-https://github.com}/${REPO}/actions/runs/${GITHUB_RUN_ID:-}"

if [ "$RUN_OUTCOME" = "success" ]; then
  conclusion="success"
  comment_body="$(
    echo "✅ **cruise** finished in ${duration}s."
    echo
    [ -n "$SESSION_ID" ] && echo "- Session: \`${SESSION_ID}\`"
    [ -n "$PR_URL" ] && echo "- Pull request: $PR_URL"
    echo "- [View run]($job_url)"
    true
  )"
else
  conclusion="failure"
  # Deliberately no log excerpt here: run output can contain anything the
  # agent printed (including environment values coaxed out via prompt
  # injection), and GitHub's secret masking only applies to the Actions log
  # viewer, not to text this script re-posts through the API. Logs stay in
  # the (access-controlled) run page only.
  comment_body="$(
    echo "❌ **cruise** failed after ${duration}s."
    echo
    [ -n "$SESSION_ID" ] && echo "- Session: \`${SESSION_ID}\`"
    [ -n "$FAIL_REASON" ] && echo "- $FAIL_REASON"
    echo "- See the [run logs]($job_url) for details (logs are never posted to this thread)."
    true
  )"
fi

if [ -n "$COMMENT_ID" ]; then
  gh api "repos/${REPO}/issues/comments/${COMMENT_ID}" -X PATCH -f "body=${comment_body}" >/dev/null
else
  echo "cruise: no tracking comment id available, skipping comment update"
fi

echo "conclusion=$conclusion" >> "$GITHUB_OUTPUT"
