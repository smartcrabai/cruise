#!/usr/bin/env bash
# Exercise the real tell poller with fake gh/cruise at the service boundary.
# The shared harness supplies isolated runner files and assertion helpers.
. "$(dirname "$0")/lib/action_test_harness.sh"

stub gh <<'SH'
#!/usr/bin/env bash
jq -cn '$ARGS.positional' --args -- "$@" >> "$GH_CALLS"
case "$*" in
  'api user --jq .login') printf '%s' 'cruise[bot]' ;;
  *collaborators/*/permission*) printf '%s' "$PERMISSION" ;;
  *issues/comments/*/reactions?per_page=100*) cat "$REACTIONS" ;;
  *issues/*/comments?since=*) cat "$COMMENTS" ;;
  *) : ;;
esac
SH
stub cruise <<'SH'
#!/usr/bin/env bash
jq -cn '$ARGS.positional' --args -- "$@" >> "$CRUISE_CALLS"
if [ "$CRUISE_EXIT" -ne 0 ]; then
  printf '%s\n' "$CRUISE_ERROR" >&2
  exit "$CRUISE_EXIT"
fi
SH
# watch executes exactly one iteration without a real 15-second delay.
stub sleep <<'SH'
#!/usr/bin/env bash
if [ -e "$SLEEP_MARKER" ]; then exit 1; fi
: > "$SLEEP_MARKER"
SH

reset_tell() {
  new_case
  export GITHUB_REPOSITORY=owner/repo TRIGGER_PHRASE=@cruise ALLOWED_BOTS=
  export CRUISE_DIR="$TMP/cruise" COMMENTS="$TMP/comments.json" REACTIONS="$TMP/reactions.json"
  export GH_CALLS="$TMP/gh.jsonl" CRUISE_CALLS="$TMP/cruise.jsonl" SLEEP_MARKER="$TMP/slept"
  export PERMISSION=write CRUISE_EXIT=0 CRUISE_ERROR=
  rm -rf "$CRUISE_DIR" "$SLEEP_MARKER"
  : > "$GH_CALLS"; : > "$CRUISE_CALLS"
  printf '[]\n' > "$COMMENTS"; printf '[]\n' > "$REACTIONS"
}
comment() { # id, actor, type, body
  must jq -n --argjson id "$1" --arg actor "$2" --arg type "$3" --arg body "$4" \
    '[{id:$id, user:{login:$actor,type:$type}, body:$body}]' > "$COMMENTS"
}
run_poller() {
  output="$(bash action/scripts/tell-poller.sh "$@" 2>&1)"
  status=$?
}
count_calls() { jq -s "[.[] | select($1)] | length" "$2"; }
assert_cruise() { # exact argument vector and call count
  assert_eq "$1" "$2" "$(jq -cs '.' "$CRUISE_CALLS")"
}
reaction_count() { count_calls '.[1] | strings | contains("/reactions")' "$GH_CALLS"; }
reply_count() { count_calls '.[1] == "repos/owner/repo/issues/42/comments"' "$GH_CALLS"; }

# T-1: Watch processes an authorized collaborator exactly once.
reset_tell
comment 101 alice User '@cruise /tell do this'
run_poller watch session-1 42 2026-01-01T00:00:00Z
assert_status 'T-1 watch exits after controlled iteration' 0 "$status" "$output"
assert_cruise 'T-1 exact cruise arguments and one call' '[["tell","session-1","--origin","github","--dedupe-key","github-comment:101","--","do this"]]'
assert_eq 'T-1 adds eyes reaction' 1 "$(count_calls '.[1] == "repos/owner/repo/issues/comments/101/reactions" and .[2] == "-f" and .[3] == "content=eyes"' "$GH_CALLS")"

# T-2: A collaborator without write permission is ignored.
reset_tell
export PERMISSION=none
comment 102 alice User '@cruise /tell no'
run_poller sweep session-1 42 2026-01-01T00:00:00Z
assert_status 'T-2 sweep succeeds' 0 "$status" "$output"
assert_cruise 'T-2 no delivery' '[]'
assert_eq 'T-2 no reaction' 0 "$(reaction_count)"

# T-3: Bot authorization is controlled by the allow-list.
reset_tell
comment 103 'robot[bot]' Bot '@cruise /tell bot instruction'
run_poller sweep session-1 42 2026-01-01T00:00:00Z
assert_cruise 'T-3 unlisted bot ignored' '[]'
assert_eq 'T-3 unlisted bot has no reaction' 0 "$(reaction_count)"
export ALLOWED_BOTS='*'
run_poller sweep session-1 42 2026-01-01T00:00:00Z
assert_status 'T-3 wildcard sweep succeeds' 0 "$status" "$output"
assert_cruise 'T-3 wildcard permits bot' '[["tell","session-1","--origin","github","--dedupe-key","github-comment:103","--","bot instruction"]]'
assert_eq 'T-3 wildcard adds reaction' 1 "$(reaction_count)"

# T-4: A previously seen comment is not delivered or reacted to again.
reset_tell
comment 104 alice User '@cruise /tell duplicate'
must mkdir -p "$CRUISE_DIR"
printf '104\n' > "$CRUISE_DIR/tell-seen.txt"
run_poller sweep session-1 42 2026-01-01T00:00:00Z
assert_status 'T-4 sweep succeeds' 0 "$status" "$output"
assert_cruise 'T-4 seen comment not delivered' '[]'
assert_eq 'T-4 seen comment not reacted to' 0 "$(reaction_count)"

# T-5: One sweep, with a failure reason that cannot re-trigger the action.
reset_tell
comment 105 alice User '@cruise /tell late'
export CRUISE_EXIT=1 CRUISE_ERROR='NotRunning: @cruise run already finished'
run_poller sweep session-1 42 2026-01-01T00:00:00Z
assert_status 'T-5 sweep succeeds after reporting failure' 0 "$status" "$output"
assert_eq 'T-5 polls once' 1 "$(count_calls '.[1] == "--paginate" and (.[2] | contains("/issues/42/comments?since="))' "$GH_CALLS")"
assert_cruise 'T-5 attempts delivery once' '[["tell","session-1","--origin","github","--dedupe-key","github-comment:105","--","late"]]'
assert_eq 'T-5 posts one failure reply' 1 "$(reply_count)"
reply="$(jq -r 'select(.[1] == "repos/owner/repo/issues/42/comments") | .[3]' "$GH_CALLS")"
assert_contains 'T-5 reply includes NotRunning reason' "$reply" 'NotRunning: [redacted] run already finished'
refute_contains 'T-5 reply does not contain trigger phrase' "$reply" '@cruise'

# T-6: Only authenticated identity eyes means handled.
reset_tell
printf '%s\n' '[{"content":"eyes","user":{"type":"Bot","login":"cruise[bot]"}}]' > "$REACTIONS"
run_poller --unhandled-reply 106 42
assert_status 'T-6 handled check succeeds' 0 "$status" "$output"
assert_eq 'T-6 existing eyes suppresses reply' 0 "$(reply_count)"
printf '[]\n' > "$REACTIONS"
run_poller --unhandled-reply 106 42
assert_status 'T-6 unhandled check succeeds' 0 "$status" "$output"
assert_eq 'T-6 missing eyes posts reply' 1 "$(reply_count)"
assert_contains 'T-6 reply reports uncertainty' "$(cat "$GH_CALLS")" 'Could not confirm whether a cruise run handled'

# T-7: Hidden HTML, image tags and invisible Unicode are stripped before delivery.
reset_tell
invisible="$(printf '\342\200\213')"
comment 107 alice User "@cruise /tell visible<!-- hidden --> <img alt=\"injected\"> text${invisible}end"
run_poller sweep session-1 42 2026-01-01T00:00:00Z
assert_status 'T-7 sweep succeeds' 0 "$status" "$output"
assert_cruise 'T-7 exact sanitized instruction' '[["tell","session-1","--origin","github","--dedupe-key","github-comment:107","--","visible  textend"]]'
finish
