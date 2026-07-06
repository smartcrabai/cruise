#!/usr/bin/env bash
# Shared helpers for resolving, rendering, and posting/editing the
# "plan tracking" comment used by the plan/fix/run/exec commands. Meant to be
# `source`d (not executed directly) by run.sh.
#
# Comment format (see docs/github-actions.md):
#
#   <!-- cruise:plan -->
#   ## 📋 Plan
#   <plan.md content>
#   ---
#   _Reply `@cruise fix <feedback>` to revise, or `@cruise run` to execute this plan._
#
# The marker/header/footer strings below are the single source of truth for
# both rendering (render_plan_comment) and parsing back out (extract_plan_body).
PLAN_MARKER='<!-- cruise:plan -->'
PLAN_HEADER='## 📋 Plan'
PLAN_FOOTER_DIVIDER='---'
# shellcheck disable=SC2016 # single-quoted on purpose: the backticks are literal Markdown, not command substitution.
PLAN_FOOTER_TEXT='_Reply `@cruise fix <feedback>` to revise, or `@cruise run` to execute this plan._'

# Logins allowed to author a comment that run/exec/fix will trust as an
# authoritative plan source, space-separated. A marker match alone is NOT
# enough: without this check, any commenter (even one without write access,
# since only the *mention* needs to come from an authorized actor -- any
# other comment on the issue is unauthenticated as far as plan trust goes)
# could post a fake "<!-- cruise:plan -->" comment that a maintainer's later
# `@cruise run`/`exec` would then execute unreviewed. Restricted to this
# action's own posting identities (see comment-start.sh's tracking comment
# and app-token.sh's token resolution); extend this list if a self-hosted
# App identity is ever added.
PLAN_AUTHOR_LOGINS="cruise-agent[bot] github-actions[bot]"

# Strips content that can smuggle hidden directives into an LLM prompt:
#  - HTML comments (<!-- ... -->), invisible in rendered Markdown
#  - <img> tags, whose alt text is a known injection channel
#  - zero-width / bidi-control / other invisible Unicode characters
# Multiline-safe (Python, not sed). This cannot stop instructions written as
# plain visible text -- that residual risk is documented in
# docs/github-actions.md. Applied to raw GitHub-sourced text (issue
# title/body, fix feedback, extracted plan-comment content, the triggering
# comment's trailing instructions) before it is used as planning input.
sanitize_text() {
  python3 -c '
import re, sys
text = sys.stdin.read()
text = re.sub(r"<!--.*?-->", "", text, flags=re.DOTALL)
text = re.sub(r"<img\b[^>]*>", "", text, flags=re.IGNORECASE | re.DOTALL)
text = re.sub("[\u200b-\u200f\u202a-\u202e\u2060-\u2064\ufeff\u00ad]", "", text)
sys.stdout.write(text)
'
}

# Fetches every comment on the issue (paginated, oldest-first) flattened into
# a single JSON array. Prints nothing and returns non-zero if the `gh` call
# itself fails -- callers must NOT treat that the same as "no comments
# found" (falling back to the issue body on a fetch failure could silently
# run/exec against the wrong plan source).
fetch_issue_comments_json() { # $1=repo $2=entity_number
  local raw
  if ! raw="$(gh api --paginate "repos/$1/issues/$2/comments?per_page=100" 2>&1)"; then
    echo "::error::cruise: failed to fetch issue comments for #$2: $raw" >&2
    return 1
  fi
  printf '%s' "$raw" | jq -rs 'add // []'
}

