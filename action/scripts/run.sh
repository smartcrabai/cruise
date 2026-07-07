#!/usr/bin/env bash
# Runs the cruise command selected by gate.sh (run/exec/plan/fix).
#
# run:  resolve plan source (last plan-marker comment, else issue title+body)
#       -> cruise --plan stdin --skip-planning (creates a session verbatim)
#       -> cruise run <session-id>   (worktree -> branch -> push -> draft PR)
#
# exec: resolve plan source (same as run)
#       -> cruise exec -c <generated {input} config> -- "<plan text>"
#          on the already-checked-out default branch
#       -> action commits + pushes directly to the default branch (no PR).
#
# plan: cruise plan < issue title+body   (LLM planning, non-interactive
#       auto-approve) -> post a NEW plan-marker tracking comment.
#
# fix:  find the last plan-marker comment (fail if none) -> cruise plan on a
#       composed "existing plan + user feedback" input -> PATCH the SAME
#       comment with the revised plan.
set -uo pipefail

COMMAND="${COMMAND:?COMMAND is required (run|exec|plan|fix)}"
ENTITY_NUMBER="${ENTITY_NUMBER:?ENTITY_NUMBER is required}"
COMMAND_REST_FILE="${COMMAND_REST_FILE:-}"
EXEC_CONFIG_PATH="${EXEC_CONFIG_PATH:-}"
REPO="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
WORKSPACE="${GITHUB_WORKSPACE:-$(pwd)}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/plan.sh
source "$SCRIPT_DIR/lib/plan.sh"

# git identity for commits cruise/this action creates: an explicit
# git_user_name/git_user_email input always wins; otherwise identify as the
# cruise-agent App bot when the run used its installation token (USED_APP is
# set by app-token.sh), falling back to github-actions[bot] when it didn't
# (e.g. GITHUB_TOKEN fallback, or a hand-supplied github_token).
GIT_USER_NAME_INPUT="${GIT_USER_NAME_INPUT:-}"
GIT_USER_EMAIL_INPUT="${GIT_USER_EMAIL_INPUT:-}"
USED_APP="${USED_APP:-false}"
TRIGGER_ACTOR="${TRIGGER_ACTOR:-}"
TRIGGER_ACTOR_ID="${TRIGGER_ACTOR_ID:-}"

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

commit_coauthor_name=""
commit_coauthor_email=""
init_commit_coauthor() {
  # GitHub recognizes ID-based noreply addresses and keeps a user's private
  # email hidden while still associating the trailer with their account.
  if [ -z "$TRIGGER_ACTOR" ] || [ -z "$TRIGGER_ACTOR_ID" ]; then
    return
  fi
  if [[ ! "$TRIGGER_ACTOR" =~ ^[A-Za-z0-9][A-Za-z0-9-]*(\[bot\])?$ ]]; then
    return
  fi
  if [[ ! "$TRIGGER_ACTOR_ID" =~ ^[0-9]+$ ]]; then
    return
  fi

  commit_coauthor_name="$TRIGGER_ACTOR"
  commit_coauthor_email="${TRIGGER_ACTOR_ID}+${TRIGGER_ACTOR}@users.noreply.github.com"
  export CRUISE_COMMIT_COAUTHOR_NAME="$commit_coauthor_name"
  export CRUISE_COMMIT_COAUTHOR_EMAIL="$commit_coauthor_email"
}
init_commit_coauthor

CRUISE_DIR="${RUNNER_TEMP:-/tmp}/cruise"
export XDG_DATA_HOME="$CRUISE_DIR/data"
export XDG_CONFIG_HOME="$CRUISE_DIR/xdg-config"
export XDG_STATE_HOME="$CRUISE_DIR/xdg-state"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_STATE_HOME"

LOG_FILE="$CRUISE_DIR/run.log"
: > "$LOG_FILE"
echo "log_file=$LOG_FILE" >> "$GITHUB_OUTPUT"

commit_coauthor_hook_installed=false
previous_hooks_path_set=false
previous_hooks_path=""

