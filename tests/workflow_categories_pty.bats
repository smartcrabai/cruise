#!/usr/bin/env bats

# Run with: CRUISE_RUNTIME=/path/to/cruise-runtime bats tests/workflow_categories_pty.bats
# The default runtime is the locally built debug binary. No build is performed.
setup() {
  ROOT="$(cd "$BATS_TEST_DIRNAME/.." && pwd)"
  RUNTIME="${CRUISE_RUNTIME:-$ROOT/target/debug/cruise-runtime}"
  if [[ ! -x "$RUNTIME" ]]; then
    echo "runtime binary not available: $RUNTIME" >&2
    return 1
  fi
  TEST_ROOT="$(mktemp -d "${BATS_TEST_TMPDIR:-${TMPDIR:-/tmp}}/cruise-pty.XXXXXX")"
  mkdir -p "$TEST_ROOT/home" "$TEST_ROOT/config/cruise" "$TEST_ROOT/data" "$TEST_ROOT/project"
  cat > "$TEST_ROOT/config/cruise/config.yaml" <<'YAML'
workflow_categories:
  Dev:
    Backend:
      workflows:
        - builtin:refactor
show_others_category: false
YAML
}

teardown() {
  if [[ -n "${TEST_ROOT:-}" && -d "$TEST_ROOT" ]]; then
    rm -rf "$TEST_ROOT"
  fi
}

@test "category drilldown, Back, and Escape cancel through a real terminal" {
  run env -u CRUISE_CONFIG HOME="$TEST_ROOT/home" XDG_CONFIG_HOME="$TEST_ROOT/config" \
    XDG_DATA_HOME="$TEST_ROOT/data" XDG_CACHE_HOME="$TEST_ROOT/cache" \
    TERM=xterm EXPECT_PROJECT="$TEST_ROOT/project" \
    EXPECT_RUNTIME="$RUNTIME" expect "$BATS_TEST_DIRNAME/workflow_categories_pty.expect"
  [ "$status" -eq 0 ] || { printf '%s\n' "$output"; return 1; }
}
