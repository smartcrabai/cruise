#!/usr/bin/env bash
# Local end-to-end coverage for the action boundary: real cruise-runtime,
# job-scoped XDG state, and a local fake GitHub CLI.
action_run_e2e() {
  local runtime_bin target_dir real_sleep
  local action_counter=0
  local e2e_root="$TMP/action-run-e2e"
  local bin_dir="$e2e_root/bin"

  mkdir -p "$bin_dir"
  if [ -n "${CRUISE_ACTION_TEST_BIN:-}" ]; then
    runtime_bin="$CRUISE_ACTION_TEST_BIN"
  else
    if ! cargo build --quiet --bin cruise-runtime; then
      fail "action e2e: cruise-runtime builds" "cargo build failed"
      return
    fi
    target_dir="$(cargo metadata --no-deps --format-version 1 | jq -r '.target_directory')"
    runtime_bin="$target_dir/debug/cruise-runtime"
  fi
  case "$runtime_bin" in /*) ;; *) runtime_bin="$REPO_ROOT/$runtime_bin" ;; esac
  if [ ! -x "$runtime_bin" ]; then
    fail "action e2e: cruise-runtime is executable" "$runtime_bin"
    return
  fi
  real_sleep="$(command -v sleep)"
  export ACTION_E2E_RUNTIME_BIN="$runtime_bin" ACTION_E2E_REAL_SLEEP="$real_sleep"

  cat > "$bin_dir/cruise" <<'SH'
#!/usr/bin/env bash
printf 'state=%s args=%s\n' "${XDG_STATE_HOME:-}" "$*" >> "${ACTION_E2E_CRUISE_LOG:?}"
exec "$ACTION_E2E_RUNTIME_BIN" "$@"
SH
  cat > "$bin_dir/gh" <<'SH'
#!/usr/bin/env bash
set -uo pipefail
printf '%s\n' "$*" >> "${ACTION_E2E_GH_LOG:?}"
if [ "${1:-}" = --version ]; then
  printf 'gh version 2.0.0\n'
  exit 0
fi
if [ "${1:-}" = pr ] && [ "${2:-}" = create ]; then
  printf '%s\n' "${ACTION_E2E_PR_URL:-https://github.com/owner/repo/pull/1}"
  exit 0
fi
if [ "${1:-}" != api ]; then
  if [ "${1:-}" = pr ] && [ "${2:-}" = view ]; then
    printf '{"url":"%s"}\n' "${ACTION_E2E_PR_URL:-https://github.com/owner/repo/pull/1}"
    exit 0
  fi
  echo "fake gh: unhandled command: $*" >&2
  exit 2
fi
shift
endpoint=""
for arg in "$@"; do
  case "$arg" in repos/*|user) endpoint="$arg" ;; esac
done
case "$endpoint" in
  *"/comments?since="*)
    if [ "${ACTION_E2E_TELL_ENABLED:-false}" = true ] && [ ! -e "$ACTION_E2E_TELL_POLLED" ]; then
      : > "$ACTION_E2E_TELL_POLLED"
      if [ -f "${ACTION_E2E_TELL_COMMENTS:-}" ]; then cat "$ACTION_E2E_TELL_COMMENTS"; else printf '[]'; fi
    else
      printf '[]'
    fi
    ;;
  *"/comments?per_page="*)
    if [ "${ACTION_E2E_COMMENTS_FAIL:-false}" = true ]; then
      echo "simulated comments request failure" >&2
      exit 1
    fi
    if [ -f "${ACTION_E2E_COMMENTS:-}" ]; then cat "$ACTION_E2E_COMMENTS"; else printf '[]'; fi
    ;;
  */collaborators/*/permission) printf 'write\n' ;;
  user) printf 'e2e-bot\n' ;;
  */issues/comments/*) printf '{"id":77,"html_url":"%s"}\n' "${ACTION_E2E_PATCH_URL:-https://github.com/owner/repo/issues/42#issuecomment-77}" ;;
  */issues/*/comments) printf '{"id":900,"html_url":"%s"}\n' "${ACTION_E2E_COMMENT_URL:-https://github.com/owner/repo/issues/42#issuecomment-900}" ;;
  */issues/*) printf '{"title":"Action E2E","body":"Implement the action fixture."}\n' ;;
  *) printf '[]' ;;
esac
SH
  cat > "$bin_dir/sleep" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = 15 ] && [ "$#" -eq 1 ]; then
  exec "$ACTION_E2E_REAL_SLEEP" 0.1
fi
exec "$ACTION_E2E_REAL_SLEEP" "$@"
SH
  chmod +x "$bin_dir/cruise" "$bin_dir/gh" "$bin_dir/sleep"
  export PATH="$bin_dir:$PATH"

  action_e2e_case() { # $1=mode $2=issue comment body $3=step command
    local mode="$1" trigger="$2" step_command="$3" repo origin expected_command
    local exec_config expected_action_dir event_file comments_file
    local action_dir_actual config_home data_home state_home jcode_home
    action_counter=$((action_counter + 1))
    ACTION_E2E_CASE="$e2e_root/$mode-$action_counter"
    repo="$ACTION_E2E_CASE/repo"
    origin="$ACTION_E2E_CASE/origin.git"
    mkdir -p "$repo" "$ACTION_E2E_CASE/home"
    export RUNNER_TEMP="$ACTION_E2E_CASE/runner"
    mkdir -p "$RUNNER_TEMP"
    export HOME="$ACTION_E2E_CASE/home" USERPROFILE="$ACTION_E2E_CASE/home"
    export GITHUB_RUN_ID="action-e2e-$action_counter" GITHUB_RUN_ATTEMPT=1 GITHUB_JOB=action-run-e2e
    export GITHUB_REPOSITORY=owner/repo GITHUB_SERVER_URL=https://github.com GITHUB_WORKSPACE="$repo"
    export GITHUB_EVENT_NAME=issue_comment GITHUB_ACTION_PATH="$REPO_ROOT"
    export GH_TOKEN=e2e-token GITHUB_TOKEN=e2e-token TRIGGER_PHRASE=@cruise ALLOWED_BOTS=
    export GITHUB_ENV="$ACTION_E2E_CASE/github-env" GITHUB_OUTPUT="$ACTION_E2E_CASE/gate-output"
    export ACTION_E2E_GH_LOG="$ACTION_E2E_CASE/gh.log" ACTION_E2E_CRUISE_LOG="$ACTION_E2E_CASE/cruise.log"
    export ACTION_E2E_PR_URL="https://github.com/owner/repo/pull/$action_counter"
    export ACTION_E2E_COMMENT_URL="https://github.com/owner/repo/issues/42#issuecomment-$((900 + action_counter))"
    export ACTION_E2E_PATCH_URL="https://github.com/owner/repo/issues/42#issuecomment-77"
    export ACTION_E2E_TELL_ENABLED=false ACTION_E2E_TELL_POLLED="$ACTION_E2E_CASE/tell-polled"
    export ACTION_E2E_TELL_COMMENTS="$ACTION_E2E_CASE/tell-comments.json"
    export ACTION_E2E_COMMENTS="$ACTION_E2E_CASE/comments.json" ACTION_E2E_COMMENTS_FAIL=false
    unset CRUISE_ACTION_DIR XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME CRUISE_CONFIG
    unset CRUISE_MODEL CRUISE_PLAN_MODEL CRUISE_SDK CRUISE_FORCE_EXEC CRUISE_RUNTIME
    : > "$GITHUB_ENV"
    : > "$GITHUB_OUTPUT"
    : > "$ACTION_E2E_GH_LOG"
    : > "$ACTION_E2E_CRUISE_LOG"
    rm -f "$ACTION_E2E_TELL_POLLED"
    printf '[]' > "$ACTION_E2E_COMMENTS"
    export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_COUNT=0
    export CRUISE_DISABLE_NOTIFICATIONS=1 NO_COLOR=1

    must git init -q --bare "$origin"
    must git init -q -b main "$repo"
    must git -C "$repo" config user.name Fixture
    must git -C "$repo" config user.email fixture@example.test
    printf 'fixture\n' > "$repo/README.md"
    must git -C "$repo" add README.md
    must git -C "$repo" commit -q -m 'initial fixture'
    must git -C "$repo" remote add origin "$origin"
    must git -C "$repo" push -q -u origin main
    must git -C "$origin" symbolic-ref HEAD refs/heads/main
    mkdir -p "$repo/.cruise"
    printf 'check_base_ci: false\n' > "$repo/.cruise/default.yaml"
    cat > "$repo/cruise.yaml" <<YAML
command:
  - sh
  - -c
  - 'cat >/dev/null; if [ -z "\$PLAN_FILE" ]; then printf "%s\n" "---" "title: Action E2E PR" "---" "Automated fixture PR."; else sleep 1; printf "%s" "\$ACTION_E2E_PLAN_CONTENT" > "\$PLAN_FILE"; fi'
env:
  PLAN_FILE: "{plan}"
  ACTION_E2E_PLAN_CONTENT: "# Action E2E Plan\n\nImplement the fixture."
max_retries: 0
steps:
  implement:
    command: "$step_command"
YAML
    event_file="$ACTION_E2E_CASE/event.json"
    python3 -c 'import json,sys; print(json.dumps({"action":"created","issue":{"number":42},"comment":{"id":100,"body":sys.argv[1],"user":{"login":"alice","id":7,"type":"User"}}}))' "$trigger" > "$event_file"
    export GITHUB_EVENT_PATH="$event_file"

    if [ "$mode" = fix ]; then
      comments_file="$ACTION_E2E_CASE/comments.json"
      # shellcheck disable=SC2016
      printf '%s\n' '[{"id":77,"body":"<!-- cruise:plan -->\n## 📋 Plan\n\nPrevious plan.\n\n---\n_Reply `@cruise fix <feedback>` to revise, or `@cruise run` to execute this plan._","user":{"login":"github-actions[bot]","type":"Bot"}}]' > "$comments_file"
    fi
    if [ "$mode" = failure ]; then ACTION_E2E_COMMENTS_FAIL=true; fi
    if [ "$mode" = run ]; then
      export ACTION_E2E_TELL_ENABLED=true
      printf '%s\n' '[{"id":501,"body":"@cruise /tell also record the tell","user":{"login":"alice","type":"User"}}]' > "$ACTION_E2E_TELL_COMMENTS"
    fi

    if ! ANTHROPIC_API_KEY_INPUT=e2e-only OPENAI_API_KEY_INPUT='' ENV_INPUT='' PROVIDERS_INPUT='' PROVIDER_API_KEYS_INPUT='' \
      bash "$REPO_ROOT/action/scripts/gate.sh" >"$ACTION_E2E_CASE/gate.log" 2>&1; then
      fail "action e2e $mode: gate accepts the authorized command" "$(cat "$ACTION_E2E_CASE/gate.log")"
      return
    fi
    expected_command="$mode"
    case "$mode" in failure|finalize) expected_command=run ;; esac
    if [ "$(sed -n 's/^proceed=//p' "$GITHUB_OUTPUT" | tail -n1)" != true ] \
      || [ "$(sed -n 's/^command=//p' "$GITHUB_OUTPUT" | tail -n1)" != "$expected_command" ]; then
      fail "action e2e $mode: gate parses and authorizes the command" "$(cat "$GITHUB_OUTPUT")"
      return
    fi
    pass "action e2e $mode: gate parses and authorizes the command"
    COMMAND="$(sed -n 's/^command=//p' "$GITHUB_OUTPUT" | tail -n1)"
    ENTITY_NUMBER=42
    COMMAND_REST_FILE="$(sed -n 's/^command_rest_file=//p' "$GITHUB_OUTPUT" | tail -n1)"
    export COMMAND ENTITY_NUMBER COMMAND_REST_FILE

    export GITHUB_OUTPUT="$ACTION_E2E_CASE/setup-output"
    : > "$GITHUB_OUTPUT"
    if ! MODEL_INPUT='' PLAN_MODEL_INPUT='' ENV_INPUT='' bash "$REPO_ROOT/action/scripts/setup-env.sh" >"$ACTION_E2E_CASE/setup.log" 2>&1; then
      fail "action e2e $mode: setup-env succeeds" "$(cat "$ACTION_E2E_CASE/setup.log")"
      return
    fi
    while IFS='=' read -r name value; do
      [ -n "$name" ] && export "$name=$value"
    done < "$GITHUB_ENV"
    expected_action_dir="$RUNNER_TEMP/cruise-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT-$GITHUB_JOB"
    action_dir_actual="${CRUISE_ACTION_DIR:-}"
    config_home="${XDG_CONFIG_HOME:-}"
    data_home="${XDG_DATA_HOME:-}"
    state_home="${XDG_STATE_HOME:-}"
    jcode_home="${JCODE_HOME:-}"
    export ACTION_E2E_STATE_HOME="$state_home" ACTION_E2E_DATA_HOME="$data_home"
    if [ "$action_dir_actual" = "$expected_action_dir" ] \
      && [ "$config_home" = "$expected_action_dir/xdg-config" ] \
      && [ "$data_home" = "$expected_action_dir/data" ] \
      && [ "$state_home" = "$expected_action_dir/xdg-state" ] \
      && [ "$jcode_home" = "$expected_action_dir/jcode-home" ]; then
      pass "action e2e $mode: XDG and jcode homes are job-scoped under RUNNER_TEMP"
    else
      fail "action e2e $mode: XDG and jcode homes are job-scoped under RUNNER_TEMP" "$(cat "$GITHUB_ENV")"
    fi

    export GITHUB_OUTPUT="$ACTION_E2E_CASE/config-output"
    : > "$GITHUB_OUTPUT"
    if ! CONFIG_INPUT='' bash "$REPO_ROOT/action/scripts/resolve-config.sh" >"$ACTION_E2E_CASE/config.log" 2>&1; then
      fail "action e2e $mode: config resolution succeeds" "$(cat "$ACTION_E2E_CASE/config.log")"
      return
    fi
    exec_config="$(sed -n 's/^exec_config_path=//p' "$GITHUB_OUTPUT" | tail -n1)"
    if [ -z "$exec_config" ] || [ ! -f "$exec_config" ]; then
      fail "action e2e $mode: exec config is generated under the job directory" "exec config path: $exec_config"
      return
    fi
    pass "action e2e $mode: config resolution succeeds under the job directory"
    while IFS='=' read -r name value; do
      [ -n "$name" ] && export "$name=$value"
    done < "$GITHUB_ENV"

    if [ "$mode" = exec ]; then
      # The action's exec config is intentionally backend-neutral. Use a
      # local command backend in the generated config slot to avoid an LLM.
      cat > "$exec_config" <<'YAML'
command:
  - sh
  - -c
  - 'cat >/dev/null; sleep 1; printf "exec complete" > exec-result.txt'
steps:
  execute:
    prompt: "{input}"
YAML
      export EXEC_CONFIG_PATH="$exec_config"
    else
      export EXEC_CONFIG_PATH="$exec_config"
    fi
    export GIT_USER_NAME_INPUT='' GIT_USER_EMAIL_INPUT='' FLAGS_INPUT='' USED_APP=false
    export POLL_SINCE=2026-01-01T00:00:00Z
    export GITHUB_OUTPUT="$ACTION_E2E_CASE/run-output"
    : > "$GITHUB_OUTPUT"
    (
      cd "$repo" || exit 1
      bash "$REPO_ROOT/action/scripts/run.sh"
    ) >"$ACTION_E2E_CASE/run.log" 2>&1 &
    local action_pid=$! daemon_pid="" state_record="$ACTION_E2E_STATE_HOME/cruise/daemon/instance.json"
    local loops=0 action_stat
    while [ "$loops" -lt 400 ]; do
      if [ -f "$state_record" ]; then
        daemon_pid="$(jq -r '.pid // empty' "$state_record" 2>/dev/null)"
        [ -n "$daemon_pid" ] && break
      fi
      action_stat="$(ps -p "$action_pid" -o stat= 2>/dev/null || true)"
      case "$action_stat" in ''|*Z*) break ;; esac
      "$ACTION_E2E_REAL_SLEEP" 0.05
      loops=$((loops + 1))
    done
    local run_status=0
    wait "$action_pid" || run_status=$?
    if [ "$mode" = failure ]; then
      if [ -z "$daemon_pid" ] && [ ! -e "$state_record" ]; then
        pass "action e2e failure: early error leaves no daemon behind"
      else
        fail "action e2e failure: early error leaves no daemon behind" "pid=$daemon_pid record=$(cat "$state_record" 2>/dev/null || true)"
      fi
    elif [ -n "$daemon_pid" ]; then
      local stopped=false stop_loops=0
      while [ "$stop_loops" -lt 100 ]; do
        if ! kill -0 "$daemon_pid" 2>/dev/null; then stopped=true; break; fi
        "$ACTION_E2E_REAL_SLEEP" 0.05
        stop_loops=$((stop_loops + 1))
      done
      if [ "$stopped" = true ] && [ ! -e "$state_record" ]; then
        pass "action e2e $mode: run.sh stops its job daemon on exit"
      else
        fail "action e2e $mode: run.sh stops its job daemon on exit" "daemon pid $daemon_pid remains; registry: $(cat "$state_record" 2>/dev/null || true)"
      fi
    else
      fail "action e2e $mode: a daemon was created for the job-scoped state directory" "$(cat "$ACTION_E2E_CASE/run.log")"
    fi
    export ACTION_E2E_RUN_OUTPUT="$GITHUB_OUTPUT"
    export ACTION_E2E_RUN_LOG="$ACTION_E2E_CASE/run.log"
    export ACTION_E2E_CASE_DIR="$ACTION_E2E_CASE"
    export ACTION_E2E_REPO="$repo" ACTION_E2E_ORIGIN="$origin"
    export ACTION_E2E_EXPECTED_PR_URL="$ACTION_E2E_PR_URL"
    action_e2e_assert_run "$mode" "$run_status"

    if [ "$mode" = run ]; then
      "$ACTION_E2E_REAL_SLEEP" 0.2
      if grep -Fq 'args=tell ' "$ACTION_E2E_CRUISE_LOG" \
        && grep -F "state=$ACTION_E2E_STATE_HOME args=tell " "$ACTION_E2E_CRUISE_LOG" >/dev/null; then
        pass "action e2e run: tell-poller delivers through the same job daemon"
      else
        fail "action e2e run: tell-poller delivers through the same job daemon" "$(cat "$ACTION_E2E_CRUISE_LOG")"
      fi
      poller_pattern="$REPO_ROOT/action/scripts/tell-poller.sh watch"
      if ! pgrep -f "$poller_pattern" >/dev/null; then
        pass "action e2e run: tell-poller is stopped before run.sh exits"
      else
        fail "action e2e run: tell-poller is stopped before run.sh exits" "$(pgrep -fl "$poller_pattern" || true)"
      fi
    fi

    export GITHUB_OUTPUT="$ACTION_E2E_CASE/finalize-output"
    : > "$GITHUB_OUTPUT"
    PROCEED=true RUN_OUTCOME="$([ "$run_status" -eq 0 ] && echo success || echo failure)" \
      COMMAND="$COMMAND" SESSION_ID="$(sed -n 's/^session_id=//p' "$ACTION_E2E_RUN_OUTPUT" | tail -n1)" \
      COMMENT_ID='' START_TS='' GITHUB_REPOSITORY=owner/repo \
      bash "$REPO_ROOT/action/scripts/finalize.sh" >"$ACTION_E2E_CASE/finalize.log" 2>&1
    local expected_conclusion=success
    [ "$run_status" -eq 0 ] || expected_conclusion=failure
    assert_eq "action e2e $mode: finalize reports the run conclusion" "$expected_conclusion" \
      "$(sed -n 's/^conclusion=//p' "$GITHUB_OUTPUT" | tail -n1)"
  }

  action_e2e_check_nonempty() {
    if [ -n "$2" ]; then pass "$1"; else fail "$1" "${3:-}"; fi
  }

  action_e2e_check_file() {
    if [ -f "$2" ]; then pass "$1"; else fail "$1" "$2"; fi
  }
  action_e2e_assert_run() { # $1=mode $2=exit status
    local mode="$1" status="$2" output="$ACTION_E2E_RUN_OUTPUT" log="$ACTION_E2E_RUN_LOG"
    local plan_url pr_url session_id state_file plan_file
    if [ "$mode" = failure ]; then
      assert_status "action e2e failure: emit_and_exit returns failure" 1 "$status" "$(cat "$log")"
      action_e2e_check_nonempty "action e2e failure: fail_reason is emitted" \
        "$(sed -n 's/^fail_reason=//p' "$output" | tail -n1)" "$(cat "$output")"
      return
    fi
    assert_status "action e2e $mode: emit_and_exit returns success" 0 "$status" "$(cat "$log")"
    case "$mode" in
      run)
        session_id="$(sed -n 's/^session_id=//p' "$output" | tail -n1)"
        state_file="$ACTION_E2E_DATA_HOME/cruise/sessions/$session_id/state.json"
        action_e2e_check_nonempty "action e2e run: created session id is emitted" "$session_id" "$(cat "$output")"
        assert_eq "action e2e run: daemon completed the session" Completed "$(jq -r '.phase // empty' "$state_file" 2>/dev/null)"
        assert_contains "action e2e run: runtime executed the session through the daemon" \
          "$(cat "$ACTION_E2E_CRUISE_LOG")" "args=run $session_id"
        pr_url="$(sed -n 's/^pr_url=//p' "$output" | tail -n1)"
        if [ "$pr_url" = "$ACTION_E2E_EXPECTED_PR_URL" ]; then
          pass "action e2e run: PR URL is emitted"
        else
          fail "action e2e run: PR URL is emitted" "expected '$ACTION_E2E_EXPECTED_PR_URL', got '$pr_url'; run log: $(cat "$log"); gh: $(cat "$ACTION_E2E_GH_LOG")"
        fi
        if grep -Fq 'pr create' "$ACTION_E2E_GH_LOG"; then
          pass "action e2e run: Cruise asked gh to create the PR"
        else
          fail "action e2e run: Cruise asked gh to create the PR" \
            "run log: $(cat "$log"); output: $(cat "$output"); gh: $(cat "$ACTION_E2E_GH_LOG"); cruise: $(cat "$ACTION_E2E_CRUISE_LOG")"
        fi
        ;;
      exec)
        action_e2e_check_nonempty "action e2e exec: commit URL is emitted" \
          "$(sed -n 's/^commit_url=//p' "$output" | tail -n1)" "$(cat "$output")"
        action_e2e_check_file "action e2e exec: command backend executed through cruise" "$ACTION_E2E_REPO/exec-result.txt"
        assert_contains "action e2e exec: Cruise executed the workflow through the daemon" \
          "$(cat "$ACTION_E2E_CRUISE_LOG")" "args=exec "
        assert_eq "action e2e exec: pushed commit reaches local origin" \
          "$(git -C "$ACTION_E2E_REPO" rev-parse HEAD)" "$(git -C "$ACTION_E2E_ORIGIN" rev-parse main)"
        ;;
      plan|fix)
        session_id="$(sed -n 's/^session_id=//p' "$output" | tail -n1)"
        state_file="$ACTION_E2E_DATA_HOME/cruise/sessions/$session_id/state.json"
        plan_file="$ACTION_E2E_DATA_HOME/cruise/sessions/$session_id/plan.md"
        action_e2e_check_nonempty "action e2e $mode: created session id is emitted" "$session_id" "$(cat "$output")"
        action_e2e_check_file "action e2e $mode: daemon persisted the planning session" "$state_file"
        assert_contains "action e2e $mode: command backend wrote the plan" "$(cat "$plan_file" 2>/dev/null)" "Action E2E Plan"
        if [ "$mode" = fix ]; then plan_url="$ACTION_E2E_PATCH_URL"; else plan_url="$ACTION_E2E_COMMENT_URL"; fi
        assert_eq "action e2e $mode: plan comment URL is emitted" "$plan_url" \
          "$(sed -n 's/^plan_comment_url=//p' "$output" | tail -n1)"
        assert_contains "action e2e $mode: cruise created a persisted session" \
          "$(cat "$ACTION_E2E_CRUISE_LOG")" "args=plan"
        ;;
    esac
    if [ -n "${CRUISE_ACTION_DIR:-}" ]; then
      assert_contains "action e2e $mode: runtime calls use the job-scoped state directory" \
        "$(cat "$ACTION_E2E_CRUISE_LOG")" "state=$ACTION_E2E_STATE_HOME"
    fi
  }

  action_e2e_case run '@cruise run' 'sleep 2; printf run-complete >> README.md'
  action_e2e_case exec '@cruise exec' 'true'
  action_e2e_case plan '@cruise plan' 'true'
  action_e2e_case fix '@cruise fix revise the plan' 'true'
  action_e2e_case failure '@cruise run' 'true'

  # The run-script trap is the primary lifecycle owner. Also exercise the
  # composite finalizer's cleanup when an earlier step left a daemon running.
  action_e2e_case finalize '@cruise run' 'sleep 1; true'
  local finalize_record="$ACTION_E2E_STATE_HOME/cruise/daemon/instance.json" finalize_pid=""
  if "$ACTION_E2E_RUNTIME_BIN" daemon start --port 0 >"$ACTION_E2E_CASE_DIR/pre-finalize-start.log" 2>&1; then
    finalize_pid="$(jq -r '.pid // empty' "$finalize_record" 2>/dev/null)"
    : > "$ACTION_E2E_CASE_DIR/finalize-output"
    GITHUB_OUTPUT="$ACTION_E2E_CASE_DIR/finalize-output" PROCEED=true RUN_OUTCOME=success \
      COMMAND=run COMMENT_ID='' START_TS='' SESSION_ID='' GITHUB_REPOSITORY=owner/repo \
      bash "$REPO_ROOT/action/scripts/finalize.sh" >"$ACTION_E2E_CASE_DIR/finalize-only.log" 2>&1
    local finalize_stopped=false finalize_loops=0
    while [ -n "$finalize_pid" ] && [ "$finalize_loops" -lt 100 ]; do
      if ! kill -0 "$finalize_pid" 2>/dev/null; then finalize_stopped=true; break; fi
      "$ACTION_E2E_REAL_SLEEP" 0.05
      finalize_loops=$((finalize_loops + 1))
    done
    if [ "$finalize_stopped" = true ] && [ ! -e "$finalize_record" ]; then
      pass "action e2e finalize: force-stops a daemon left by an earlier step"
    else
      fail "action e2e finalize: force-stops a daemon left by an earlier step" "pid=$finalize_pid record=$(cat "$finalize_record" 2>/dev/null || true)"
    fi
  else
    fail "action e2e finalize: starts a fixture daemon" "$(cat "$ACTION_E2E_CASE_DIR/pre-finalize-start.log")"
  fi
}
