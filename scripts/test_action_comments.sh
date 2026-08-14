#!/usr/bin/env bash
# Exercises action/scripts/comment-start.sh and action/scripts/finalize.sh
# directly against the runner contract described in action.yml (the
# comment_start and finalize steps).
. "$(dirname "$0")/lib/action_test_harness.sh"

# --- shared `gh` stub -------------------------------------------------
# Records every invocation as NUL-delimited fields in $STUB_LOG (argc,
# then each arg) so a single call's argv can be reconstructed exactly --
# including an argument that itself contains embedded newlines, which is
# exactly what needs checking for finalize.sh's `-f body=...` call. When
# invoked with `--jq FILTER` it applies FILTER (via real jq) to the canned
# response in $GH_RESPONSE_FILE, matching gh's own `--jq` behavior.
export GH_RESPONSE_FILE="$TMP/gh_response.json"
export GH_EXIT_FILE="$TMP/gh_exit"
: > "$GH_RESPONSE_FILE"
printf '0' > "$GH_EXIT_FILE"
export GH_STDERR_MSG=""
stub gh <<'SH'
#!/usr/bin/env bash
printf '%s\0' "$#" >> "$STUB_LOG"
printf '%s\0' "$@" >> "$STUB_LOG"
jqf=""
prev=""
for a in "$@"; do
  [ "$prev" = "--jq" ] && jqf="$a"
  prev="$a"
done
if [ -n "$jqf" ] && [ -s "$GH_RESPONSE_FILE" ]; then
  jq -r "$jqf" "$GH_RESPONSE_FILE"
fi
if [ -n "${GH_STDERR_MSG:-}" ]; then
  echo "$GH_STDERR_MSG" >&2
fi
status=0
[ -s "$GH_EXIT_FILE" ] && status="$(cat "$GH_EXIT_FILE")"
exit "$status"
SH

reset_gh() { # $1=exit_code $2=response_json $3=stderr_msg
  printf '%s' "$1" > "$GH_EXIT_FILE"
  printf '%s' "$2" > "$GH_RESPONSE_FILE"
  export GH_STDERR_MSG="$3"
}

# Reconstructs the single recorded `gh` call from $STUB_LOG into $ARGC and
# the plain indexed array GH_ARGV (bash 3.2 compatible: no associative
# arrays, no mapfile).
read_gh_call() {
  ARGC=0
  GH_ARGV=()
  local i=0 first=1 field
  while IFS= read -r -d '' field; do
    if [ "$first" -eq 1 ]; then
      ARGC="$field"
      first=0
    else
      GH_ARGV[i]="$field"
      i=$((i + 1))
    fi
  done < "$STUB_LOG"
}

# =======================================================================
# comment-start.sh
# =======================================================================

reset_comment_start_env() {
  unset GITHUB_REPOSITORY ENTITY_NUMBER COMMAND TRIGGER_ACTOR \
        GITHUB_SERVER_URL GITHUB_RUN_ID 2>/dev/null || true
}

run_comment_start() {
  CS_OUT="$(bash action/scripts/comment-start.sh 2>&1)"
  CS_STATUS=$?
}

new_case
reset_comment_start_env
reset_gh 0 '{"id": 42424242}' ""
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 COMMAND=run \
       TRIGGER_ACTOR=alice GITHUB_SERVER_URL=https://github.example GITHUB_RUN_ID=999
run_comment_start
read_gh_call
if [ "$CS_STATUS" -eq 0 ] && [ "$(out comment_id)" = "42424242" ]; then
  pass "comment-start: posts a comment and writes comment_id from the gh api response"
else
  fail "comment-start: posts a comment and writes comment_id from the gh api response" "status=$CS_STATUS comment_id=$(out comment_id) out=$CS_OUT"
fi
assert_eq "comment-start: posts to repos/{repo}/issues/{entity_number}/comments" \
  "repos/owner/repo/issues/7/comments" "${GH_ARGV[1]:-}"
