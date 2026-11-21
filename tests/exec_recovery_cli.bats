#!/usr/bin/env bats

# CRUISE_BIN must point to the built cruise executable. Requires jq, curl and Expect.
setup() {
  [[ -x "${CRUISE_BIN:-}" ]] || { echo 'Set CRUISE_BIN to the built cruise executable' >&2; return 1; }
  CRUISE_BIN="$(cd "$(dirname "$CRUISE_BIN")" && pwd)/$(basename "$CRUISE_BIN")"
  export CRUISE_BIN
  while IFS= read -r key; do
    [[ "$key" == CRUISE_BIN ]] || unset "$key"
  done < <(compgen -e CRUISE_)
  export HOME="$BATS_TEST_TMPDIR/home"
  export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
  export XDG_DATA_HOME="$BATS_TEST_TMPDIR/data"
  export XDG_STATE_HOME="$BATS_TEST_TMPDIR/state"
  export XDG_CACHE_HOME="$BATS_TEST_TMPDIR/cache"
  export CRUISE_DISABLE_NOTIFICATIONS=1 CRUISE_DISABLE_HERDR=1 CRUISE_UI_LANGUAGE=en NO_COLOR=1
  export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_COUNT=0
  unset HERDR_ENV
  mkdir -p "$HOME" "$BATS_TEST_TMPDIR/repo"
  cd "$BATS_TEST_TMPDIR/repo"
  git init -q -b main
  git -c user.name=Test -c user.email=test@example.test -c commit.gpgsign=false \
    -c core.hooksPath=/dev/null commit --allow-empty -qm fixture
  printf 'dirty work\n' > dirty.txt
  original_head="$(git rev-parse HEAD)"
  sessions="$XDG_DATA_HOME/cruise/sessions"
  workflow="$BATS_TEST_TMPDIR/workflow.yaml"
  "$CRUISE_BIN" daemon start --port 0
}

teardown() {
  if [[ -n "${exec_pid:-}" ]]; then
    touch release
    kill "$exec_pid" 2>/dev/null || true
    wait "$exec_pid" 2>/dev/null || true
  fi
  if [[ -n "${sessions:-}" ]]; then
    "$CRUISE_BIN" daemon stop --force >/dev/null 2>&1 || true
  fi
}

recoverable_workflow() {
  cat > "$workflow" <<'YAML'
command: [echo]
steps:
  prefix:
    command: printf P >> visits
  repair:
    command: printf R >> visits; printf '%s\n' '{prev.success}' >> prior-success; test -f repaired
  tail:
    command: printf T >> visits; test -f repaired
YAML
}

start_failure() {
  run "$CRUISE_BIN" exec --config "$workflow" "$@" </dev/null
  [[ "$status" -ne 0 ]] || { echo "expected exec failure: $output"; return 1; }
  failure_output="$output"
  run "$CRUISE_BIN" list --json
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  id="$(jq -er 'select(length == 1) | .[0] | select(.phase == "Failed" and .exec == true and .plan_available == false) | .id' <<< "$output")" || {
    echo "expected one retained planless failed exec: $output (exec: $failure_output)"; return 1;
  }
  [[ "$failure_output" == *"cruise run $id"* && "$failure_output" == *'cruise list'* ]]
}

assert_work_preserved() {
  [[ "$(cat dirty.txt)" == 'dirty work' ]]
  [[ "$(git branch --show-current)" == main ]]
  [[ "$(git rev-parse HEAD)" == "$original_head" ]]
  [[ "$(git worktree list --porcelain | grep -c '^worktree ')" -eq 1 ]]
}

