#!/usr/bin/env bats

# Run with CRUISE_BIN pointing at the built cruise executable.
setup() {
  [ -n "${CRUISE_BIN:-}" ] && [ -x "$CRUISE_BIN" ] || { echo 'CRUISE_BIN must point to a built cruise executable' >&2; return 1; }
  export HOME="$BATS_TEST_TMPDIR/home"
  export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
  export XDG_DATA_HOME="$BATS_TEST_TMPDIR/data"
  export XDG_STATE_HOME="$BATS_TEST_TMPDIR/state"
  export CRUISE_DISABLE_NOTIFICATIONS=1 CRUISE_DISABLE_HERDR=1 CRUISE_SKIP_AGENT_AUTH_CHECK=1 CRUISE_UI_LANGUAGE=en NO_COLOR=1
  export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_COUNT=0
  unset CRUISE_FORGE HERDR_ENV
  mkdir -p "$HOME" "$BATS_TEST_TMPDIR/bin" "$BATS_TEST_TMPDIR/repo" "$XDG_CONFIG_HOME"
  export REAL_GIT="$(command -v git)"
  export PATH="$BATS_TEST_TMPDIR/bin:$PATH"
  cat > "$BATS_TEST_TMPDIR/bin/git" <<'EOF'
#!/bin/sh
for arg in "$@"; do
  case "$arg" in
    fetch|pull|push|clone|ls-remote|remote-https|remote-http|remote-ssh)
      echo "network-capable git command forbidden in this test: $*" >&2; exit 1 ;;
  esac
done
exec "$REAL_GIT" "$@"
EOF
  chmod +x "$BATS_TEST_TMPDIR/bin/git"
  export GH_LOG="$BATS_TEST_TMPDIR/gh.log"
  cat > "$BATS_TEST_TMPDIR/bin/gh" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
case "$*" in
  --version) echo 'gh version 2.103.0' ;;
  *graphql*) echo '{"data":{"repository":{"defaultBranchRef":{"name":"main","target":{"oid":"abcdef1234567890abcdef1234567890abcdef12","statusCheckRollup":{"state":"FAILURE"}}}}}}' ;;
  *) echo "unexpected gh invocation: $*" >&2; exit 1 ;;
esac
EOF
  chmod +x "$BATS_TEST_TMPDIR/bin/gh"
  git -C "$BATS_TEST_TMPDIR/repo" init -q -b main
  git -C "$BATS_TEST_TMPDIR/repo" remote add origin https://github.com/owner/repo.git
  cd "$BATS_TEST_TMPDIR/repo"

  session=20261112000000001
  session_dir="$XDG_DATA_HOME/cruise/sessions/$session"
  mkdir -p "$session_dir"
  jq -n --arg id "$session" --arg base "$PWD" '{id:$id, base_dir:$base, phase:"Planned", config:{kind:"builtin_snapshot"}, input:"", current_step:null, created_at:"2026-11-12T00:00:00Z", completed_at:null, worktree_path:null, worktree_branch:null, input_as_plan:true}' > "$session_dir/state.json"
  cat > "$session_dir/config.yaml" <<'EOF'
command: [cat]
steps:
  test:
    prompt: do not execute
EOF
  echo '# Test plan' > "$session_dir/plan.md"
}

@test "failing base CI rejects a fresh run before state or workspace mutation" {
  state="$session_dir/state.json"
  cp "$state" "$BATS_TEST_TMPDIR/state.before"
  git worktree list --porcelain > "$BATS_TEST_TMPDIR/worktrees.before"

  run "$CRUISE_BIN" run "$session" </dev/null
  [ "$status" -ne 0 ] || { echo "expected rejection: $output"; return 1; }
  [[ "$output" == *'CI on base branch'* && "$output" == *'main'* && "$output" == *'failing'* && "$output" == *'check_base_ci: false'* ]] || { echo "$output"; return 1; }
  grep -F 'graphql' "$GH_LOG" >/dev/null
  cmp "$BATS_TEST_TMPDIR/state.before" "$state"
  jq -e '.phase == "Planned" and .worktree_path == null' "$state" >/dev/null
  [ ! -e "$XDG_DATA_HOME/cruise/worktrees/$session" ]
  [ ! -e "$XDG_DATA_HOME/cruise/clones/$session" ]
  git worktree list --porcelain > "$BATS_TEST_TMPDIR/worktrees.after"
  cmp "$BATS_TEST_TMPDIR/worktrees.before" "$BATS_TEST_TMPDIR/worktrees.after"
}