expected_body="body=🧭 **cruise** is on it, @alice -- planning and opening a pull request... [View run](https://github.example/owner/repo/actions/runs/999)"
assert_eq "comment-start: body names the command's verb, the triggering actor, and the run link" \
  "$expected_body" "${GH_ARGV[3]:-}"

new_case
reset_comment_start_env
reset_gh 0 '{"id": 1}' ""
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 COMMAND=exec TRIGGER_ACTOR=bob
run_comment_start
read_gh_call
assert_contains "comment-start: exec command uses the 'executing directly' verb" \
  "${GH_ARGV[3]:-}" "executing directly on the default branch (no PR)"

new_case
reset_comment_start_env
reset_gh 0 '{"id": 1}' ""
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 COMMAND=plan TRIGGER_ACTOR=bob
run_comment_start
read_gh_call
assert_contains "comment-start: plan command uses the 'drafting a plan' verb" \
  "${GH_ARGV[3]:-}" "drafting a plan"

new_case
reset_comment_start_env
reset_gh 0 '{"id": 1}' ""
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 COMMAND=fix TRIGGER_ACTOR=bob
run_comment_start
read_gh_call
assert_contains "comment-start: fix command uses the 'revising the plan' verb" \
  "${GH_ARGV[3]:-}" "revising the plan"

new_case
reset_comment_start_env
reset_gh 0 '{"id": 1}' ""
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 COMMAND=frobnicate TRIGGER_ACTOR=bob
run_comment_start
read_gh_call
assert_contains "comment-start: an unrecognized command falls back to the generic 'working on this' verb" \
  "${GH_ARGV[3]:-}" "working on this"

new_case
reset_comment_start_env
reset_gh 0 '{"id": 1}' ""
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 TRIGGER_ACTOR=bob
run_comment_start
read_gh_call
assert_contains "comment-start: an unset COMMAND defaults to 'run' (planning and opening a pull request)" \
  "${GH_ARGV[3]:-}" "planning and opening a pull request"

new_case
reset_comment_start_env
reset_gh 0 '{"id": 1}' ""
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 COMMAND=run
run_comment_start
read_gh_call
assert_contains "comment-start: an unset TRIGGER_ACTOR defaults to 'someone'" \
  "${GH_ARGV[3]:-}" "@someone"

new_case
reset_comment_start_env
reset_gh 0 '{"id": 1}' ""
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 COMMAND=run TRIGGER_ACTOR=bob
run_comment_start
read_gh_call
assert_contains "comment-start: unset GITHUB_SERVER_URL/GITHUB_RUN_ID still produce a well-formed run link" \
  "${GH_ARGV[3]:-}" "[View run](https://github.com/owner/repo/actions/runs/)"

# The tracking comment posted here is looked up downstream purely by
# comment_id (COMMENT_ID threaded through finalize.sh's env) -- nothing else
# re-parses its body by content. In particular it must NOT carry the plan
# marker lib/plan.sh actually greps comments for (that marker is only
# rendered by run.sh's plan/fix flow onto a *different* comment); pin the
# current PLAN_MARKER literal here rather than hardcoding a second copy, so
# a silent rename in lib/plan.sh is caught even though this script doesn't
# use it. This isn't a real exercise of a comment-start.sh code path, only
# a cross-file absence check -- so it's hard-guarded: refute_contains's
# built-in empty-needle guard makes the case fail loudly (instead of
# vacuously matching everything) if the grep/sed extraction below ever
# comes back empty, e.g. from a behaviour-preserving rename in lib/plan.sh.
plan_marker="$(grep '^PLAN_MARKER=' action/scripts/lib/plan.sh | sed -e "s/^PLAN_MARKER='//" -e "s/'\$//")"
new_case
reset_comment_start_env
reset_gh 0 '{"id": 1}' ""
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 COMMAND=run TRIGGER_ACTOR=bob
run_comment_start
read_gh_call
refute_contains "comment-start: tracking comment body carries no plan-tracking marker (identified only by comment_id, never re-parsed by content)" \
  "${GH_ARGV[3]:-}" "$plan_marker"

