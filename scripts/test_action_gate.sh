#!/usr/bin/env bash
# Exercises action/scripts/gate.sh directly: the trigger-phrase match, actor
# authorization (collaborator permission / bot allow-list), and command-word
# parsing that together decide `proceed` and populate every output
# action.yml's later steps consume (entity_number, actor, actor_id, command,
# command_rest_file, gate_error -- plus `proceed` itself). Only `gh` is
# stubbed; jq and python3 are the real binaries, exactly like production.
#
# scripts/test_action_provider_config.sh already covers gate.sh's
# credential-gate exit status (empty anthropic/openai/provider_api_keys/
# providers/env -> non-zero) without ever reading
# $GITHUB_OUTPUT; this suite deliberately does not repeat those credential
# cases and instead asserts the actual output *values* on every path.

. "$(dirname "$0")/lib/action_test_harness.sh"

# `gh` is the only external dependency gate.sh calls that we control here:
# it looks up the commenting/mentioning actor's collaborator permission via
# `gh api repos/<repo>/collaborators/<actor>/permission --jq '.permission'`.
# The stub logs its invocation (so a test can confirm the right repo/actor
# were queried) and returns GH_PERMISSION_RESPONSE on stdout, or exits
# GH_PERMISSION_EXIT to simulate a failed lookup (gate.sh redirects stderr
# to /dev/null and tolerates a non-zero exit via `|| true`, ending up with
# an empty $permission either way).
stub gh <<'SH'
#!/usr/bin/env bash
printf 'gh %s\n' "$*" >> "$STUB_LOG"
if [ "${GH_PERMISSION_EXIT:-0}" -ne 0 ]; then
  exit "$GH_PERMISSION_EXIT"
fi
printf '%s' "${GH_PERMISSION_RESPONSE:-write}"
SH

# Builds an issue_comment event payload at $1.
# $2=body $3=login $4=id $5=type(User|Bot) $6=is_pr(true|false) $7=action
ic_event() {
  must jq -n \
    --arg action "$7" \
    --arg body "$2" \
    --arg login "$3" \
    --argjson id "$4" \
    --arg type "$5" \
    --argjson is_pr "$6" \
    '{action: $action, issue: {number: 42, pull_request: (if $is_pr then {} else null end)},
      comment: {body: $body, user: {login: $login, id: $id, type: $type}}}' \
    > "$1"
}

# Builds an issues event payload at $1.
# $2=title $3=body $4=login $5=id $6=type $7=action $8=number
issues_event() {
  must jq -n \
    --arg action "$7" \
    --arg title "$2" \
    --arg body "$3" \
    --arg login "$4" \
    --argjson id "$5" \
    --arg type "$6" \
    --argjson number "$8" \
    '{action: $action, issue: {number: $number, title: $title, body: $body,
      user: {login: $login, id: $id, type: $type}}}' \
    > "$1"
}

# Default issue_comment fixture (login=alice id=123 type=User, not a PR,
# action=created) carrying $1 as the comment body. Sets the event env vars
# and runs gate.sh.
comment_event() {
  ic_event "$TMP/event.json" "$1" alice 123 User false created
  export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
  run_gate
}

# Runs gate.sh, capturing combined stdout+stderr into $output and its exit
# status into $status.
run_gate() {
  output="$(bash action/scripts/gate.sh 2>&1)"
  status=$?
}

# Every env var gate.sh reads, reset to a baseline that proceeds all the way
# through (valid trigger, write-permission actor, one non-empty credential
# input) unless a test overrides something.
reset_gate_env() {
  export GITHUB_REPOSITORY=owner/repo
  export GH_TOKEN=fake-gh-token
  export TRIGGER_PHRASE=@cruise
  export ALLOWED_BOTS=
  export ANTHROPIC_API_KEY_INPUT=has-a-key
  export OPENAI_API_KEY_INPUT=
  export ENV_INPUT=
  export PROVIDER_API_KEYS_INPUT=
  export PROVIDERS_INPUT=
  export GH_PERMISSION_RESPONSE=write
  export GH_PERMISSION_EXIT=0
  unset GITHUB_EVENT_NAME GITHUB_EVENT_PATH
}

assert_nonempty() { # $1=name $2=value
  if [ -n "$2" ]; then pass "$1"; else fail "$1" "value was empty"; fi
}

