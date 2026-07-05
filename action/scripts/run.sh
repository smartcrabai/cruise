#!/usr/bin/env bash
# Runs cruise for this event.
#
# Issue mode:  cruise --plan stdin --skip-planning   (skip_planning=true)
#              cruise plan                           (skip_planning=false, auto-approved: non-TTY)
#              -> cruise run <session-id>            (worktree mode -> pushes a branch + opens a draft PR)
#
# PR mode:     gh pr checkout <number>
#              -> cruise exec (no plan, no worktree, no PR -- runs on the checked-out branch)
#              -> git add/commit/push directly, since `cruise exec` never commits.
set -uo pipefail

MODE="${MODE:?MODE is required (issue|pr)}"
ENTITY_NUMBER="${ENTITY_NUMBER:?ENTITY_NUMBER is required}"
SKIP_PLANNING="${SKIP_PLANNING:-false}"
CONFIG_PATH="${CONFIG_PATH:?CONFIG_PATH is required}"
# PR mode runs `cruise exec`, which binds the whole task to {input} and
# leaves {plan} empty (there is no planning step) -- resolve-config.sh
# generates (or resolves) a separate PR-mode config built around {input} for
# exactly this reason. Falls back to CONFIG_PATH so older callers of this
# script (or a manual invocation) that only set CONFIG_PATH don't hard-fail;
# resolve-config.sh always sets both to the same file when `config` is a
# user-supplied path.
PR_CONFIG_PATH="${PR_CONFIG_PATH:-$CONFIG_PATH}"
TASK_FILE="${TASK_FILE:?TASK_FILE is required}"
REPO="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
WORKSPACE="${GITHUB_WORKSPACE:-$(pwd)}"

# git identity for commits cruise/this action creates: an explicit
# git_user_name/git_user_email input always wins; otherwise identify as the
# cruise-agent App bot when the run used its installation token (USED_APP is
# set by app-token.sh), falling back to github-actions[bot] when it didn't
# (e.g. GITHUB_TOKEN fallback, or a hand-supplied github_token).
GIT_USER_NAME_INPUT="${GIT_USER_NAME_INPUT:-}"
GIT_USER_EMAIL_INPUT="${GIT_USER_EMAIL_INPUT:-}"
USED_APP="${USED_APP:-false}"

if [ -n "$GIT_USER_NAME_INPUT" ]; then
  git_user_name="$GIT_USER_NAME_INPUT"
elif [ "$USED_APP" = "true" ]; then
  git_user_name="cruise-agent[bot]"
else
  git_user_name="github-actions[bot]"
fi

if [ -n "$GIT_USER_EMAIL_INPUT" ]; then
  git_user_email="$GIT_USER_EMAIL_INPUT"
elif [ "$USED_APP" = "true" ]; then
  git_user_email="299756300+cruise-agent[bot]@users.noreply.github.com"
else
  git_user_email="41898282+github-actions[bot]@users.noreply.github.com"
fi

export GIT_AUTHOR_NAME="$git_user_name"
export GIT_AUTHOR_EMAIL="$git_user_email"
export GIT_COMMITTER_NAME="$git_user_name"
export GIT_COMMITTER_EMAIL="$git_user_email"

CRUISE_DIR="${RUNNER_TEMP:-/tmp}/cruise"
export XDG_DATA_HOME="$CRUISE_DIR/data"
export XDG_CONFIG_HOME="$CRUISE_DIR/xdg-config"
export XDG_STATE_HOME="$CRUISE_DIR/xdg-state"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_STATE_HOME"
export CRUISE_CONFIG="$CONFIG_PATH"

LOG_FILE="$CRUISE_DIR/run.log"
: > "$LOG_FILE"
echo "log_file=$LOG_FILE" >> "$GITHUB_OUTPUT"

# cruise (and this script, in PR mode) push to `origin` over HTTPS. Configure
# auth the same way actions/checkout does: an AUTHORIZATION extraheader scoped
# to the server URL. When the workflow's checkout step persisted its own
# credentials (persist-credentials: true, the default), git prefers that
# extraheader over credentials embedded in the remote URL, so replacing the
# header -- not the URL -- is the only way to guarantee `github_token` is the
# token actually used for pushes. Worktrees share .git/config, so this covers
# cruise's worktree pushes too. Token values are never echoed.
if [ -n "${GH_TOKEN:-}" ]; then
  server_url="${GITHUB_SERVER_URL:-https://github.com}"
  auth_b64="$(printf '%s' "x-access-token:${GH_TOKEN}" | base64 | tr -d '\n')"
  # auth_b64 trivially decodes back to the raw token (it's plain base64, not
  # encryption), so it must be masked in the Actions log exactly like the
  # token itself.
  echo "::add-mask::${auth_b64}"
  git -C "$WORKSPACE" config --local --unset-all "http.${server_url}/.extraheader" 2>/dev/null || true
  git -C "$WORKSPACE" config --local "http.${server_url}/.extraheader" "AUTHORIZATION: basic ${auth_b64}"