restore_commit_coauthor_hook() {
  if [ "$commit_coauthor_hook_installed" != "true" ]; then
    return
  fi

  if [ "$previous_hooks_path_set" = "true" ]; then
    git -C "$WORKSPACE" config --local core.hooksPath "$previous_hooks_path" >>"$LOG_FILE" 2>&1 \
      || echo "::warning::cruise: failed to restore previous git core.hooksPath"
  else
    git -C "$WORKSPACE" config --local --unset core.hooksPath >>"$LOG_FILE" 2>&1 || true
  fi
}
trap restore_commit_coauthor_hook EXIT

install_commit_coauthor_hook() {
  if [ -z "$commit_coauthor_name" ] || [ -z "$commit_coauthor_email" ]; then
    return
  fi

  local hooks_dir hook_path
  hooks_dir="$CRUISE_DIR/git-hooks"
  hook_path="$hooks_dir/prepare-commit-msg"
  mkdir -p "$hooks_dir"
  cat > "$hook_path" <<'EOF'
#!/usr/bin/env bash
set -uo pipefail

msg_file="${1:-}"
if [ -z "$msg_file" ] || [ ! -f "$msg_file" ]; then
  exit 0
fi

name="${CRUISE_COMMIT_COAUTHOR_NAME:-}"
email="${CRUISE_COMMIT_COAUTHOR_EMAIL:-}"
if [ -z "$name" ] || [ -z "$email" ]; then
  exit 0
fi

trailer="Co-authored-by: $name <$email>"
if grep -Fqx "$trailer" "$msg_file"; then
  exit 0
fi

git interpret-trailers --in-place --trailer "$trailer" "$msg_file" >/dev/null 2>&1 \
  || printf '\n%s\n' "$trailer" >> "$msg_file"
EOF
  chmod +x "$hook_path"

  if previous_hooks_path="$(git -C "$WORKSPACE" config --local --get core.hooksPath 2>/dev/null)"; then
    previous_hooks_path_set=true
  fi
  if ! git -C "$WORKSPACE" config --local core.hooksPath "$hooks_dir" >>"$LOG_FILE" 2>&1; then
    echo "::warning::cruise: failed to install commit co-author hook; run-mode commits may omit Co-authored-by"
  else
    commit_coauthor_hook_installed=true
  fi
}
install_commit_coauthor_hook

# cruise (and this script, for `run`/`exec`) push to `origin` over HTTPS.
# Configure auth the same way actions/checkout does: an AUTHORIZATION
# extraheader scoped to the server URL. When the workflow's checkout step
# persisted its own credentials (persist-credentials: true, the default),
# git prefers that extraheader over credentials embedded in the remote URL,
# so replacing the header -- not the URL -- is the only way to guarantee
# `github_token` is the token actually used for pushes. Worktrees share
# .git/config, so this covers cruise's worktree pushes too. Harmless no-op
# for plan/fix, which never push. Token values are never echoed.
if [ -n "${GH_TOKEN:-}" ]; then
  server_url="${GITHUB_SERVER_URL:-https://github.com}"
  auth_b64="$(printf '%s' "x-access-token:${GH_TOKEN}" | base64 | tr -d '\n')"
  # auth_b64 trivially decodes back to the raw token (it's plain base64, not
  # encryption), so it must be masked in the Actions log exactly like the
  # token itself.
  echo "::add-mask::${auth_b64}"
  # actions/checkout v7 no longer writes its AUTHORIZATION extraheader into
  # .git/config directly: it stores it in an external credentials config
  # wired in via includeIf.gitdir entries (including .git/worktrees/*), so a
  # plain --unset-all of the extraheader key cannot see it. If both that
  # include and our own extraheader survive, git sends TWO Authorization
  # headers and GitHub rejects the push with 400 'Duplicate header'. Drop
  # every includeIf entry that points at a credentials config first.
  while IFS=' ' read -r key path; do
    case "$path" in
      *git-credentials*)
        git -C "$WORKSPACE" config --local --unset-all "$key" 2>/dev/null || true
        ;;
    esac
  done < <(git -C "$WORKSPACE" config --local --get-regexp '^includeif\..*\.path$' 2>/dev/null || true)
  git -C "$WORKSPACE" config --local --unset-all "http.${server_url}/.extraheader" 2>/dev/null || true
  git -C "$WORKSPACE" config --local "http.${server_url}/.extraheader" "AUTHORIZATION: basic ${auth_b64}"