new_case
reset_comment_start_env
export ENTITY_NUMBER=7
run_comment_start
if [ "$CS_STATUS" -ne 0 ] && printf '%s\n' "$CS_OUT" | grep -Fq 'GITHUB_REPOSITORY is required'; then
  pass "comment-start: missing GITHUB_REPOSITORY fails fast with the ':?' message"
else
  fail "comment-start: missing GITHUB_REPOSITORY fails fast with the ':?' message" "status=$CS_STATUS out=$CS_OUT"
fi

new_case
reset_comment_start_env
export GITHUB_REPOSITORY=owner/repo
run_comment_start
if [ "$CS_STATUS" -ne 0 ] && printf '%s\n' "$CS_OUT" | grep -Fq 'ENTITY_NUMBER is required'; then
  pass "comment-start: missing ENTITY_NUMBER fails fast with the ':?' message"
else
  fail "comment-start: missing ENTITY_NUMBER fails fast with the ':?' message" "status=$CS_STATUS out=$CS_OUT"
fi

# `comment_start` runs with `continue-on-error: true` precisely because a
# failed post must not skip the actual run, and finalize.sh already
# tolerates an empty COMMENT_ID -- so the behaviours this pins are (a) gh
# was actually invoked with the expected endpoint -- the positive anchor
# that makes (c) meaningful, since otherwise a script that never called gh
# at all would also "pass" -- (b) the script itself does exit non-zero
# (set -euo pipefail aborts on the failed command substitution), and (c)
# it must NOT have written a bogus comment_id first.
new_case
reset_comment_start_env
reset_gh 1 "" "gh: HTTP 502 (something exploded)"
export GITHUB_REPOSITORY=owner/repo ENTITY_NUMBER=7 COMMAND=run TRIGGER_ACTOR=alice
run_comment_start
read_gh_call
assert_eq "comment-start: a failing gh call still invokes gh with the expected endpoint before aborting" \
  "repos/owner/repo/issues/7/comments" "${GH_ARGV[1]:-}"
assert_nonzero_status "comment-start: a failing gh call makes the script itself exit non-zero (tolerated by the step's continue-on-error)" \
  "$CS_STATUS" "out=$CS_OUT"
assert_eq "comment-start: a failing gh call writes no comment_id output" "" "$(out comment_id)"

# =======================================================================
# finalize.sh
# =======================================================================

reset_finalize_env() {
  unset GITHUB_REPOSITORY PROCEED GATE_ERROR COMMAND COMMENT_ID RUN_OUTCOME \
        START_TS SESSION_ID PR_URL COMMIT_URL PLAN_COMMENT_URL FAIL_REASON \
        GITHUB_SERVER_URL GITHUB_RUN_ID 2>/dev/null || true
}

run_finalize() {
  FZ_OUT="$(bash action/scripts/finalize.sh 2>&1)"
  FZ_STATUS=$?
}

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=false GATE_ERROR=
run_finalize
if [ "$FZ_STATUS" -eq 0 ] && [ "$(out conclusion)" = "skipped" ] && [ ! -s "$STUB_LOG" ]; then
  pass "finalize: proceed!=true with no gate_error -> conclusion=skipped, exit 0, no gh call"
else
  fail "finalize: proceed!=true with no gate_error -> conclusion=skipped, exit 0, no gh call" \
    "status=$FZ_STATUS conclusion=$(out conclusion) stub_log=$(cat "$STUB_LOG")"
fi

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=false GATE_ERROR='no usable API key found'
run_finalize
if [ "$FZ_STATUS" -eq 0 ] && [ "$(out conclusion)" = "failure" ] && [ ! -s "$STUB_LOG" ]; then
  pass "finalize: proceed!=true with a gate_error -> conclusion=failure, exit 0, still no gh call"