fi

latest_session_id() {
  cruise list --json 2>/dev/null | jq -r 'sort_by(.created_at) | last | .id // empty'
}

emit_and_exit() {
  # $1=session_id $2=pr_url $3=conclusion $4=exit_code $5=fail_reason (single line, optional)
  {
    echo "session_id=$1"
    echo "pr_url=$2"
    echo "conclusion=$3"
    echo "fail_reason=${5:-}"
  } >> "$GITHUB_OUTPUT"
  exit "$4"
}

if [ "$MODE" = "pr" ]; then
  echo "cruise: checking out PR #$ENTITY_NUMBER" | tee -a "$LOG_FILE"
  if ! gh pr checkout "$ENTITY_NUMBER" --repo "$REPO" >>"$LOG_FILE" 2>&1; then
    echo "cruise: failed to check out PR #$ENTITY_NUMBER" | tee -a "$LOG_FILE"
    emit_and_exit "" "" "failure" 1
  fi
  task_content="$(cat "$TASK_FILE")"

  cruise exec -c "$PR_CONFIG_PATH" -- "$task_content" 2>&1 | tee -a "$LOG_FILE"
  exec_exit=$?
  session_id="$(latest_session_id)"

  if [ "$exec_exit" -ne 0 ]; then
    emit_and_exit "$session_id" "" "failure" 1
  fi

  if [ -n "$(git -C "$WORKSPACE" status --porcelain)" ]; then
    if ! git -C "$WORKSPACE" add -A >>"$LOG_FILE" 2>&1 \
      || ! git -C "$WORKSPACE" commit -m "cruise: address request on #$ENTITY_NUMBER" >>"$LOG_FILE" 2>&1; then
      echo "cruise: git commit failed" | tee -a "$LOG_FILE"
      emit_and_exit "$session_id" "" "failure" 1
    fi
    # Plain `git push` (no explicit refspec): `gh pr checkout` already set up
    # branch tracking, including for cross-fork PRs (pushing there succeeds
    # only if GH_TOKEN has write access to the fork, which is rarely true --
    # that failure surfaces here rather than silently pushing to the wrong
    # branch in this repository).
    if ! git -C "$WORKSPACE" push >>"$LOG_FILE" 2>&1; then
      echo "cruise: git push failed" | tee -a "$LOG_FILE"
      emit_and_exit "$session_id" "" "failure" 1
    fi
  else
    echo "cruise: no file changes produced, nothing to push" | tee -a "$LOG_FILE"
  fi

  pr_url="${GITHUB_SERVER_URL:-https://github.com}/${REPO}/pull/${ENTITY_NUMBER}"
  emit_and_exit "$session_id" "$pr_url" "success" 0
fi

# Issue mode.
if [ "$SKIP_PLANNING" = "true" ]; then
  cruise --plan stdin --skip-planning < "$TASK_FILE" 2>&1 | tee -a "$LOG_FILE"
else
  cruise plan -c "$CONFIG_PATH" < "$TASK_FILE" 2>&1 | tee -a "$LOG_FILE"
fi
plan_exit=$?
session_id="$(latest_session_id)"

if [ "$plan_exit" -ne 0 ] || [ -z "$session_id" ]; then
  echo "cruise: plan creation failed" | tee -a "$LOG_FILE"
  emit_and_exit "$session_id" "" "failure" 1
fi

cruise run "$session_id" 2>&1 | tee -a "$LOG_FILE"
run_exit=$?

session_json="$(cruise list --json 2>/dev/null | jq -c --arg id "$session_id" '.[] | select(.id==$id)')"
pr_url="$(printf '%s' "$session_json" | jq -r '.pr_url // empty')"

if [ "$run_exit" -ne 0 ]; then
  emit_and_exit "$session_id" "$pr_url" "failure" 1
fi

# cruise treats `gh pr create` failure on local (non --repo) sessions as a
# warning only: it exits 0 and marks the session Completed with pr_url unset
# (src/worktree_pr.rs). An empty pr_url after a successful run therefore means
# the PR was NOT opened even though changes may already sit on a pushed
# branch. Surface that as a failure instead of reporting silent success.
if [ -z "$pr_url" ]; then
  echo "cruise: run completed but no pull request was recorded" | tee -a "$LOG_FILE"
  emit_and_exit "$session_id" "" "failure" 1 \
    "cruise completed but no pull request was created (changes may have been pushed to a branch without a PR). Check that the workflow grants \`pull-requests: write\` and that branch protection allows PR creation."
fi

emit_and_exit "$session_id" "$pr_url" "success" 0