fi

latest_session_id() {
  cruise list --json 2>/dev/null | jq -r 'sort_by(.created_at) | last | .id // empty'
}

session_plan_path() { # $1=session_id
  printf '%s/cruise/sessions/%s/plan.md' "$XDG_DATA_HOME" "$1"
}

emit_and_exit() { # $1=session_id $2=pr_url $3=commit_url $4=plan_comment_url $5=exit_code $6=fail_reason (optional, single line)
  {
    echo "session_id=$1"
    echo "pr_url=$2"
    echo "commit_url=$3"
    echo "plan_comment_url=$4"
    echo "fail_reason=${6:-}"
  } >> "$GITHUB_OUTPUT"
  exit "$5"
}

do_run() {
  local plan_file plan_source resolve_rc
  plan_file="$(mktemp)"
  plan_source="$(resolve_plan_source "$REPO" "$ENTITY_NUMBER" "$plan_file" "$COMMAND_REST_FILE")"
  resolve_rc=$?
  if [ "$resolve_rc" -eq 2 ]; then
    emit_and_exit "" "" "" "" 1 "failed to fetch issue comments while resolving the plan source (see run logs); aborting rather than risk running against the wrong plan."
  fi
  echo "cruise: plan source = $plan_source" | tee -a "$LOG_FILE"

  cruise --plan stdin --skip-planning < "$plan_file" 2>&1 | tee -a "$LOG_FILE"
  local plan_exit=$?
  local session_id
  session_id="$(latest_session_id)"
  if [ "$plan_exit" -ne 0 ] || [ -z "$session_id" ]; then
    echo "cruise: failed to create a session from the resolved plan" | tee -a "$LOG_FILE"
    emit_and_exit "$session_id" "" "" "" 1 "cruise failed to create a session from the resolved plan (see run logs)."
  fi

  cruise run "$session_id" 2>&1 | tee -a "$LOG_FILE"
  local run_exit=$?

  local session_json pr_url
  session_json="$(cruise list --json 2>/dev/null | jq -c --arg id "$session_id" '.[] | select(.id==$id)')"
  pr_url="$(printf '%s' "$session_json" | jq -r '.pr_url // empty')"

  if [ "$run_exit" -ne 0 ]; then
    echo "cruise: run failed" | tee -a "$LOG_FILE"
    emit_and_exit "$session_id" "$pr_url" "" "" 1 "cruise run failed -- see the run logs for details."
  fi

  # cruise treats `gh pr create` failure on local (non --repo) sessions as a
  # warning only: it exits 0 and marks the session Completed with pr_url
  # unset (src/worktree_pr.rs). An empty pr_url after a successful run
  # therefore means the PR was NOT opened even though changes may already
  # sit on a pushed branch. Surface that as a failure instead of reporting
  # silent success.
  if [ -z "$pr_url" ]; then
    echo "cruise: run completed but no pull request was recorded" | tee -a "$LOG_FILE"
    emit_and_exit "$session_id" "" "" "" 1 \
      "cruise completed but no pull request was created (changes may have been pushed to a branch without a PR). Check that the workflow grants \`pull-requests: write\` and that branch protection allows PR creation."
  fi

  emit_and_exit "$session_id" "$pr_url" "" "" 0
}

