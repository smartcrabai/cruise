#!/usr/bin/env bash
# Decides whether cruise should run for this event: strict trigger-phrase
# match (word boundary, same rule as anthropics/claude-code-action) plus an
# actor authorization check (collaborator write/admin permission, or an
# allow-listed bot). Never hard-fails on "this event doesn't apply" -- it
# always exits 0 and communicates the verdict via `proceed`.
#
# For issue_comment / pull_request_review / pull_request_review_comment
# events, also writes the raw triggering comment/review body to
# $RUNNER_TEMP/cruise-trigger-body.txt and reports it as `trigger_body_file`.
# build-prompt.sh only fetches the issue/PR title+body via the REST API,
# which does not include a review's or a review comment's own text -- without
# this, the very "@cruise ..." request that triggered a review/review-comment
# run would never reach the prompt. `issues` (opened) is deliberately
# excluded: its body is already the issue body embedded in the Description
# section, so writing it again here would just duplicate it.
set -uo pipefail

TRIGGER_PHRASE="${TRIGGER_PHRASE:-@cruise}"
ALLOWED_BOTS="${ALLOWED_BOTS:-}"
EVENT_NAME="${GITHUB_EVENT_NAME:-}"
EVENT_PATH="${GITHUB_EVENT_PATH:-}"
REPO="${GITHUB_REPOSITORY:-}"

out() {
  echo "$1=$2" >> "$GITHUB_OUTPUT"
}

deny() {
  echo "cruise: not proceeding - $1"
  out proceed false
  out mode ""
  out entity_number ""
  out actor ""
  out is_bot false
  out trigger_body_file ""
  exit 0
}

if [ -z "$EVENT_PATH" ] || [ ! -f "$EVENT_PATH" ]; then
  deny "no event payload found"
fi

if ! command -v jq >/dev/null 2>&1; then
  deny "jq is not available"
fi

# Missing python3 is a runner-configuration error, not a "this event doesn't
# apply" case, so hard-fail instead of silently skipping.
if ! command -v python3 >/dev/null 2>&1; then
  echo "::error::python3 is required by the cruise action (GitHub-hosted runners include it; self-hosted runners must install it)" >&2
  exit 1
fi

jqr() {
  jq -r "$1 // empty" "$EVENT_PATH"
}

action="$(jqr '.action')"
mode=""
number=""
body=""
actor=""
actor_type=""

case "$EVENT_NAME" in
  issue_comment)
    if [ "$action" != "created" ]; then
      deny "issue_comment action is '$action', not 'created'"
    fi
    body="$(jqr '.comment.body')"
    actor="$(jqr '.comment.user.login')"
    actor_type="$(jqr '.comment.user.type')"
    number="$(jqr '.issue.number')"
    is_pr="$(jq -r 'if .issue.pull_request then "true" else "false" end' "$EVENT_PATH")"
    if [ "$is_pr" = "true" ]; then
      mode="pr"
    else
      mode="issue"
    fi
    ;;
  issues)
    if [ "$action" != "opened" ]; then
      deny "issues action is '$action', not 'opened'"
    fi
    title="$(jqr '.issue.title')"
    issue_body="$(jqr '.issue.body')"
    body="$title
$issue_body"
    actor="$(jqr '.issue.user.login')"
    actor_type="$(jqr '.issue.user.type')"
    number="$(jqr '.issue.number')"
    mode="issue"
    ;;
  pull_request_review_comment)
    if [ "$action" != "created" ]; then
      deny "pull_request_review_comment action is '$action', not 'created'"
    fi
    body="$(jqr '.comment.body')"
    actor="$(jqr '.comment.user.login')"
    actor_type="$(jqr '.comment.user.type')"
    number="$(jqr '.pull_request.number')"
    mode="pr"
    ;;
  pull_request_review)
    if [ "$action" != "submitted" ]; then
      deny "pull_request_review action is '$action', not 'submitted'"
    fi
    body="$(jqr '.review.body')"
    actor="$(jqr '.review.user.login')"
    actor_type="$(jqr '.review.user.type')"
    number="$(jqr '.pull_request.number')"
    mode="pr"
    ;;
  *)
    deny "unsupported event: $EVENT_NAME"
    ;;
esac

if [ -z "$number" ] || [ "$number" = "null" ]; then
  deny "could not determine issue/PR number"
fi

# Strict word-boundary trigger match: (^|\s)<phrase>([\s.,!?;:]|$), the same
# rule anthropics/claude-code-action uses, matched case-insensitively. The
# whole check runs inside Python so re.escape() and the engine that consumes
# it always agree (Python 3.7+ re.escape() is not compatible with POSIX ERE
# for every metacharacter).
if ! printf '%s' "$body" | python3 -c '
import re, sys
phrase = sys.argv[1]
body = sys.stdin.read()
pattern = r"(^|\s)" + re.escape(phrase) + r"([\s.,!?;:]|$)"
sys.exit(0 if re.search(pattern, body, re.IGNORECASE) else 1)
' "$TRIGGER_PHRASE"; then
  deny "trigger phrase '$TRIGGER_PHRASE' not found in body"
fi

if [ -z "$actor" ] || [ "$actor" = "null" ]; then
  deny "could not determine actor"
fi

is_bot=false
case "$actor" in
  *"[bot]") is_bot=true ;;
esac
if [ "$actor_type" = "Bot" ]; then
  is_bot=true
fi

if [ "$is_bot" = "true" ]; then
  allowed=false
  trimmed_allowed="$(printf '%s' "$ALLOWED_BOTS" | tr -d '[:space:]')"
  if [ "$trimmed_allowed" = "*" ]; then
    allowed=true
  else
    normalized_actor="$(printf '%s' "$actor" | tr '[:upper:]' '[:lower:]')"
    normalized_actor="${normalized_actor%\[bot\]}"
    old_ifs="$IFS"
    IFS=','
    for b in $ALLOWED_BOTS; do
      nb="$(printf '%s' "$b" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')"
      nb="${nb%\[bot\]}"
      if [ -n "$nb" ] && [ "$nb" = "$normalized_actor" ]; then
        allowed=true
      fi
    done
    IFS="$old_ifs"
  fi
  if [ "$allowed" != "true" ]; then
    deny "bot actor '$actor' is not in allowed_bots ('$ALLOWED_BOTS')"
  fi
else
  # The `permission` field only ever reports admin/write/read/none; the
  # "maintain" and "triage" roles are mapped to write and read respectively
  # (they appear only in `role_name`), so maintainers pass the `write` arm.
  permission="$(gh api "repos/$REPO/collaborators/$actor/permission" --jq '.permission' 2>/dev/null || true)"
  case "$permission" in
    admin | write) : ;;
    *) deny "actor '$actor' has insufficient permission: '${permission:-unknown}'" ;;
  esac
fi

trigger_body_file=""
case "$EVENT_NAME" in
  issue_comment | pull_request_review | pull_request_review_comment)
    trigger_body_file="${RUNNER_TEMP:-/tmp}/cruise-trigger-body.txt"
    mkdir -p "$(dirname "$trigger_body_file")"
    printf '%s' "$body" > "$trigger_body_file"
    ;;
esac

out proceed true
out mode "$mode"
out entity_number "$number"
out actor "$actor"
out is_bot "$is_bot"
out trigger_body_file "$trigger_body_file"
echo "cruise: triggered by @$actor on $mode #$number"
