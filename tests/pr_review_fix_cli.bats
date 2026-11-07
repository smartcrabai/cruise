#!/usr/bin/env bats

# Run with CRUISE_BIN pointing at the built cruise executable.
setup() {
  [ -n "${CRUISE_BIN:-}" ] && [ -x "$CRUISE_BIN" ] || { echo 'CRUISE_BIN must point to a built cruise executable' >&2; return 1; }
  export HOME="$BATS_TEST_TMPDIR/home"
  export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
  export XDG_DATA_HOME="$BATS_TEST_TMPDIR/data"
  export XDG_STATE_HOME="$BATS_TEST_TMPDIR/state"
  export CRUISE_DISABLE_NOTIFICATIONS=1 CRUISE_FORGE=github NO_COLOR=1
  export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
  mkdir -p "$HOME" "$BATS_TEST_TMPDIR/bin" "$BATS_TEST_TMPDIR/seed"
  export REAL_GIT="$(command -v git)"
  export PATH="$BATS_TEST_TMPDIR/bin:$PATH"
  cat > "$BATS_TEST_TMPDIR/bin/git" <<'EOF'
#!/bin/sh
case "$*" in
  "remote get-url --all origin"|"remote get-url --all --push origin") echo https://github.com/acme/app.git ;;
  *) exec "$REAL_GIT" "$@" ;;
esac
EOF
  chmod +x "$BATS_TEST_TMPDIR/bin/git"
  export CRUISE_SKIP_AGENT_AUTH_CHECK=1
  export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@example.com GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@example.com
  git init --bare -q "$BATS_TEST_TMPDIR/origin.git"
  git -C "$BATS_TEST_TMPDIR/seed" init -q -b main
  git -C "$BATS_TEST_TMPDIR/seed" -c commit.gpgsign=false commit -q --allow-empty -m initial
  git -C "$BATS_TEST_TMPDIR/seed" remote add origin "$BATS_TEST_TMPDIR/origin.git"
  git -C "$BATS_TEST_TMPDIR/seed" push -q origin HEAD:refs/heads/feat/x
  git --git-dir="$BATS_TEST_TMPDIR/origin.git" symbolic-ref HEAD refs/heads/feat/x
  git clone -q "$BATS_TEST_TMPDIR/origin.git" "$BATS_TEST_TMPDIR/repo"
  git -C "$BATS_TEST_TMPDIR/repo" checkout -q -b main
  git -C "$BATS_TEST_TMPDIR/repo" branch -D feat/x >/dev/null
  cat > "$BATS_TEST_TMPDIR/bin/gh" <<'EOF'