# bats test_tags=recovery-resume-by-id
@test "failed exec continues, survives clean and restart, and retries from the first unresolved step by ID" {
  recoverable_workflow
  start_failure
  [[ "$(cat visits)" == PRT ]]
  [[ -s "$sessions/$id/run.log" && -s "$sessions/$id/dag.json" ]]

  # No restart or delay before the first explicit retry: the terminal result must release the claim.
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -ne 0 && "$output" == *"cruise run $id"* ]]
  [[ "$(cat visits)" == PRTRT ]]
  run "$CRUISE_BIN" clean </dev/null
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  "$CRUISE_BIN" daemon stop
  "$CRUISE_BIN" daemon start --port 0
  run "$CRUISE_BIN" list --json
  [[ "$status" -eq 0 ]]
  jq -e --arg id "$id" 'length == 1 and .[0].id == $id and .[0].phase == "Failed"' <<< "$output"

  run "$CRUISE_BIN" run </dev/null
  [[ "$status" -ne 0 && "$output" == *'No pending sessions'* ]]
  run "$CRUISE_BIN" run --all </dev/null
  [[ "$status" -eq 0 && "$output" != *"$id"* && "$output" != *'Run All Summary'* ]]
  [[ "$(cat visits)" == PRTRT ]]

  touch repaired
  run "$CRUISE_BIN" run "$id" --dry-run </dev/null
  [[ "$status" -eq 0 && "$output" == *'repair [command]'* && "$output" != *'prefix [command]'* ]]
  [[ "$(cat visits)" == PRTRT ]]
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$output" == *'again'* && "$output" == *'rolled back'* ]]
  [[ "$(cat visits)" == PRTRTRT ]]
  [[ "$(cat prior-success)" == $'true\ntrue\ntrue' ]]
  [[ ! -e "$sessions/$id" ]]
  assert_work_preserved
}

# bats test_tags=recovery-menu-resume
@test "list offers Resume and Delete for failed exec and dispatches Resume through the real menu" {
  recoverable_workflow
  start_failure
  touch repaired
  cp "$sessions/$id/state.json" exec-state
  jq '.exec = false' exec-state > "$sessions/$id/state.json"
  printf '# Ordinary plan\n' > "$sessions/$id/plan.md"
  run env TERM=xterm RECOVERY_ACTION=Ordinary expect "$BATS_TEST_DIRNAME/exec_recovery_pty.expect"
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  cp exec-state "$sessions/$id/state.json"
  : > "$sessions/$id/plan.md"
  run env TERM=xterm RECOVERY_ACTION=Resume expect "$BATS_TEST_DIRNAME/exec_recovery_pty.expect"
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$(cat visits)" == PRTRT && ! -e "$sessions/$id" ]]
  assert_work_preserved
}

# bats test_tags=recovery-menu-delete
@test "list can Delete immediately after failure without touching the working files" {
  recoverable_workflow
  start_failure
  run env TERM=xterm RECOVERY_ACTION=Delete expect "$BATS_TEST_DIRNAME/exec_recovery_pty.expect"
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$(cat visits)" == PRT && ! -e "$sessions/$id" ]]
  assert_work_preserved
}

# bats test_tags=recovery-artifact-retention
@test "clean preserves failed exec artifacts for retry and success removes them" {
  cat > "$workflow" <<'YAML'
command: [cat]
steps:
  save:
    prompt: retained-evidence
    output_file: evidence.md
  repair:
    command: test -f repaired
  read:
    prompt: 'recovered: {file:evidence.md}'
YAML
  start_failure
  artifact="$sessions/$id/artifacts/evidence.md"
  [[ "$(cat "$artifact")" == retained-evidence ]]
  run "$CRUISE_BIN" clean </dev/null
  [[ "$status" -eq 0 && "$(cat "$artifact")" == retained-evidence ]]
  touch repaired
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -eq 0 && "$output" == *'recovered: retained-evidence'* && ! -e "$sessions/$id" ]]
}