# =============================================================================
# Happy path: every one of the 7 outputs action.yml consumes (proceed,
# entity_number, actor, actor_id, command, command_rest_file, gate_error) is
# produced with the right value. Renaming any `out` key in gate.sh (e.g.
# `out entity_number` -> `out entity_num`) makes `out KEY` return empty here,
# which mismatches the non-empty expected value on 6 of the 7 keys; the 7th
# (gate_error, expected empty here) is covered by the hard_fail case below,
# where it is expected non-empty.
# =============================================================================
new_case
reset_gate_env
comment_event "@cruise please fix the bug"
assert_status "gate: happy path exits 0" 0 "$status" "$output"
assert_eq "gate: happy path proceed=true" "true" "$(out proceed)"
assert_eq "gate: happy path entity_number" "42" "$(out entity_number)"
assert_eq "gate: happy path actor" "alice" "$(out actor)"
assert_eq "gate: happy path actor_id" "123" "$(out actor_id)"
assert_eq "gate: happy path command defaults to run" "run" "$(out command)"
assert_eq "gate: happy path gate_error is empty" "" "$(out gate_error)"
assert_eq "gate: happy path is_bot=false" "false" "$(out is_bot)"
rest_file="$(out command_rest_file)"
assert_nonempty "gate: happy path command_rest_file output is non-empty" "$rest_file"
if [ -f "$rest_file" ] && [ -r "$rest_file" ]; then
  pass "gate: command_rest_file names a real, readable file"
else
  fail "gate: command_rest_file names a real, readable file" "'$rest_file' missing or unreadable"
fi
assert_eq "gate: command_rest_file contains the text after the mention" "please fix the bug" "$(cat "$rest_file")"

# =============================================================================
# hard_fail: all credential inputs empty. Unlike deny() (proceed=false,
# gate_error="", exit 0), hard_fail() must ALSO leave gate_error non-empty
# and exit non-zero -- finalize.sh tells a real configuration error apart
# from a plain skip solely by whether gate_error is blank. Blanking
# hard_fail()'s `out gate_error "$1"` (i.e. writing "" instead) is exactly
# the mutation this catches: it would leave `status` and `proceed` alone but
# silently downgrade a hard failure into what finalize.sh reads as "skipped".
# =============================================================================
new_case
reset_gate_env
export ANTHROPIC_API_KEY_INPUT= OPENAI_API_KEY_INPUT= ENV_INPUT= PROVIDER_API_KEYS_INPUT= PROVIDERS_INPUT=
comment_event "@cruise please fix the bug"
assert_nonzero_status "gate: hard_fail on empty credentials exits non-zero" "$status" "$output"
assert_eq "gate: hard_fail sets proceed=false" "false" "$(out proceed)"
assert_nonempty "gate: hard_fail sets a non-empty gate_error" "$(out gate_error)"
assert_contains "gate: hard_fail names the empty credential inputs" "$output" "are all empty"
reset_gate_env

# =============================================================================
# Trigger matching: strict word-boundary, case-insensitive, honors a custom
# TRIGGER_PHRASE. Catches: dropping the (^|\s) prefix requirement or the
# ([\s.,!?;:]|$) suffix requirement from the regex (either would make
# "email@cruise.com" or "@cruisex" match), dropping re.IGNORECASE, or
# hardcoding "@cruise" instead of using $TRIGGER_PHRASE.
# =============================================================================
new_case
reset_gate_env
comment_event "hey @cruise please help"
assert_eq "gate: trigger matches on a leading word boundary" "true" "$(out proceed)"

new_case
reset_gate_env
comment_event "email@cruise.com typo"
assert_eq "gate: trigger does not match mid-token (no preceding whitespace)" "false" "$(out proceed)"
assert_contains "gate: trigger mismatch names the phrase" "$output" "trigger phrase '@cruise' not found in body"

new_case
reset_gate_env
comment_event "@cruisex is a variable name"
assert_eq "gate: trigger does not match without a trailing boundary" "false" "$(out proceed)"

new_case
reset_gate_env
comment_event "@CRUISE Please Help"
assert_eq "gate: trigger match is case-insensitive" "true" "$(out proceed)"

new_case
reset_gate_env
export TRIGGER_PHRASE="/cruise-ci"
comment_event "/cruise-ci run this"
assert_eq "gate: a custom trigger_phrase matches" "true" "$(out proceed)"
assert_eq "gate: a custom trigger_phrase still parses the command word" "run" "$(out command)"

new_case
reset_gate_env
export TRIGGER_PHRASE="/cruise-ci"
comment_event "@cruise run this"
assert_eq "gate: the default phrase no longer matches once trigger_phrase is customized" "false" "$(out proceed)"
reset_gate_env

# =============================================================================
# Command grammar. Each case checks both `command` and the exact
# `command_rest_file` contents. Catches: flipping the "anything unrecognized
# defaults to run" fallback to some other command (e.g. exec, which would
# push straight to the default branch on a plain mention), dropping the
# optional "/" prefix strip, dropping trailing-punctuation stripping,
# matching the FIRST mention instead of the LAST, or matching a substring
# instead of the first whitespace-delimited token.
# =============================================================================
gate_command_case() { # $1=test-label $2=body $3=expected-command $4=expected-rest
  new_case
  reset_gate_env
  comment_event "$2"
  assert_eq "gate: command grammar -- $1 (command)" "$3" "$(out command)"
  assert_eq "gate: command grammar -- $1 (command_rest_file contents)" "$4" "$(cat "$(out command_rest_file)")"
}