else
  fail "finalize: proceed!=true with a gate_error -> conclusion=failure, exit 0, still no gh call" \
    "status=$FZ_STATUS conclusion=$(out conclusion) stub_log=$(cat "$STUB_LOG")"
fi
assert_contains "finalize: the gate_error text is echoed to stdout" "$FZ_OUT" "no usable API key found"

# --- success: per-command link selection -------------------------------

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID=555 RUN_OUTCOME=success START_TS= SESSION_ID= \
       PR_URL=https://github.example/owner/repo/pull/9 COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON= \
       GITHUB_SERVER_URL=https://github.example GITHUB_RUN_ID=42
run_finalize
read_gh_call
if [ "$FZ_STATUS" -eq 0 ] && [ "$(out conclusion)" = "success" ]; then
  pass "finalize: run success -> conclusion=success, exit 0"
else
  fail "finalize: run success -> conclusion=success, exit 0" "status=$FZ_STATUS conclusion=$(out conclusion)"
fi
assert_eq "finalize: ARGC=6 -- the PATCH body is a single -f argument, never shell-split across the body's newlines" \
  "6" "$ARGC"
assert_eq "finalize: PATCHes repos/{repo}/issues/comments/{comment_id}" \
  "repos/owner/repo/issues/comments/555" "${GH_ARGV[1]:-}"
assert_eq "finalize: PATCH call uses -X PATCH -f" "-X PATCH -f" "${GH_ARGV[2]:-} ${GH_ARGV[3]:-} ${GH_ARGV[4]:-}"
run_body="${GH_ARGV[5]:-}"
assert_contains "finalize: run success body links the PR" "$run_body" "- Pull request: https://github.example/owner/repo/pull/9"
refute_contains "finalize: run success body names only the PR link, not a commit bullet" "$run_body" "- Commit:"
refute_contains "finalize: run success body names only the PR link, not a plan bullet" "$run_body" "- Plan:"
refute_contains "finalize: run success body names only the PR link, not a revised-plan bullet" "$run_body" "- Revised plan:"
assert_contains "finalize: run success body reports 0s elapsed when START_TS is empty" "$run_body" "finished in 0s."

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=exec \
       COMMENT_ID=555 RUN_OUTCOME=success START_TS= SESSION_ID= \
       PR_URL= COMMIT_URL=https://github.example/owner/repo/commit/abc PLAN_COMMENT_URL= FAIL_REASON=
run_finalize
read_gh_call
exec_body="${GH_ARGV[5]:-}"
assert_contains "finalize: exec success body links the commit" "$exec_body" "- Commit: https://github.example/owner/repo/commit/abc"
refute_contains "finalize: exec success body names only the commit link, not a PR bullet" "$exec_body" "- Pull request:"
refute_contains "finalize: exec success body names only the commit link, not a plan bullet" "$exec_body" "- Plan:"
refute_contains "finalize: exec success body names only the commit link, not a revised-plan bullet" "$exec_body" "- Revised plan:"
refute_contains "finalize: exec success body names only the commit link, not the no-changes fallback" "$exec_body" "No file changes were produced"

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=exec \
       COMMENT_ID=555 RUN_OUTCOME=success START_TS= SESSION_ID= \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON=
run_finalize
read_gh_call
assert_contains "finalize: exec success with an empty commit_url reports 'no file changes were produced'" \
  "${GH_ARGV[5]:-}" "No file changes were produced; nothing was pushed."

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=plan \
       COMMENT_ID=555 RUN_OUTCOME=success START_TS= SESSION_ID= \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL=https://github.example/owner/repo/issues/1#issuecomment-1 FAIL_REASON=
run_finalize
read_gh_call
plan_body="${GH_ARGV[5]:-}"
assert_contains "finalize: plan success body links the plan comment as 'Plan:'" \
  "$plan_body" "- Plan: https://github.example/owner/repo/issues/1#issuecomment-1"