# bats test_tags=recovery-damaged-checkpoint
@test "corrupt or missing started checkpoints reject preview and live recovery without new effects" {
  cat > "$workflow" <<'YAML'
command: [echo]
groups:
  warmup:
    max_retries: 1
    if:
      file-changed: prepare
    steps:
      write:
        command: printf P >> visits; touch marker
steps:
  prepare:
    command: 'true'
  warmup:
    group: warmup
  repair:
    command: printf R >> visits; test -f repaired
  tail:
    command: printf T >> visits
YAML
  start_failure
  checkpoint="$sessions/$id/dag.json"
  cp "$checkpoint" original-checkpoint
  effects="$(cat visits)"
  jq -e '.state.exec_failure | (.edge_counts | any(.traversals > 0 and .budgeted_traversals == 0)) and (.group_counts | any(. > 0))' original-checkpoint
  touch repaired
  for damage in zero-edge zero-group corrupt missing; do
    case "$damage" in
      zero-edge) jq '(.state.exec_failure.edge_counts | map(select(.budgeted_traversals == 0))[0]) as $edge | (.state.exec_failure.edge_counts[] | select(. == $edge) | .traversals) = 0' original-checkpoint > "$checkpoint" ;;
      zero-group) jq '(.state.exec_failure.group_counts[])=0' original-checkpoint > "$checkpoint" ;;
      corrupt) printf '{invalid checkpoint' > "$checkpoint" ;;
      missing) rm "$checkpoint" ;;
    esac
    if [[ "$damage" != missing ]]; then cp "$checkpoint" damaged-checkpoint; fi
    for mode in preview live; do
      args=(run "$id")
      [[ "$mode" != preview ]] || args+=(--dry-run)
      run "$CRUISE_BIN" "${args[@]}" </dev/null
      [[ "$status" -ne 0 ]] || { echo "accepted $damage checkpoint: $output"; return 1; }
      [[ "$(cat visits)" == "$effects" ]]
      [[ -f "$sessions/$id/state.json" && -s "$sessions/$id/run.log" ]]
      if [[ "$damage" == missing ]]; then
        [[ ! -e "$checkpoint" ]]
      else
        cmp "$checkpoint" damaged-checkpoint
      fi
    done
  done
}

# bats test_tags=recovery-expanded-child-preview
@test "exec recovery preview lists the expanded failed child and following steps" {
  cat > "$workflow" <<'YAML'
command: [echo]
groups:
  checks:
    steps:
      verify:
        command: printf V >> visits; test -f repaired
steps:
  prefix:
    command: printf P >> visits
  checks:
    group: checks
  tail:
    command: printf T >> visits
YAML
  start_failure
  cp "$sessions/$id/dag.json" before-preview
  [[ "$(cat visits)" == PVT ]]
  run "$CRUISE_BIN" run "$id" --dry-run </dev/null
  [[ "$status" -eq 0 && "$output" == *'Exec resume step: checks/verify'* ]]
  steps="${output#*steps:}"
  [[ "$steps" == *'checks/verify [command]'*'tail [command]'* && "$steps" != *'prefix [command]'* ]]
  cmp "$sessions/$id/dag.json" before-preview
  [[ "$(cat visits)" == PVT ]]
}

# bats test_tags=recovery-handled-failure
@test "handled if.fail transfers to recovery without retaining a failed session" {
  cat > "$workflow" <<'YAML'
command: [echo]
steps:
  accepted:
    command: printf A >> visits; exit 75
    accepted_exit_codes: [0, 75]
  preferred:
    command: printf P >> visits; exit 1
    if:
      fail: recover
  skipped:
    command: printf S >> visits
  recover:
    command: printf R >> visits
YAML
  run "$CRUISE_BIN" exec --config "$workflow" </dev/null
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$(cat visits)" == APR ]]
  run "$CRUISE_BIN" list --json
  [[ "$status" -eq 0 ]]
  jq -e 'length == 0' <<< "$output"
  [[ ! -d "$sessions" || -z "$(find "$sessions" -name state.json -print)" ]]
}

# bats test_tags=recovery-wrong-branch
@test "a wrong-branch resume preserves the retry point until the original workspace is restored" {
  recoverable_workflow
  start_failure
  touch repaired
  cp "$sessions/$id/dag.json" "$BATS_TEST_TMPDIR/checkpoint.before"
  git switch -qc wrong-branch
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -ne 0 && "$output" == *main* && "$output" == *wrong-branch* ]]
  [[ "$(cat visits)" == PRT ]]
  cmp "$sessions/$id/dag.json" "$BATS_TEST_TMPDIR/checkpoint.before"
  git switch -q main
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$(cat visits)" == PRTRT && ! -e "$sessions/$id" ]]
}

# bats test_tags=recovery-workspace-setup
@test "an accepted exec that fails workspace setup retries from graph entry" {
  recoverable_workflow
  git switch -q --detach HEAD
  start_failure
  [[ ! -e visits && ! -e "$sessions/$id/dag.json" ]]

  git switch -q main
  touch repaired
  run "$CRUISE_BIN" run "$id" --dry-run </dev/null
  [[ "$status" -eq 0 && "$output" == *'prefix [command]'* && ! -e visits ]]
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$(cat visits)" == PRT && ! -e "$sessions/$id" ]]
  assert_work_preserved
}

