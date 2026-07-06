#!/usr/bin/env bash
# Rewrites the tracking comment with the final result. Always runs (even if
# `run.sh` failed or the gate said no), so the issue thread always ends up
# with a clear status instead of being stuck on "cruise is working...".
set -uo pipefail

REPO="${GITHUB_REPOSITORY:-}"
PROCEED="${PROCEED:-false}"
GATE_ERROR="${GATE_ERROR:-}"

if [ "$PROCEED" != "true" ]; then
  if [ -n "$GATE_ERROR" ]; then
    # gate.sh hard-failed on a genuine configuration error (e.g. no usable
    # API key) rather than a "this event doesn't apply" no-op -- PROCEED
    # reads the same as a plain skip in both cases, so GATE_ERROR is the
    # only signal that distinguishes them. Report it as a real failure
    # instead of `skipped`. There is usually no tracking comment yet in this
    # case (the comment-start step is itself gated on proceed=='true'), so
    # this only affects the `conclusion` output, not an issue comment.
    echo "cruise: gate reported a configuration error: $GATE_ERROR"
    echo "conclusion=failure" >> "$GITHUB_OUTPUT"
  else
    echo "cruise: nothing to finalize (gate did not proceed)"
    echo "conclusion=skipped" >> "$GITHUB_OUTPUT"
  fi
  exit 0
fi

COMMAND="${COMMAND:-run}"
COMMENT_ID="${COMMENT_ID:-}"
RUN_OUTCOME="${RUN_OUTCOME:-failure}"
START_TS="${START_TS:-}"
SESSION_ID="${SESSION_ID:-}"
PR_URL="${PR_URL:-}"
COMMIT_URL="${COMMIT_URL:-}"
PLAN_COMMENT_URL="${PLAN_COMMENT_URL:-}"
FAIL_REASON="${FAIL_REASON:-}"

now_ts="$(date +%s)"
duration=0
if [ -n "$START_TS" ]; then
  duration=$(( now_ts - START_TS ))
fi

job_url="${GITHUB_SERVER_URL:-https://github.com}/${REPO}/actions/runs/${GITHUB_RUN_ID:-}"

# NOTE: inside the two `comment_body="$( ... )"` blocks below, avoid
# comments that contain an unbalanced "(" or quote character -- older bash
# (e.g. bash 3.2, macOS's default /bin/bash) miscounts parens while scanning
# for the closing ")" of the substitution and mis-parses the whole script.
if [ "$RUN_OUTCOME" = "success" ]; then
  conclusion="success"
  comment_body="$(
    echo "✅ **cruise** finished in ${duration}s."
    echo
    [ -n "$SESSION_ID" ] && echo "- Session: \`${SESSION_ID}\`"
    if [ "$COMMAND" = "run" ]; then
      [ -n "$PR_URL" ] && echo "- Pull request: $PR_URL"
    elif [ "$COMMAND" = "exec" ]; then
      if [ -n "$COMMIT_URL" ]; then
        echo "- Commit: $COMMIT_URL"
      else
        echo "- No file changes were produced; nothing was pushed."
      fi
    elif [ "$COMMAND" = "plan" ]; then
      [ -n "$PLAN_COMMENT_URL" ] && echo "- Plan: $PLAN_COMMENT_URL"
    elif [ "$COMMAND" = "fix" ]; then
      [ -n "$PLAN_COMMENT_URL" ] && echo "- Revised plan: $PLAN_COMMENT_URL"
    fi
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