refute_contains "finalize: plan success body says 'Plan:' not 'Revised plan:'" "$plan_body" "- Revised plan:"

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=fix \
       COMMENT_ID=555 RUN_OUTCOME=success START_TS= SESSION_ID= \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL=https://github.example/owner/repo/issues/1#issuecomment-1 FAIL_REASON=
run_finalize
read_gh_call
fix_body="${GH_ARGV[5]:-}"
assert_contains "finalize: fix success body links the plan comment as 'Revised plan:'" \
  "$fix_body" "- Revised plan: https://github.example/owner/repo/issues/1#issuecomment-1"
refute_contains "finalize: fix success body says 'Revised plan:' not a bare 'Plan:'" \
  "$fix_body" "- Plan: https://github.example/owner/repo/issues/1#issuecomment-1"

# --- session id bullet --------------------------------------------------

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID=555 RUN_OUTCOME=success START_TS= SESSION_ID=sess-abc123 \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON=
run_finalize
read_gh_call
assert_contains "finalize: success body includes the session id bullet when SESSION_ID is set" \
  "${GH_ARGV[5]:-}" '- Session: `sess-abc123`'

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID=555 RUN_OUTCOME=success START_TS= SESSION_ID= \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON=
run_finalize
read_gh_call
assert_contains "finalize: success body without a session id still reaches gh with a real PATCH body" \
  "${GH_ARGV[5]:-}" "finished in 0s."
refute_contains "finalize: success body omits the session id bullet when SESSION_ID is empty" \
  "${GH_ARGV[5]:-}" "- Session:"

# --- failure outcome -----------------------------------------------------

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID=555 RUN_OUTCOME=failure START_TS= SESSION_ID=sess-xyz \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON='cruise exited with status 1'
run_finalize
read_gh_call
fail_body="${GH_ARGV[5]:-}"
if [ "$FZ_STATUS" -eq 0 ] && [ "$(out conclusion)" = "failure" ]; then
  pass "finalize: run_outcome=failure -> conclusion=failure, exit 0"
else
  fail "finalize: run_outcome=failure -> conclusion=failure, exit 0" "status=$FZ_STATUS conclusion=$(out conclusion)"
fi
assert_contains "finalize: failure body includes FAIL_REASON when set" "$fail_body" "- cruise exited with status 1"
assert_contains "finalize: failure body includes the session id bullet too" "$fail_body" '- Session: `sess-xyz`'
assert_contains "finalize: failure body points at the run logs instead of quoting them" \
  "$fail_body" "logs are never posted to this thread"

# --- failure body: exact contract, no extra log excerpt ------------------
# finalize.sh:73-86 builds the failure body from exactly three things: the
# header/duration line, the (optional) session bullet, FAIL_REASON, and the
# run-log link -- never a log excerpt. Pin that precisely: for a fixed,
# single-line FAIL_REASON the PATCH body must equal the fully-reconstructed
# string byte for byte, so any mutation that appends, removes, or reorders
# content (including a real log tail) is caught, rather than only ever
# checking that one specific fixed secret string doesn't leak.
new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID=555 RUN_OUTCOME=failure START_TS= SESSION_ID= \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON='cruise exited with status 1'
run_finalize
read_gh_call
expected_fail_body="body=❌ **cruise** failed after 0s.

