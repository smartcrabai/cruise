#!/usr/bin/env bash
# Decides whether cruise should run for this event: strict trigger-phrase
# match (word boundary, same rule as anthropics/claude-code-action), an actor
# authorization check (collaborator write/admin permission, or an
# allow-listed bot), and -- once a run is authorized -- parses which cruise
# command was requested. Never hard-fails on "this event doesn't apply" -- it
# always exits 0 and communicates the verdict via `proceed`. Configuration
# errors (missing tooling, no usable API key) DO hard-fail (non-zero exit)
# via hard_fail(), which -- unlike deny() -- also records `gate_error` so
# finalize.sh reports `conclusion=failure` instead of misreporting a real
# error as `skipped` (PROCEED would otherwise read as empty/false either way).
#
# Only `issues` (opened) and `issue_comment` (created) are supported. A
# comment made on a pull request (`.issue.pull_request` present) is always
# denied: this action no longer has a PR mode.
set -uo pipefail

TRIGGER_PHRASE="${TRIGGER_PHRASE:-@cruise}"
ALLOWED_BOTS="${ALLOWED_BOTS:-}"
ANTHROPIC_API_KEY_INPUT="${ANTHROPIC_API_KEY_INPUT:-}"
OPENAI_API_KEY_INPUT="${OPENAI_API_KEY_INPUT:-}"
ENV_INPUT="${ENV_INPUT:-}"
EVENT_NAME="${GITHUB_EVENT_NAME:-}"
EVENT_PATH="${GITHUB_EVENT_PATH:-}"
REPO="${GITHUB_REPOSITORY:-}"

out() {
  echo "$1=$2" >> "$GITHUB_OUTPUT"
}

deny() {
  echo "cruise: not proceeding - $1"
  out proceed false
  out entity_number ""
  out actor ""
  out actor_id ""
  out is_bot false
  out command ""
  out command_rest_file ""
  out gate_error ""
  exit 0
}

# Unlike deny() (this event legitimately doesn't apply), hard_fail() is for
# genuine configuration errors. It still writes `proceed=false` so later
# steps stay skipped, but also writes `gate_error` (non-empty) so
# finalize.sh can tell the two apart and report `conclusion=failure` rather
# than `skipped`.
hard_fail() {
  echo "::error::cruise: $1" >&2
  out proceed false
  out entity_number ""
  out actor ""
  out actor_id ""
  out is_bot false
  out command ""
  out command_rest_file ""
  out gate_error "$1"
  exit 1
}

if [ -z "$EVENT_PATH" ] || [ ! -f "$EVENT_PATH" ]; then
  deny "no event payload found"
fi

if ! command -v jq >/dev/null 2>&1; then
  deny "jq is not available"
fi

# Missing python3 is a runner-configuration error, not a "this event doesn't
# apply" case, so hard-fail instead of silently skipping.
if ! command -v python3 >/dev/null 2>&1; then
  hard_fail "python3 is required by the cruise action (GitHub-hosted runners include it; self-hosted runners must install it)"
fi

jqr() {
  jq -r "$1 // empty" "$EVENT_PATH"
}

action="$(jqr '.action')"
number=""
body=""
actor=""
actor_id=""
actor_type=""

case "$EVENT_NAME" in
  issue_comment)
    if [ "$action" != "created" ]; then
      deny "issue_comment action is '$action', not 'created'"
    fi
    is_pr="$(jq -r 'if .issue.pull_request then "true" else "false" end' "$EVENT_PATH")"
    if [ "$is_pr" = "true" ]; then
      deny "issue_comment is on a pull request -- this action has no PR mode"
    fi
    body="$(jqr '.comment.body')"
    actor="$(jqr '.comment.user.login')"
    actor_id="$(jqr '.comment.user.id')"
    actor_type="$(jqr '.comment.user.type')"
    number="$(jqr '.issue.number')"
    ;;
  issues)
    if [ "$action" != "opened" ]; then
      deny "issues action is '$action', not 'opened'"
    fi
    title="$(jqr '.issue.title')"
    issue_body="$(jqr '.issue.body')"
    body="$title
$issue_body"
    actor="$(jqr '.issue.user.login')"
    actor_id="$(jqr '.issue.user.id')"
    actor_type="$(jqr '.issue.user.type')"
    number="$(jqr '.issue.number')"
    ;;
  *)
    deny "unsupported event: $EVENT_NAME (only issues/issue_comment are supported)"
    ;;
esac

if [ -z "$number" ] || [ "$number" = "null" ]; then
  deny "could not determine issue number"
fi

# Strict word-boundary trigger match: (^|\s)<phrase>([\s.,!?;:]|$), the same
# rule anthropics/claude-code-action uses, matched case-insensitively. The
# whole check runs inside Python so re.escape() and the engine that consumes
# it always agree (Python 3.7+ re.escape() is not compatible with POSIX ERE
# for every metacharacter).
if ! printf '%s' "$body" | python3 -c '
import re, sys
phrase = sys.argv[1]
body = sys.stdin.read()
pattern = r"(^|\s)" + re.escape(phrase) + r"([\s.,!?;:]|$)"
sys.exit(0 if re.search(pattern, body, re.IGNORECASE) else 1)
' "$TRIGGER_PHRASE"; then
  deny "trigger phrase '$TRIGGER_PHRASE' not found in body"