gate_command_case "bare mention with no trailing text defaults to run" \
  "@cruise" "run" ""
gate_command_case "bare mention with trailing text defaults to run, rest is the whole text" \
  "@cruise please fix the bug" "run" "please fix the bug"
gate_command_case "plan" \
  "@cruise plan the feature" "plan" "the feature"
gate_command_case "fix" \
  "@cruise fix the flaky test" "fix" "the flaky test"
gate_command_case "exec" \
  "@cruise exec now" "exec" "now"
gate_command_case "optional slash prefix" \
  "@cruise /fix retry logic" "fix" "retry logic"
gate_command_case "trailing punctuation stripped from the command word" \
  "@cruise plan: do the design" "plan" "do the design"
gate_command_case "command word matching is case-insensitive" \
  "@cruise FIX capitalization" "fix" "capitalization"
gate_command_case "an unknown word after the mention defaults to run, kept verbatim as rest" \
  "@cruise banana bread" "run" "banana bread"
gate_command_case "multiple mentions: the LAST one wins" \
  "@cruise plan first thing
@cruise fix second thing" "fix" "second thing"
reset_gate_env

# =============================================================================
# Authorization: admin/write proceed; read/none/a failed permission lookup
# deny, and the denial reason must surface in the gate's output. Catches:
# deleting the `admin | write) : ;;` arm's write case (docs promise
# write-only access), or deleting the `*) deny ...` arm entirely (both would
# let an unauthorized commenter proceed).
# =============================================================================
new_case
reset_gate_env
export GH_PERMISSION_RESPONSE=admin
comment_event "@cruise run"
assert_eq "gate: admin permission proceeds" "true" "$(out proceed)"

new_case
reset_gate_env
export GH_PERMISSION_RESPONSE=write
comment_event "@cruise run"
assert_eq "gate: write permission proceeds" "true" "$(out proceed)"

new_case
reset_gate_env
export GH_PERMISSION_RESPONSE=read
comment_event "@cruise run"
assert_eq "gate: read permission denies" "false" "$(out proceed)"
assert_contains "gate: read permission denial names the actual permission" "$output" "insufficient permission: 'read'"

new_case
reset_gate_env
export GH_PERMISSION_RESPONSE=none
comment_event "@cruise run"
assert_eq "gate: 'none' permission denies" "false" "$(out proceed)"
assert_contains "gate: 'none' permission denial names the actual permission" "$output" "insufficient permission: 'none'"

new_case
reset_gate_env
export GH_PERMISSION_EXIT=1
export GH_PERMISSION_RESPONSE=
comment_event "@cruise run"
assert_eq "gate: a failed permission lookup denies" "false" "$(out proceed)"
assert_contains "gate: a failed permission lookup denies as 'unknown'" "$output" "insufficient permission: 'unknown'"

new_case
reset_gate_env
comment_event "@cruise run"
assert_contains "gate: the permission check queries the right repo and actor" "$(cat "$STUB_LOG")" "repos/owner/repo/collaborators/alice/permission"
reset_gate_env

# =============================================================================
# Bot actors. Catches: making the default (empty allowed_bots) permissive
# instead of blocking, breaking the "[bot]" suffix stripping on either side
# of the comparison, breaking the case-insensitive compare, allowing a bot
# not on the list, or breaking the "*" wildcard.
# =============================================================================
new_case
reset_gate_env
ic_event "$TMP/event.json" "@cruise run" "mybot[bot]" 1 User false created
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: default (empty allowed_bots) blocks a bot actor" "false" "$(out proceed)"
assert_contains "gate: default bot denial names the actor and allowed_bots" "$output" "bot actor 'mybot[bot]' is not in allowed_bots ('')"

new_case
reset_gate_env
export ALLOWED_BOTS="mybot[bot]"
ic_event "$TMP/event.json" "@cruise run" "mybot[bot]" 1 User false created
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: an exact (suffixed) allowed_bots entry allows that bot" "true" "$(out proceed)"

new_case
reset_gate_env
export ALLOWED_BOTS="mybot"
ic_event "$TMP/event.json" "@cruise run" "mybot[bot]" 1 User false created
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: an allowed_bots entry without the [bot] suffix still matches an actor that has it" "true" "$(out proceed)"

new_case
reset_gate_env
export ALLOWED_BOTS="MyBot"
ic_event "$TMP/event.json" "@cruise run" "mybot[bot]" 1 User false created
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: bot allow-list matching is case-insensitive" "true" "$(out proceed)"

new_case
reset_gate_env
export ALLOWED_BOTS="otherbot,thirdbot"
ic_event "$TMP/event.json" "@cruise run" "mybot[bot]" 1 User false created
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: a non-matching allowed_bots list still denies" "false" "$(out proceed)"