# bats test_tags=recovery-saved-budget
@test "preview and live resume use the saved explicit retry budget unless overridden" {
  cat > "$workflow" <<'YAML'
command: [echo]
max_retries: 1
groups:
  group:
    max_retries: 4
    steps:
      work:
        command: printf G >> visits
steps:
  prefix:
    group: group
  repair:
    command: printf R >> visits; test -f repaired
YAML
  start_failure --max-retries 7
  [[ "$(cat visits)" == GR ]]
  touch repaired
  for mode in preview live; do
    args=(run "$id" --max-retries 2)
    [[ "$mode" != preview ]] || args+=(--dry-run)
    run "$CRUISE_BIN" "${args[@]}" </dev/null
    [[ "$status" -ne 0 && "$output" == *max_retries* ]]
    [[ "$(cat visits)" == GR ]]
  done
  run "$CRUISE_BIN" run "$id" --dry-run </dev/null
  [[ "$status" -eq 0 && "$output" == *'repair [command]'* ]]
  [[ "$(cat visits)" == GR ]]
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$(cat visits)" == GRR && ! -e "$sessions/$id" ]]
}

# bats test_tags=recovery-hard-error
@test "hard errors resume from a durable checkpoint unless an unresolved ordinary failure came first" {
  for earlier in false true; do
    rm -f repaired visits
    cat > "$workflow" <<YAML
command: [echo]
steps:
  prefix:
    command: printf P >> visits
  optional:
    command: printf O >> visits; ! $earlier || test -f repaired
  required:
    command: printf H >> visits; test -f repaired
    accepted_exit_codes: [0, 75]
  finish:
    command: printf F >> visits
YAML
    start_failure
    [[ "$(cat visits)" == POH ]]
    touch repaired
    run "$CRUISE_BIN" run "$id" </dev/null
    [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
    if [[ "$earlier" == true ]]; then
      [[ "$(cat visits)" == POHOHF ]]
    else
      [[ "$(cat visits)" == POHHF ]]
    fi
    [[ ! -e "$sessions/$id" ]]
  done
}

# bats test_tags=recovery-pending-transition
@test "recovery does not re-run a rejected pending transition's source or reset its budget" {
  cat > "$workflow" <<'YAML'
command: [echo]
steps:
  retry:
    command: printf R >> visits; test -f repaired
    if:
      fail:
        retry: true
  finish:
    command: printf F >> visits
YAML
  start_failure --max-retries 1
  [[ "$(cat visits)" == RR ]]
  touch repaired
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -ne 0 && "$output" == *"cruise run $id"* ]]
  [[ "$(cat visits)" == RR ]]
  run "$CRUISE_BIN" run "$id" --max-retries 2 </dev/null
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$(cat visits)" == RRRF && ! -e "$sessions/$id" ]]
}

# bats test_tags=recovery-interrupted-terminal-write
@test "a completed checkpoint still retries the unresolved step when the terminal phase write was interrupted" {
  recoverable_workflow
  start_failure
  [[ "$(cat visits)" == PRT ]]
  "$CRUISE_BIN" daemon stop
  # Reproduce the on-disk window after the graph reached its end but before the
  # terminal phase was persisted. Keep the real execution checkpoint intact.
  state="$sessions/$id/state.json"
  jq '.phase = "Running"' "$state" > "$state.interrupted"
  mv "$state.interrupted" "$state"
  "$CRUISE_BIN" daemon start --port 0
  touch repaired
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$output" == *'may run again. Files and external effects are not rolled back.'*'> repair'* ]]
  [[ "$(cat visits)" == PRTRT && ! -e "$sessions/$id" ]]
}