# Finds the LAST comment that both contains the plan marker AND was authored
# by one of $PLAN_AUTHOR_LOGINS as a Bot (comments are returned oldest-first,
# so the last match is the most recent *trusted* plan comment). Prints the
# comment id to stdout (empty if none found) and writes the raw comment body
# to $3 (empty file if none found or on fetch failure).
#
# Return code: 0 = lookup completed (id may be empty), 2 = the comments
# fetch itself failed (distinct from "not found" -- callers must abort
# rather than silently fall back).
find_last_plan_comment() { # $1=repo $2=entity_number $3=out_body_file -> prints comment id or empty
  local comments_json id
  if ! comments_json="$(fetch_issue_comments_json "$1" "$2")"; then
    : > "$3"
    return 2
  fi
  id="$(printf '%s' "$comments_json" | jq -r \
    --arg marker "$PLAN_MARKER" \
    --arg logins "$PLAN_AUTHOR_LOGINS" '
    ($logins | split(" ")) as $allowed
    | [.[] | select(
        ((.body // "") | contains($marker))
        and ((.user.type // "") == "Bot")
        and (((.user.login // "") as $login | $allowed | index($login)) != null)
      )]
    | last | .id // empty')"
  if [ -n "$id" ]; then
    printf '%s' "$comments_json" | jq -r --arg id "$id" '
      .[] | select((.id|tostring)==$id) | .body // ""' > "$3"
  else
    : > "$3"
  fi
  printf '%s' "$id"
}

# Extracts the plan.md content embedded in a rendered plan-comment body
# (strips the marker/header and the trailing divider+reply-hint footer).
# Falls back to the whole (trimmed) input if the header isn't found, so a
# hand-edited or unexpected comment body doesn't silently disappear.
extract_plan_body() { # $1=raw_comment_body_file -> plan content on stdout
  python3 -c '
import sys
header, footer, path = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(path, encoding="utf-8").read()
h = text.find(header)
if h == -1:
    sys.stdout.write(text.strip() + "\n")
    sys.exit(0)
start = h + len(header)
f = text.rfind(footer)
end = f if f != -1 else len(text)
body = text[start:end].strip("\n")
if body.endswith("---"):
    body = body[:-3].rstrip("\n")
sys.stdout.write(body.strip("\n") + "\n")
' "$PLAN_HEADER" "$PLAN_FOOTER_TEXT" "$1"
}

# Renders the full plan-tracking comment body (marker + header + plan
# content + footer) to $2, reading the plan content from file $1.
render_plan_comment() { # $1=plan_content_file $2=out_file
  {
    printf '%s\n' "$PLAN_MARKER"
    printf '%s\n' "$PLAN_HEADER"
    echo
    cat "$1"
    echo
    printf '%s\n' "$PLAN_FOOTER_DIVIDER"
    printf '%s\n' "$PLAN_FOOTER_TEXT"
  } > "$2"
}

# GitHub caps issue comment bodies at 65536 characters. Cap well below that
# so a truncation note always fits, and always preserve the reply-hint
# footer (never silently drop it) -- only the plan content in the middle is
# trimmed. This only affects the *posted comment*; cruise reads the
# untruncated plan.md for actual execution, so truncation here never changes
# what the agent runs.
COMMENT_MAX_CHARS=60000
cap_comment_body() { # $1=rendered_comment_file -> capped body on stdout
  python3 -c '
import sys
limit = int(sys.argv[1])
footer_divider, footer_text, path = sys.argv[2], sys.argv[3], sys.argv[4]
text = open(path, encoding="utf-8").read()
if len(text) <= limit:
    sys.stdout.write(text)
    sys.exit(0)
note = "\n\n_(plan truncated for comment length; full plan was still used for execution)_\n"
tail = "\n" + footer_divider + "\n" + footer_text + "\n"
budget = max(limit - len(note) - len(tail), 0)
sys.stdout.write(text[:budget] + note + tail)
' "$COMMENT_MAX_CHARS" "$PLAN_FOOTER_DIVIDER" "$PLAN_FOOTER_TEXT" "$1"
}

# Appends a sanitized "## Additional instructions from the triggering
# comment" section to $1 (a plan-source file that already has content),
# taken from $2 (the command_rest_file gate.sh wrote -- the text after the
# parsed command word in the comment/issue body that triggered this run).
# No-op if $2 is unset, missing, or blank after sanitizing -- this keeps
# `@cruise run`/`@cruise <request>` (no rest text) byte-identical to before.
append_rest_instructions() { # $1=target_plan_file $2=command_rest_file (optional)
  local target="$1" rest_file="${2:-}"
  [ -z "$rest_file" ] && return 0
  [ -f "$rest_file" ] || return 0
  local rest
  rest="$(sanitize_text < "$rest_file")"
  local trimmed
  trimmed="$(printf '%s' "$rest" | tr -d '[:space:]')"
  [ -z "$trimmed" ] && return 0
  {
    echo
    echo "## Additional instructions from the triggering comment"
    echo
    printf '%s\n' "$rest"
  } >> "$target"
}

# Resolves the plan text used by the run/exec commands: the last *trusted*
# plan-marker comment's content if one exists, otherwise the issue's own
# title + body (mention lines are left in verbatim -- they read fine as task
# context). Either way, any additional instructions typed after the command
# word in the triggering comment/issue body ($4, gate.sh's
# command_rest_file) are appended so a request like "@cruise run also add a
# changelog entry" isn't silently discarded once a plan comment exists.
#
# Writes the resolved plan text to $3 and prints "marker:<id>" or
# "issue_body" to stdout for logging.
#
# Return code: 0 = success, 2 = the underlying comments fetch failed --
# callers MUST abort rather than treat this the same as "no plan comment".
resolve_plan_source() { # $1=repo $2=entity_number $3=out_plan_file $4=command_rest_file (optional)
  local rest_file="${4:-}"
  local body_file id rc
  body_file="$(mktemp)"
  id="$(find_last_plan_comment "$1" "$2" "$body_file")"
  rc=$?
  if [ "$rc" -eq 2 ]; then
    rm -f "$body_file"
    return 2
  fi
  if [ -n "$id" ]; then
    extract_plan_body "$body_file" | sanitize_text > "$3"
    rm -f "$body_file"
    append_rest_instructions "$3" "$rest_file"
    printf 'marker:%s' "$id"
    return 0
  fi
  rm -f "$body_file"

  local issue_json title issue_body
  issue_json="$(gh api "repos/$1/issues/$2")"
  title="$(printf '%s' "$issue_json" | jq -r '.title // ""' | sanitize_text)"
  issue_body="$(printf '%s' "$issue_json" | jq -r '.body // ""' | sanitize_text)"
  {
    printf '%s\n' "$title"
    echo
    printf '%s\n' "$issue_body"
  } > "$3"
  append_rest_instructions "$3" "$rest_file"
  printf 'issue_body'
}