- cruise exited with status 1
- See the [run logs](https://github.com/owner/repo/actions/runs/) for details (logs are never posted to this thread)."
assert_eq "finalize: failure body is exactly the run-log link plus the single-line FAIL_REASON -- no additional log excerpt" \
  "$expected_fail_body" "${GH_ARGV[5]:-}"

# Security-relevant contract (docs/github-actions.md "Run logs are never
# posted back to the issue"): FAIL_REASON is the only channel finalize.sh
# has for surfacing text about the run, and passing it straight through to
# the gh PATCH body (asserted above/below) is intentional. What must NOT
# happen is a `tail -n 20 "$LOG_FILE"`-shaped regression that smuggles extra
# key=value lines into $GITHUB_OUTPUT: finalize.sh's only write to
# $GITHUB_OUTPUT is the final `conclusion=$conclusion` line -- it never
# writes comment_body there at all -- so pin that precisely with a
# FAIL_REASON crafted to look like extra step outputs if it ever leaked in.
new_case
reset_finalize_env
reset_gh 0 "" ""
multiline_fail_reason="cruise exited with status 1
malicious_output=pwned
another_key=leaked"
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID=555 RUN_OUTCOME=failure START_TS= SESSION_ID= \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON="$multiline_fail_reason"
run_finalize
read_gh_call
assert_contains "finalize: a multi-line FAIL_REASON still reaches the PATCH body verbatim" \
  "${GH_ARGV[5]:-}" "- $multiline_fail_reason"
assert_eq "finalize: \$GITHUB_OUTPUT gets only the conclusion line even when FAIL_REASON contains key=value-shaped lines" \
  "conclusion=failure" "$(cat "$GITHUB_OUTPUT")"

# --- empty COMMENT_ID: conclusion is still written, gh is never called ---

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID= RUN_OUTCOME=success START_TS= SESSION_ID= \
       PR_URL=https://github.example/owner/repo/pull/1 COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON=
run_finalize
if [ "$(out conclusion)" = "success" ] && [ ! -s "$STUB_LOG" ]; then
  pass "finalize: empty COMMENT_ID with a successful run still writes conclusion=success and skips gh"
else
  fail "finalize: empty COMMENT_ID with a successful run still writes conclusion=success and skips gh" \
    "conclusion=$(out conclusion) stub_log=$(cat "$STUB_LOG")"
fi
assert_contains "finalize: empty COMMENT_ID logs that the comment update was skipped" \
  "$FZ_OUT" "no tracking comment id available, skipping comment update"

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID= RUN_OUTCOME=failure START_TS= SESSION_ID= \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON='boom'
run_finalize
if [ "$(out conclusion)" = "failure" ] && [ ! -s "$STUB_LOG" ]; then
  pass "finalize: empty COMMENT_ID with a failed run still writes conclusion=failure and skips gh"
else
  fail "finalize: empty COMMENT_ID with a failed run still writes conclusion=failure and skips gh" \
    "conclusion=$(out conclusion) stub_log=$(cat "$STUB_LOG")"
fi

# --- duration -------------------------------------------------------------

new_case
reset_finalize_env
reset_gh 0 "" ""
now_ts="$(date +%s)"
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID=555 RUN_OUTCOME=success START_TS=$((now_ts - 125)) SESSION_ID= \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON=
run_finalize
read_gh_call
dur="$(printf '%s' "${GH_ARGV[5]:-}" | sed -n 's/.*finished in \(-\{0,1\}[0-9]*\)s\..*/\1/p')"
if [ -n "$dur" ] && [ "$dur" -ge 120 ] && [ "$dur" -le 135 ] 2>/dev/null; then
  pass "finalize: a non-empty START_TS produces a plausible elapsed-seconds duration"
else
  fail "finalize: a non-empty START_TS produces a plausible elapsed-seconds duration" "dur=$dur body=${GH_ARGV[5]:-}"
fi

new_case
reset_finalize_env
reset_gh 0 "" ""
export GITHUB_REPOSITORY=owner/repo PROCEED=true GATE_ERROR= COMMAND=run \
       COMMENT_ID=555 RUN_OUTCOME=failure START_TS= SESSION_ID= \
       PR_URL= COMMIT_URL= PLAN_COMMENT_URL= FAIL_REASON=
run_finalize
read_gh_call
assert_contains "finalize: an empty START_TS reports 0s rather than a garbage/negative duration" \
  "${GH_ARGV[5]:-}" "failed after 0s."

finish