# bats test_tags=recovery-mid-graph-suspension
@test "mid-graph suspension resumes at the live step without forgetting an earlier failure" {
  cat > "$workflow" <<'YAML'
command: [echo]
steps:
  prefix:
    command: printf P >> visits
  repair:
    command: printf R >> visits; test -f repaired
  gate:
    command: touch gate-entered; while [ ! -f release ]; do sleep 0.05; done; printf G >> visits
  tail:
    command: printf T >> visits
YAML
  "$CRUISE_BIN" exec --config "$workflow" </dev/null > "$BATS_TEST_TMPDIR/exec.output" 2>&1 &
  exec_pid=$!
  for ((attempt = 0; attempt < 300; attempt++)); do
    [[ ! -f gate-entered ]] || break
    sleep 0.05
  done
  [[ -f gate-entered ]] || { cat "$BATS_TEST_TMPDIR/exec.output"; return 1; }
  state_files=("$sessions"/*/state.json)
  [[ "${#state_files[@]}" -eq 1 && -f "${state_files[0]}" ]]
  id="$(jq -er .id "${state_files[0]}")"
  run env TERM=xterm RECOVERY_ID="$id" expect -c '
    set timeout 20
    spawn -noecho $env(CRUISE_BIN) run $env(RECOVERY_ID)
    after 500
    send -- "\003"
    expect {
      -exact {session saved as Suspended.} {}
      timeout { exit 1 }
      eof { exit 1 }
    }
    expect eof
  '
  [[ "$status" -eq 0 && "$output" != *'not rolled back'* ]] || { echo "$output"; return 1; }
  for ((attempt = 0; attempt < 300; attempt++)); do
    jq -e '.phase == "Suspended"' "${state_files[0]}" >/dev/null && break
    sleep 0.05
  done
  jq -e '.phase == "Suspended"' "${state_files[0]}"
  wait "$exec_pid" || :
  unset exec_pid
  [[ "$(cat visits)" == PR ]]

  touch release
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -ne 0 && "$output" == *"cruise run $id"* ]]
  [[ "$(cat visits)" == PRGT ]]
  touch repaired
  run "$CRUISE_BIN" run "$id" </dev/null
  [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
  [[ "$(cat visits)" == PRGTRGT && ! -e "$sessions/$id" ]]
}

# bats test_tags=recovery-admission-interrupt
@test "Ctrl+C preserves Failed before recovery admission and saves Suspended afterward" {
  cat > "$workflow" <<'YAML'
command: [echo]
steps:
  repair:
    command: test -f repaired || exit 1; touch running; while [ ! -f release ]; do sleep 0.05; done
YAML
  start_failure
  touch repaired
  "$CRUISE_BIN" daemon stop
  export REAL_GIT="$(command -v git)" SETUP_GATE="$BATS_TEST_TMPDIR/setup"
  mkdir bin
  cat > bin/git <<'SH'
#!/bin/sh
if [ "$*" = 'rev-parse --abbrev-ref HEAD' ] && [ ! -f "$SETUP_GATE.release" ]; then
  touch "$SETUP_GATE.entered"
  for n in $(seq 1 300); do
    [ ! -f "$SETUP_GATE.release" ] || break
    sleep 0.05
  done
fi
exec "$REAL_GIT" "$@"
SH
  chmod +x bin/git
  PATH="$PWD/bin:$PATH" "$CRUISE_BIN" daemon start --port 0
  state="$sessions/$id/state.json"
  cp "$sessions/$id/dag.json" "$BATS_TEST_TMPDIR/admission-checkpoint.json"
  for phase in Failed Suspended; do
    run env TERM=xterm RECOVERY_ID="$id" EXPECT_PHASE="$phase" expect -c '
      set timeout 20
      spawn -noecho $env(CRUISE_BIN) run $env(RECOVERY_ID)
      expect {
        -exact {not rolled back.} {}
        timeout { exit 1 }
        eof { exit 1 }
      }
      set before [expr {$env(EXPECT_PHASE) eq "Failed"}]
      set gate [expr {$before ? "$env(SETUP_GATE).entered" : "running"}]
      for {set i 0} {$i < 200 && ![file exists $gate]} {incr i} { after 50 }
      if {![file exists $gate]} { exit 1 }
      send -- "\003"
      expect {
        -exact {Cancelling session} {}
        timeout { exit 1 }
        eof { exit 1 }
      }
      if {$before} {
        after 500
        close [open "$env(SETUP_GATE).release" w]
        set message {exec session is still Failed.}
      } else {
        set message {session saved as Suspended.}
      }
      expect {
        -exact $message {}
        timeout { exit 1 }
        eof { exit 1 }
      }
      expect eof
    '
    [[ "$status" -eq 0 ]] || { echo "$output"; return 1; }
    if [[ "$phase" == Failed ]]; then
      cmp "$BATS_TEST_TMPDIR/admission-checkpoint.json" "$sessions/$id/dag.json"
      jq -e '.phase | has("Failed")' "$state"
      [[ ! -e running ]]
    else
      jq -e '.phase == "Suspended"' "$state"
    fi
  done
}