do_exec() {
  if [ -z "$EXEC_CONFIG_PATH" ]; then
    emit_and_exit "" "" "" "" 1 "internal error: EXEC_CONFIG_PATH was not set."
  fi

  local plan_file plan_source plan_content resolve_rc
  plan_file="$(mktemp)"
  plan_source="$(resolve_plan_source "$REPO" "$ENTITY_NUMBER" "$plan_file" "$COMMAND_REST_FILE")"
  resolve_rc=$?
  if [ "$resolve_rc" -eq 2 ]; then
    emit_and_exit "" "" "" "" 1 "failed to fetch issue comments while resolving the plan source (see run logs); aborting rather than risk executing against the wrong plan."
  fi
  echo "cruise: plan source = $plan_source" | tee -a "$LOG_FILE"
  plan_content="$(cat "$plan_file")"

  cruise exec -c "$EXEC_CONFIG_PATH" -- "$plan_content" 2>&1 | tee -a "$LOG_FILE"
  local exec_exit=$?
  local session_id
  session_id="$(latest_session_id)"

  if [ "$exec_exit" -ne 0 ]; then
    emit_and_exit "$session_id" "" "" "" 1 "cruise exec failed (see run logs)."
  fi

  if [ -z "$(git -C "$WORKSPACE" status --porcelain)" ]; then
    echo "cruise: no file changes produced, nothing to push" | tee -a "$LOG_FILE"
    emit_and_exit "$session_id" "" "" "" 0
  fi

  commit_args=(-m "cruise: address request on #$ENTITY_NUMBER")
  if [ -n "$commit_coauthor_name" ] && [ -n "$commit_coauthor_email" ]; then
    commit_args+=(-m "Co-authored-by: $commit_coauthor_name <$commit_coauthor_email>")
  fi

  if ! git -C "$WORKSPACE" add -A >>"$LOG_FILE" 2>&1 \
    || ! git -C "$WORKSPACE" commit "${commit_args[@]}" >>"$LOG_FILE" 2>&1; then
    echo "cruise: git commit failed" | tee -a "$LOG_FILE"
    emit_and_exit "$session_id" "" "" "" 1 "git commit failed (see run logs)."
  fi

  # Plain `git push` (no explicit refspec): the workflow's checkout already
  # set up branch tracking against the default branch. This pushes straight
  # to that branch -- no PR, no review step. See docs/github-actions.md for
  # why this is an advanced/opt-in mode (branch protection interacts badly
  # with an unattended agent pushing directly).
  if ! git -C "$WORKSPACE" push >>"$LOG_FILE" 2>&1; then
    echo "cruise: git push failed" | tee -a "$LOG_FILE"
    emit_and_exit "$session_id" "" "" "" 1 "git push to the default branch failed (see run logs; check branch protection rules)."
  fi

  local sha commit_url
  sha="$(git -C "$WORKSPACE" rev-parse HEAD)"
  commit_url="${GITHUB_SERVER_URL:-https://github.com}/${REPO}/commit/${sha}"
  emit_and_exit "$session_id" "" "$commit_url" "" 0
}

do_plan() {
  local issue_json title issue_body input_file
  issue_json="$(gh api "repos/$REPO/issues/$ENTITY_NUMBER")"
  title="$(printf '%s' "$issue_json" | jq -r '.title // ""' | sanitize_text)"
  issue_body="$(printf '%s' "$issue_json" | jq -r '.body // ""' | sanitize_text)"
  input_file="$(mktemp)"
  { printf '%s\n\n%s\n' "$title" "$issue_body"; } > "$input_file"

  cruise plan < "$input_file" 2>&1 | tee -a "$LOG_FILE"
  local plan_exit=$?
  local session_id
  session_id="$(latest_session_id)"
  if [ "$plan_exit" -ne 0 ] || [ -z "$session_id" ]; then
    emit_and_exit "$session_id" "" "" "" 1 "cruise plan failed to generate a plan (see run logs)."
  fi

  local plan_md
  plan_md="$(session_plan_path "$session_id")"
  if [ ! -s "$plan_md" ]; then
    emit_and_exit "$session_id" "" "" "" 1 "cruise plan finished but produced an empty plan.md."
  fi

  local rendered body resp comment_url
  rendered="$(mktemp)"
  render_plan_comment "$plan_md" "$rendered"
  body="$(cap_comment_body "$rendered")"
  resp="$(gh api "repos/$REPO/issues/$ENTITY_NUMBER/comments" -f "body=${body}")"
  comment_url="$(printf '%s' "$resp" | jq -r '.html_url // empty')"
  if [ -z "$comment_url" ]; then
    emit_and_exit "$session_id" "" "" "" 1 "cruise generated a plan but posting the plan comment failed."
  fi

  emit_and_exit "$session_id" "" "" "$comment_url" 0
}

