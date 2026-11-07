#!/usr/bin/env bash
# Poll only for the lifetime of the run which owns this process.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib/command.sh"
source "$SCRIPT_DIR/lib/authz.sh"
source "$SCRIPT_DIR/lib/plan.sh"
REPO="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
TRIGGER_PHRASE="${TRIGGER_PHRASE:-@cruise}"
ALLOWED_BOTS="${ALLOWED_BOTS:-}"
CRUISE_DIR="${CRUISE_DIR:-${RUNNER_TEMP:-/tmp}/cruise}"
mkdir -p "$CRUISE_DIR"
seen="$CRUISE_DIR/tell-seen.txt"
touch "$seen"

reply() {
  gh api "repos/$REPO/issues/$1/comments" -f "body=$2" >/dev/null
}
react() {
  gh api "repos/$REPO/issues/comments/$1/reactions" -f content=eyes >/dev/null
}

if [ "${1:-}" = --unhandled-reply ]; then
  cid="${2:?comment id required}"
  issue="${3:-}"
  if [ -z "$issue" ] && [ -n "${GITHUB_EVENT_PATH:-}" ]; then
    issue="$(jq -r '.issue.number // empty' "$GITHUB_EVENT_PATH")"
  fi
  [[ "$cid" =~ ^[0-9]+$ && "$issue" =~ ^[0-9]+$ ]] || exit 1
  # Allow the active run's 15-second poll interval to handle this comment.
  sleep 20
  identity="$(gh api user --jq .login)" || identity=""
  reactions="$(gh api --paginate "repos/$REPO/issues/comments/$cid/reactions?per_page=100")" || reactions=""
  if [ -n "$identity" ] && [ -n "$reactions" ] && printf '%s' "$reactions" | jq -se --arg identity "$identity" 'any(.[][]; .content == "eyes" and .user.login == $identity)' >/dev/null; then
    exit 0
  fi
  reply "$issue" "Could not confirm whether a cruise run handled this instruction. Check the run logs and comment reaction before retrying."
  exit $?
fi

mode="${1:-}"
sid="${2:-}"
issue="${3:-}"
since="${4:-}"
case "$mode" in watch|sweep) ;; *) echo "usage: $0 watch|sweep SESSION ISSUE SINCE" >&2; exit 2 ;; esac
[ -n "$sid" ] && [[ "$issue" =~ ^[0-9]+$ ]] && [ -n "$since" ] || exit 2

sweep() {
  local raw comment cid actor actor_type body cmd rest result reason
  if ! raw="$(gh api --paginate "repos/$REPO/issues/$issue/comments?since=$since&per_page=100")"; then
    echo "::warning::cruise: failed to poll issue comments" >&2
    return 1
  fi
  while IFS= read -r comment; do
    cid="$(jq -r '.id // empty' <<< "$comment")"
    [[ "$cid" =~ ^[0-9]+$ ]] || continue
    grep -Fxq "$cid" "$seen" && continue
    body="$(jq -r '.body // ""' <<< "$comment")"
    rest="$(mktemp "${RUNNER_TEMP:-/tmp}/cruise-tell.XXXXXX")" || return 1
    cmd="$(printf '%s' "$body" | parse_command "$TRIGGER_PHRASE" "$rest")" || cmd=""
    if [ "$cmd" != tell ]; then
      rm -f "$rest"
      continue
    fi
    actor="$(jq -r '.user.login // empty' <<< "$comment")"
    actor_type="$(jq -r '.user.type // empty' <<< "$comment")"
    if ! authorize_actor "$actor" "$actor_type" "$REPO" "$ALLOWED_BOTS"; then
      rm -f "$rest"
      continue
    else
      body="$(sanitize_text < "$rest")"
      if result="$(cruise tell "$sid" --origin github --dedupe-key "github-comment:$cid" -- "$body" 2>&1)"; then
        reason=""
      else
        # Remove the trigger phrase so a reply cannot start another job.
        result="${result//"$TRIGGER_PHRASE"/[redacted]}"
        reason="Instruction was not delivered: $result"
      fi
    fi
    rm -f "$rest"
    if [ -n "$reason" ]; then
      reply "$issue" "$reason" || continue
    fi
    react "$cid" || continue
    printf '%s\n' "$cid" >> "$seen"
  done < <(printf '%s' "$raw" | jq -rc '.[]')
}

if [ "$mode" = sweep ]; then
  sweep
else
  # Give cruise run time to open its inbox before the first poll.
  while sleep 15; do sweep || true; done
fi