new_case
reset_gate_env
export ALLOWED_BOTS="*"
ic_event "$TMP/event.json" "@cruise run" "anybot[bot]" 1 User false created
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: allowed_bots='*' allows any bot" "true" "$(out proceed)"

new_case
reset_gate_env
export ALLOWED_BOTS="robocopy"
# GH_PERMISSION_RESPONSE=none: if actor_type=="Bot" detection were deleted,
# this actor (no "[bot]" suffix in the login) would fall through to the
# collaborator-permission branch instead of the bot allow-list, and "none"
# ensures that fallback denies -- so proceed=true here is only possible via
# the actor_type=="Bot" path, not an accidental pass-through.
export GH_PERMISSION_RESPONSE=none
ic_event "$TMP/event.json" "@cruise run" "robocopy" 1 Bot false created
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: actor_type=Bot (no [bot] suffix in the login) is still treated as a bot" "true" "$(out proceed)"
reset_gate_env

# =============================================================================
# Event shapes: issues (opened) vs issue_comment (created), a comment on a
# pull request, unsupported event names/actions, and malformed payloads
# (missing issue/actor). Catches: swapping which JSON fields feed
# number/actor/actor_id/body for each event type, deleting the PR-comment
# rejection ("this action has no PR mode"), or deleting the event
# name/action allowlist checks.
# =============================================================================
new_case
reset_gate_env
issues_event "$TMP/event.json" "@cruise" "Implement feature X." bob 99 User opened 7
export GITHUB_EVENT_NAME=issues GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_status "gate: issues(opened) exits 0" 0 "$status" "$output"
assert_eq "gate: issues(opened) proceeds" "true" "$(out proceed)"
assert_eq "gate: issues(opened) entity_number comes from .issue.number" "7" "$(out entity_number)"
assert_eq "gate: issues(opened) actor comes from .issue.user.login" "bob" "$(out actor)"
assert_eq "gate: issues(opened) actor_id comes from .issue.user.id" "99" "$(out actor_id)"
assert_eq "gate: issues(opened) body is title+body, so the mention in the title still triggers" "run" "$(out command)"
assert_eq "gate: issues(opened) command_rest_file gets the issue body" "Implement feature X." "$(cat "$(out command_rest_file)")"

new_case
reset_gate_env
ic_event "$TMP/event.json" "@cruise run" alice 123 User true created
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: a comment on a pull request is always denied" "false" "$(out proceed)"
assert_contains "gate: PR-comment denial names the reason" "$output" "issue_comment is on a pull request -- this action has no PR mode"

new_case
reset_gate_env
ic_event "$TMP/event.json" "@cruise run" alice 123 User false edited
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: issue_comment action != created is denied" "false" "$(out proceed)"
assert_contains "gate: wrong issue_comment action names the actual action" "$output" "issue_comment action is 'edited', not 'created'"

new_case
reset_gate_env
issues_event "$TMP/event.json" "@cruise run" "body" alice 123 User closed 7
export GITHUB_EVENT_NAME=issues GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: issues action != opened is denied" "false" "$(out proceed)"
assert_contains "gate: wrong issues action names the actual action" "$output" "issues action is 'closed', not 'opened'"

new_case
reset_gate_env
ic_event "$TMP/event.json" "@cruise run" alice 123 User false created
export GITHUB_EVENT_NAME=pull_request GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: an unsupported event name is denied" "false" "$(out proceed)"
assert_contains "gate: unsupported event names the event" "$output" "unsupported event: pull_request (only issues/issue_comment are supported)"

new_case
reset_gate_env
export GITHUB_EVENT_NAME=issue_comment
unset GITHUB_EVENT_PATH
run_gate
assert_eq "gate: a missing event payload path is denied" "false" "$(out proceed)"
assert_contains "gate: missing event payload names the reason" "$output" "no event payload found"

new_case
reset_gate_env
must jq -n --arg body "@cruise run" --arg login alice '{action: "created", comment: {body: $body, user: {login: $login, id: 1, type: "User"}}}' > "$TMP/event.json"
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: a payload missing .issue entirely is denied" "false" "$(out proceed)"
assert_contains "gate: missing issue number names the reason" "$output" "could not determine issue number"

new_case
reset_gate_env
must jq -n --arg body "@cruise run" '{action: "created", issue: {number: 5, pull_request: null}, comment: {body: $body}}' > "$TMP/event.json"
export GITHUB_EVENT_NAME=issue_comment GITHUB_EVENT_PATH="$TMP/event.json"
run_gate
assert_eq "gate: a payload missing comment.user is denied" "false" "$(out proceed)"
assert_contains "gate: missing actor names the reason" "$output" "could not determine actor"
reset_gate_env

finish