#!/bin/sh
case "$*" in
  *"pr create"*) echo 'unexpected PR creation' >&2; exit 1 ;;
  *collaborators/*/permission*) echo write ;;
  *resolveReviewThread*) touch "$BATS_TEST_TMPDIR/resolved"; echo '{"data":{"resolveReviewThread":{"thread":{"id":"t1","isResolved":true}}}}' ;;
  *reviewThreads*)
    if [ -f "$BATS_TEST_TMPDIR/review-enabled" ] && [ ! -f "$BATS_TEST_TMPDIR/resolved" ]; then
      echo '{"data":{"repository":{"pullRequest":{"reviewThreads":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"id":"t1","isResolved":false,"viewerCanReply":true,"viewerCanResolve":true,"path":"file.txt","line":1,"comments":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[{"author":{"login":"writer"},"body":"Please fix"}]}}]}}}}}'
    else
      echo '{"data":{"repository":{"pullRequest":{"reviewThreads":{"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[]}}}}}'
    fi ;;
  *headRefOid*) git --git-dir="$BATS_TEST_TMPDIR/origin.git" rev-parse refs/heads/feat/x ;;
  *"pr view"*)
    if [ -n "${PR_JSON:-}" ]; then printf '%s\n' "$PR_JSON"; else
      printf '%s\n' '{"number":12,"title":"Fix","url":"https://github.com/acme/app/pull/12","state":"OPEN","headRefName":"feat/x","baseRefName":"main","isCrossRepository":false,"isDraft":true}'
    fi ;;
  --version) echo 'gh version 2.103.0' ;;
  *) echo "unexpected gh invocation: $*" >&2; exit 1 ;;
esac
EOF
  chmod +x "$BATS_TEST_TMPDIR/bin/gh"
  cd "$BATS_TEST_TMPDIR/repo"
}

prepare_review_workflow() {
  touch "$BATS_TEST_TMPDIR/review-enabled"
  cat > "$BATS_TEST_TMPDIR/review-agent" <<'EOF'
#!/bin/sh
echo fixed > review-change.txt
if [ "${REVIEW_MODE:-commit}" = commit ]; then
  git add review-change.txt
  git -c commit.gpgsign=false commit -q -m 'address review'
fi
echo '{"actions":[{"thread_id":"t1","resolve":true}]}'
EOF
  chmod +x "$BATS_TEST_TMPDIR/review-agent"
  cat > "$BATS_TEST_TMPDIR/review-workflow.yaml" <<EOF
command: ["$BATS_TEST_TMPDIR/review-agent"]
create_pr: true
cleanup_after_pr: true
steps:
  sync-head:
    command: git pull --ff-only
    timeout: 5m
after-pr:
  address-review:
    github-review:
      reviewers: all
      max-iterations: 3
    timeout: 60m
    allow_commit: true
    output_file: review-report.json
    prompt: Address the review.
EOF
}

@test "review commit is pushed to the existing PR and completed worktree is cleaned" {
  prepare_review_workflow
  run "$CRUISE_BIN" plan --pr 12 -c "$BATS_TEST_TMPDIR/review-workflow.yaml" </dev/null
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
  state=$(find "$XDG_DATA_HOME/cruise/sessions" -name state.json -print)
  session=$(basename "$(dirname "$state")")
  worktree=$(jq -r '.worktree_path' "$state")
  before=$(git --git-dir="$BATS_TEST_TMPDIR/origin.git" rev-parse refs/heads/feat/x)
  run "$CRUISE_BIN" run "$session" </dev/null
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
  jq -e '.phase == "Completed"' "$state" || return 1
  after=$(git --git-dir="$BATS_TEST_TMPDIR/origin.git" rev-parse refs/heads/feat/x)
  [ "$after" != "$before" ] || return 1
  [ "$(git --git-dir="$BATS_TEST_TMPDIR/origin.git" show "$after:review-change.txt")" = fixed ] || return 1
  [ -f "$BATS_TEST_TMPDIR/resolved" ] || return 1
  [ ! -d "$worktree" ] || return 1
  report=$(find "$(dirname "$state")" -name review-report.json -print)
  [ -n "$report" ] && jq -e '.error == null and (.iterations | length) > 0' "$report"
}

@test "uncommitted review changes fail without pushing or resolving" {
  prepare_review_workflow
  export REVIEW_MODE=dirty
  run "$CRUISE_BIN" plan --pr 12 -c "$BATS_TEST_TMPDIR/review-workflow.yaml" </dev/null
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
  state=$(find "$XDG_DATA_HOME/cruise/sessions" -name state.json -print)
  session=$(basename "$(dirname "$state")")
  worktree=$(jq -r '.worktree_path' "$state")
  before=$(git --git-dir="$BATS_TEST_TMPDIR/origin.git" rev-parse refs/heads/feat/x)
  run "$CRUISE_BIN" run "$session" </dev/null
  [ "$status" -ne 0 ] || return 1
  [ "$(git --git-dir="$BATS_TEST_TMPDIR/origin.git" rev-parse refs/heads/feat/x)" = "$before" ] || return 1
  [ -f "$worktree/review-change.txt" ] || return 1
  [ ! -f "$BATS_TEST_TMPDIR/resolved" ] || return 1
  report=$(find "$(dirname "$state")" -name review-report.json -print)
  [ -n "$report" ] && jq -e '.error != null' "$report"
}

@test "execution syncs the existing PR head and reports without creating a PR" {
  run "$CRUISE_BIN" plan --pr 12 </dev/null
  [ "$status" -eq 0 ] || return 1
  state=$(find "$XDG_DATA_HOME/cruise/sessions" -name state.json -print)
  session=$(basename "$(dirname "$state")")
  worktree=$(jq -r '.worktree_path' "$state")
  git -C "$BATS_TEST_TMPDIR/seed" -c commit.gpgsign=false commit -q --allow-empty -m upstream
  git -C "$BATS_TEST_TMPDIR/seed" push -q origin HEAD:refs/heads/feat/x
  upstream=$(git --git-dir="$BATS_TEST_TMPDIR/origin.git" rev-parse refs/heads/feat/x)
  run "$CRUISE_BIN" run "$session" --no-cleanup-after-pr </dev/null
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
  [ "$(git -C "$worktree" rev-parse HEAD)" = "$upstream" ] || return 1
  report=$(find "$(dirname "$state")" -name review-report.json -print)
  [ -n "$report" ] && jq -e '.error == null and (.iterations | length) > 0' "$report"
}

@test "divergent upstream is rejected without overwriting local commits" {
  run "$CRUISE_BIN" plan --pr 12 </dev/null
  [ "$status" -eq 0 ] || return 1
  state=$(find "$XDG_DATA_HOME/cruise/sessions" -name state.json -print)
  session=$(basename "$(dirname "$state")")
  worktree=$(jq -r '.worktree_path' "$state")
  git -C "$worktree" -c commit.gpgsign=false commit -q --allow-empty -m local
  local_head=$(git -C "$worktree" rev-parse HEAD)
  git -C "$BATS_TEST_TMPDIR/seed" -c commit.gpgsign=false commit -q --allow-empty -m upstream
  git -C "$BATS_TEST_TMPDIR/seed" push -q origin HEAD:refs/heads/feat/x
  run "$CRUISE_BIN" run "$session" --no-cleanup-after-pr </dev/null
  [ "$status" -ne 0 ] || return 1
  [ "$(git -C "$worktree" rev-parse HEAD)" = "$local_head" ]
}

@test "plan for an existing PR persists a deterministic plan on its head branch" {
  run "$CRUISE_BIN" plan --pr 12 'focus on tests' </dev/null
  [ "$status" -eq 0 ] || return 1
  state=$(find "$XDG_DATA_HOME/cruise/sessions" -name state.json -print)
  [ "$(printf '%s\n' "$state" | wc -l | tr -d ' ')" -eq 1 ] || return 1
  jq -e '.phase == "Planned" and .pr_target.number == 12 and .pr_url == "https://github.com/acme/app/pull/12" and .input_as_plan == true and .worktree_branch == "feat/x"' "$state" || return 1
  plan="$(dirname "$state")/plan.md"
  [ "$(head -n 1 "$plan")" = '# PR #12 review fix: Fix' ] || return 1
  grep -F '## Additional instruction' "$plan" || return 1
  grep -F 'focus on tests' "$plan" || return 1
  worktree=$(jq -r '.worktree_path' "$state")
  [ "$(git -C "$worktree" branch --show-current)" = 'feat/x' ] || return 1
  [ "$(git -C "$worktree" rev-parse --abbrev-ref '@{u}')" = 'origin/feat/x' ]
}

@test "closed PR is rejected without creating a session" {
  export PR_JSON='{"number":12,"title":"Fix","url":"https://github.com/acme/app/pull/12","state":"CLOSED","headRefName":"feat/x","baseRefName":"main","isCrossRepository":false,"isDraft":false}'
  run "$CRUISE_BIN" plan --pr 12 </dev/null
  [ "$status" -ne 0 ] || return 1
  [[ "$(printf '%s' "$output" | tr '[:upper:]' '[:lower:]')" == *'only open pull requests'* ]] || return 1
  [ -z "$(find "$XDG_DATA_HOME/cruise/sessions" -name state.json -print 2>/dev/null)" ]
}

@test "fork PR is rejected without creating a session" {
  export PR_JSON='{"number":12,"title":"Fix","url":"https://github.com/acme/app/pull/12","state":"OPEN","headRefName":"feat/x","baseRefName":"main","isCrossRepository":true,"isDraft":false}'
  run "$CRUISE_BIN" plan --pr 12 </dev/null
  [ "$status" -ne 0 ] || return 1
  [[ "$(printf '%s' "$output" | tr '[:upper:]' '[:lower:]')" == *fork* ]] || return 1
  [ -z "$(find "$XDG_DATA_HOME/cruise/sessions" -name state.json -print 2>/dev/null)" ]
}

@test "PR planning rejects an incompatible workflow before session creation" {
  run "$CRUISE_BIN" plan --pr 12 -c builtin:tdd </dev/null
  [ "$status" -ne 0 ] || return 1
  [[ "$output" == *'builtin:pr-review-fix'* ]] || return 1
  [ -z "$(find "$XDG_DATA_HOME/cruise/sessions" -name state.json -print 2>/dev/null)" ]
}

@test "PR dry-run leaves no session" {
  run "$CRUISE_BIN" plan --pr 12 --dry-run </dev/null
  [ "$status" -eq 0 ] || return 1
  [[ "$output" == *12* ]] || return 1
  [ -z "$(find "$XDG_DATA_HOME/cruise/sessions" -name state.json -print 2>/dev/null)" ]
}