fi

if [ -z "$actor" ] || [ "$actor" = "null" ]; then
  deny "could not determine actor"
fi

is_bot=false
case "$actor" in
  *"[bot]") is_bot=true ;;
esac
if [ "$actor_type" = "Bot" ]; then
  is_bot=true
fi

if [ "$is_bot" = "true" ]; then
  allowed=false
  trimmed_allowed="$(printf '%s' "$ALLOWED_BOTS" | tr -d '[:space:]')"
  if [ "$trimmed_allowed" = "*" ]; then
    allowed=true
  else
    normalized_actor="$(printf '%s' "$actor" | tr '[:upper:]' '[:lower:]')"
    normalized_actor="${normalized_actor%\[bot\]}"
    old_ifs="$IFS"
    IFS=','
    for b in $ALLOWED_BOTS; do
      nb="$(printf '%s' "$b" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')"
      nb="${nb%\[bot\]}"
      if [ -n "$nb" ] && [ "$nb" = "$normalized_actor" ]; then
        allowed=true
      fi
    done
    IFS="$old_ifs"
  fi
  if [ "$allowed" != "true" ]; then
    deny "bot actor '$actor' is not in allowed_bots ('$ALLOWED_BOTS')"
  fi
else
  # The `permission` field only ever reports admin/write/read/none; the
  # "maintain" and "triage" roles are mapped to write and read respectively
  # (they appear only in `role_name`), so maintainers pass the `write` arm.
  permission="$(gh api "repos/$REPO/collaborators/$actor/permission" --jq '.permission' 2>/dev/null || true)"
  case "$permission" in
    admin | write) : ;;
    *) deny "actor '$actor' has insufficient permission: '${permission:-unknown}'" ;;
  esac
fi

# A missing API key is a configuration error, not a "this event doesn't
# apply" case -- surface it as a hard failure now (before installing
# anything) rather than deep inside the run step. A non-empty `env` input
# also counts: pi supports many providers whose keys (KIMI_API_KEY,
# GOOGLE_API_KEY, GROQ_API_KEY, ...) are passed through `env` rather than a
# dedicated input, and gate.sh cannot tell which of those lines is an API
# key -- pi itself fails with a clear MissingApiKey error if none applies.
if [ -z "$ANTHROPIC_API_KEY_INPUT" ] && [ -z "$OPENAI_API_KEY_INPUT" ] && [ -z "$ENV_INPUT" ]; then
  hard_fail "'anthropic_api_key', 'openai_api_key', and 'env' are all empty -- provide an API key so pi can authenticate (either a dedicated input, or a provider key such as KIMI_API_KEY via 'env')"
fi

# Parse the cruise command: the first whitespace-delimited token immediately
# following the LAST trigger-phrase mention in the body (optionally prefixed
# with "/", trailing punctuation stripped, e.g. "plan." -> "plan"),
# case-insensitively matched against run/exec/plan/fix. Anything else --
# including no token at all -- defaults to "run" (bare "@cruise <request>"
# mentions keep working). The remainder of the text after that token
# (verbatim, may be multi-line) is written to a file and used both for the
# "fix" command's feedback and (by lib/plan.sh) as extra instructions
# appended to the resolved plan for "run"/"exec".
#
# The LAST mention (not the first) is used so a quoted reply that includes
# an earlier "@cruise ..." message followed by the replier's own new mention
# is parsed from the new one, not the quoted one.
#
# Command-word matching is intentionally strict (first token only) -- see
# docs/github-actions.md's "Command grammar" section for the exact rule and
# its false-positive/negative edge cases (e.g. a sentence that happens to
# start with "fix").
command_rest_file="${RUNNER_TEMP:-/tmp}/cruise-command-rest.txt"
mkdir -p "$(dirname "$command_rest_file")"
command="$(printf '%s' "$body" | python3 -c '
import re, sys
phrase, rest_file = sys.argv[1], sys.argv[2]
body = sys.stdin.read()
pattern = re.compile(r"(?:^|(?<=\s))" + re.escape(phrase) + r"(?=[\s.,!?;:]|$)", re.IGNORECASE)
matches = list(pattern.finditer(body))
m = matches[-1] if matches else None
remainder = body[m.end():] if m else ""
remainder = re.sub(r"^[\s.,!?;:]+", "", remainder)
known = {"run", "exec", "plan", "fix"}
command = "run"
rest = remainder
first_line, _, tail = remainder.partition("\n")
tokens = first_line.split(None, 1)
if tokens:
    candidate = tokens[0]
    bare = candidate[1:] if candidate.startswith("/") else candidate
    bare = bare.rstrip(".,!?;:")
    if bare.lower() in known:
        command = bare.lower()
        after = tokens[1] if len(tokens) > 1 else ""
        rest = (after + "\n" + tail) if tail else after
sys.stdout.write(command)
with open(rest_file, "w", encoding="utf-8") as f:
    f.write(rest.strip())
' "$TRIGGER_PHRASE" "$command_rest_file")"

out proceed true
out entity_number "$number"
out actor "$actor"
out actor_id "$actor_id"
out is_bot "$is_bot"
out command "$command"
out command_rest_file "$command_rest_file"
out gate_error ""
echo "cruise: triggered by @$actor on issue #$number -- command: $command"
