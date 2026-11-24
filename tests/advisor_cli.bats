#!/usr/bin/env bats

setup() {
  [ -n "${CRUISE_BIN:-}" ] && [ -x "$CRUISE_BIN" ] || {
    echo 'CRUISE_BIN must point to the built cruise executable' >&2
    return 1
  }
  local name
  advisor_cruise_bin="$(cd "$(dirname "$CRUISE_BIN")" && pwd)/$(basename "$CRUISE_BIN")"
  while IFS= read -r name; do unset "$name"; done < <(compgen -v CRUISE_)
  export HOME="$BATS_TEST_TMPDIR/home"
  export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
  export XDG_DATA_HOME="$BATS_TEST_TMPDIR/data"
  export XDG_STATE_HOME="$BATS_TEST_TMPDIR/state"
  export CRUISE_UI_LANGUAGE=en CRUISE_DISABLE_NOTIFICATIONS=1 NO_COLOR=1
  unset HERDR_ENV
  mkdir -p "$HOME" "$XDG_CONFIG_HOME/cruise" "$BATS_TEST_TMPDIR/project"
  cd "$BATS_TEST_TMPDIR/project"
}

use_command_backend() {
  printf 'command: [cat]\n' > "$XDG_CONFIG_HOME/cruise/default.yaml"
}

assert_advisor_requires_jcode() {
  [ "$status" -ne 0 ] && [[ "$output" == *advise* && "$output" == *advisor* && "$output" == *jcode* ]] || {
    echo "expected rejection of enabled advise step on command backend (exit $status): $output"
    return 1
  }
}

@test "workflow doctor permits builtin tdd with advisor off or on under jcode" {
  run "$advisor_cruise_bin" workflow doctor builtin:tdd
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
  run "$advisor_cruise_bin" workflow doctor -f advisor builtin:tdd
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
}

@test "workflow doctor rejects builtin tdd on command only when advisor is enabled" {
  use_command_backend
  run "$advisor_cruise_bin" workflow doctor builtin:tdd
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
  run "$advisor_cruise_bin" workflow doctor -f advisor builtin:tdd
  [ "$status" -eq 1 ] || { echo "$output"; return 1; }
  assert_advisor_requires_jcode
}

@test "exec dry-run permits builtin tdd with advisor off or on under jcode" {
  run "$advisor_cruise_bin" exec --dry-run -c builtin:tdd x
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
  run "$advisor_cruise_bin" exec --dry-run -c builtin:tdd -f advisor x
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
}

@test "exec dry-run rejects builtin tdd on command only when advisor is enabled" {
  use_command_backend
  run "$advisor_cruise_bin" exec --dry-run -c builtin:tdd x
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
  [[ "$output" == *'command: cat'* ]] || { echo "$output"; return 1; }
  run "$advisor_cruise_bin" exec --dry-run -c builtin:tdd -f advisor x
  assert_advisor_requires_jcode
}

@test "exec dry-run reports the advisor model set by the environment" {
  run env CRUISE_ADVISOR_MODEL=provider/reviewer "$advisor_cruise_bin" exec --dry-run -c builtin:tdd x
  [ "$status" -eq 0 ] || { echo "$output"; return 1; }
  [[ "$output" == *'advisor_model: provider/reviewer'* ]] || { echo "$output"; return 1; }
}
