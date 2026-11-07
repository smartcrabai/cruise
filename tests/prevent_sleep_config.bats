#!/usr/bin/env bats

setup() {
  TEST_ROOT="$(mktemp -d)"
  mkdir -p "$TEST_ROOT/home" "$TEST_ROOT/config/cruise" "$TEST_ROOT/project"
  export HOME="$TEST_ROOT/home"
  export XDG_CONFIG_HOME="$TEST_ROOT/config"
  export XDG_DATA_HOME="$TEST_ROOT/data"
  export XDG_STATE_HOME="$TEST_ROOT/state"
  export NO_COLOR=1
  cd "$TEST_ROOT/project"
}

teardown() {
  rm -rf "$TEST_ROOT"
}

@test "config displays the configured machine sleep setting" {
  printf 'prevent_sleep: true\n' > "$XDG_CONFIG_HOME/cruise/config.yaml"
  run "$CRUISE_BIN" config
  [ "$status" -eq 0 ]
  [[ "$output" == *"prevent_sleep = true"* ]]
}

@test "config displays sleep prevention disabled by default" {
  run "$CRUISE_BIN" config
  [ "$status" -eq 0 ]
  [[ "$output" == *"prevent_sleep = false"* ]]
}