do_fix() {
  local feedback
  feedback="$( { [ -n "$COMMAND_REST_FILE" ] && [ -f "$COMMAND_REST_FILE" ] && cat "$COMMAND_REST_FILE"; } | sanitize_text)"

  local body_file existing_id find_rc
  body_file="$(mktemp)"
  existing_id="$(find_last_plan_comment "$REPO" "$ENTITY_NUMBER" "$body_file")"
  find_rc=$?
  if [ "$find_rc" -eq 2 ]; then
    rm -f "$body_file"
    emit_and_exit "" "" "" "" 1 "failed to fetch issue comments while looking for an existing plan (see run logs); aborting rather than risk revising the wrong plan."
  fi
  if [ -z "$existing_id" ]; then
    rm -f "$body_file"
    emit_and_exit "" "" "" "" 1 "No existing plan comment found -- run \`@cruise plan\` first, then \`@cruise fix <feedback>\` to revise it. (If you meant 'fix ...' as plain text rather than the \`fix\` command: the word right after the @cruise mention is always parsed as a command. Use \`@cruise run <request>\` to execute directly, or \`@cruise plan <request>\` to draft a plan first.)"
  fi

  local existing_plan
  existing_plan="$(mktemp)"
  extract_plan_body "$body_file" > "$existing_plan"
  rm -f "$body_file"

  local input_file
  input_file="$(mktemp)"
  {
    echo "# Revise an existing implementation plan"
    echo
    echo "## Existing plan"
    echo
    cat "$existing_plan"
    echo
    echo "## User feedback"
    echo
    if [ -n "$feedback" ]; then
      printf '%s\n' "$feedback"
    else
      echo "(no additional feedback was provided)"
    fi
    echo
    echo "## Instructions"
    echo
    echo "Revise the plan above to incorporate the user's feedback. Output the complete, revised plan (not just a diff)."
  } > "$input_file"

  cruise plan < "$input_file" 2>&1 | tee -a "$LOG_FILE"
  local plan_exit=$?
  local session_id
  session_id="$(latest_session_id)"
  if [ "$plan_exit" -ne 0 ] || [ -z "$session_id" ]; then
    emit_and_exit "$session_id" "" "" "" 1 "cruise plan failed to revise the plan (see run logs)."
  fi

  local plan_md
  plan_md="$(session_plan_path "$session_id")"
  if [ ! -s "$plan_md" ]; then
    emit_and_exit "$session_id" "" "" "" 1 "cruise plan finished but produced an empty revised plan.md."
  fi

  local rendered body resp comment_url
  rendered="$(mktemp)"
  render_plan_comment "$plan_md" "$rendered"
  body="$(cap_comment_body "$rendered")"
  resp="$(gh api "repos/$REPO/issues/comments/$existing_id" -X PATCH -f "body=${body}")"
  comment_url="$(printf '%s' "$resp" | jq -r '.html_url // empty')"
  if [ -z "$comment_url" ]; then
    emit_and_exit "$session_id" "" "" "" 1 "cruise revised the plan but editing the plan comment failed."
  fi

  emit_and_exit "$session_id" "" "" "$comment_url" 0
}

case "$COMMAND" in
  run) do_run ;;
  exec) do_exec ;;
  plan) do_plan ;;
  fix) do_fix ;;
  *)
    echo "::error::cruise: unknown command '$COMMAND'" >&2
    emit_and_exit "" "" "" "" 1 "internal error: unknown command '$COMMAND'."
    ;;
esac
