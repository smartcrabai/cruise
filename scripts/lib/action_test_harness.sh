#!/usr/bin/env bash
# Shared harness for the scripts/test_action_*.sh suites, which exercise the
# composite action's step scripts (action/scripts/*.sh) directly by faking the
# runner contract: GITHUB_ENV / GITHUB_OUTPUT files, RUNNER_TEMP, and PATH
# stubs standing in for gh / curl / git / cruise.
#
# Source it from a test script (it cd's to the repo root):
#   . "$(dirname "${BASH_SOURCE[0]}")/lib/action_test_harness.sh"
#
# Shell policy, set here once for every suite: `set -uo pipefail` WITHOUT
# `-e`. The suites routinely invoke scripts that are expected to fail and
# then inspect `$?`, which errexit makes impossible without scattered
# `set +e`/`set -e` pairs (and one stray re-enable silently truncates a
# suite). Fixture commands that must succeed go through `must`.
#
# API:
#   $TMP                  scratch dir, removed on exit
#   $STUB_DIR / $STUB_LOG stub bin dir (already first on PATH) / invocation log
#   new_case              truncate GITHUB_ENV, GITHUB_OUTPUT and STUB_LOG
#   stub NAME             create an executable stub from stdin
#   log_stub NAME         create a stub that only records "NAME $*" and succeeds
#   out KEY / genv KEY    last value written for KEY in GITHUB_OUTPUT/GITHUB_ENV
#   must CMD...           run fixture setup, abort the suite if it fails
#   pass NAME / fail NAME DETAIL
#   check NAME COND_STATUS DETAIL   pass when COND_STATUS is 0
#   assert_eq NAME EXPECTED ACTUAL
#   assert_contains NAME HAYSTACK NEEDLE     (empty needle = failure, never a match)
#   refute_contains NAME HAYSTACK NEEDLE
#   assert_status NAME EXPECTED ACTUAL [DETAIL]
#   assert_nonzero_status NAME ACTUAL [DETAIL]
#   assert_file_absent NAME PATH
#   finish                print the tally and exit non-zero on any failure
# Written for bash 3.2 (macOS /bin/bash) as well as CI's bash 5: no
# associative arrays, no `${var^^}`, no `mapfile`, and no `case`/`;;` inside
# a `$( )` command substitution (bash 3.2 mis-parses it).
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

PASS=0
FAIL=0

TMP="$(mktemp -d)"
# `rm -rf` must not become the script's exit status: without the explicit
# `exit "$rc"` a suite that dies mid-run (parse error, unset variable) would
# exit 0 because the trap's last command succeeded, and CI would read a
# truncated run as a pass.
trap 'rc=$?; rm -rf "$TMP"; exit "$rc"' EXIT

export GITHUB_ENV="$TMP/github_env"
export GITHUB_OUTPUT="$TMP/github_output"
export RUNNER_TEMP="$TMP/runner"
STUB_DIR="$TMP/bin"
STUB_LOG="$TMP/stub.log"
mkdir -p "$RUNNER_TEMP" "$STUB_DIR"
# Pin the git environment for every suite: fixtures must not pick up the
# developer's global/system gitconfig (a global commit.gpgsign=true or
# core.hooksPath makes fixture commits fail or run foreign hooks), and an
# exported GIT_DIR/GIT_WORK_TREE would redirect `git -C <fixture>` at the
# developer's real repository.
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY
: > "$GITHUB_ENV"
: > "$GITHUB_OUTPUT"
: > "$STUB_LOG"
export PATH="$STUB_DIR:$PATH"
export STUB_LOG

new_case() {
  : > "$GITHUB_ENV"
  : > "$GITHUB_OUTPUT"
  : > "$STUB_LOG"
}

stub() { # $1=command name; body on stdin
  cat > "$STUB_DIR/$1"
  chmod +x "$STUB_DIR/$1"
}

log_stub() { # $1=command name -- records the invocation, exits 0
  stub "$1" <<STUB
#!/usr/bin/env bash
printf '%s\n' "$1 \$*" >> "\$STUB_LOG"
exit 0
STUB
}

out() { # $1=key -- last value written for that key
  sed -n "s/^$1=//p" "$GITHUB_OUTPUT" | tail -n1
}

genv() { # $1=key -- last value exported for that key
  sed -n "s/^$1=//p" "$GITHUB_ENV" | tail -n1
}

pass() {
  echo "PASS: $1"
  PASS=$((PASS + 1))
}

fail() {
  echo "FAIL: $1 -- $2"
  FAIL=$((FAIL + 1))
}

# Fixture setup that MUST succeed. The suites deliberately run without
# `set -e` (they invoke scripts that are expected to fail and inspect `$?`),
# so an unguarded fixture command would fail silently and cascade into
# confusing assertion failures instead of one clear abort.
must() { # $@=command
  if ! "$@"; then
    echo "FATAL: fixture command failed: $*" >&2
    exit 1
  fi
}

check() { # $1=name $2=status of the condition $3=detail
  if [ "$2" -eq 0 ]; then pass "$1"; else fail "$1" "${3:-}"; fi
}

assert_eq() { # $1=name $2=expected $3=actual
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1" "expected '$2', got '$3'"; fi
}

assert_contains() { # $1=name $2=haystack $3=needle
  if [ -z "${3:-}" ]; then
    fail "$1" "empty needle -- the expected substring was computed as empty, which would match anything"
    return
  fi
  case "$2" in
    *"$3"*) pass "$1" ;;
    *) fail "$1" "missing '$3' in: $2" ;;
  esac
}

refute_contains() { # $1=name $2=haystack $3=needle
  if [ -z "${3:-}" ]; then
    fail "$1" "empty needle -- the forbidden substring was computed as empty"
    return
  fi
  case "$2" in
    *"$3"*) fail "$1" "unexpectedly found '$3' in: $2" ;;
    *) pass "$1" ;;
  esac
}

assert_status() { # $1=name $2=expected status $3=actual status $4=detail
  if [ "$2" -eq "$3" ]; then pass "$1"; else fail "$1" "expected exit $2, got $3${4:+ -- $4}"; fi
}

assert_nonzero_status() { # $1=name $2=actual status $3=detail
  if [ "$2" -ne 0 ]; then pass "$1"; else fail "$1" "expected a non-zero exit${3:+ -- $3}"; fi
}

assert_file_absent() { # $1=name $2=path
  if [ -e "$2" ]; then fail "$1" "'$2' exists"; else pass "$1"; fi
}

finish() {
  echo "Results: $PASS passed, $FAIL failed"
  [ "$FAIL" -eq 0 ]
}
