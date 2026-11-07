#!/usr/bin/env bash
# Returns success only for an allowed bot or a collaborator with write access.
authorize_actor() {
  local actor="$1" actor_type="$2" repo="$3" bots="$4" b nb normalized permission
  AUTHZ_IS_BOT=false
  AUTHZ_REASON=""
  if [ -z "$actor" ] || [ "$actor" = null ]; then
    AUTHZ_REASON="could not determine actor"
    return 1
  fi
  case "$actor" in *'[bot]') AUTHZ_IS_BOT=true ;; esac
  [ "$actor_type" = Bot ] && AUTHZ_IS_BOT=true
  if [ "$AUTHZ_IS_BOT" = true ]; then
    [ "$(printf '%s' "$bots" | tr -d '[:space:]')" = '*' ] && return 0
    normalized="$(printf '%s' "$actor" | tr '[:upper:]' '[:lower:]')"
    normalized="${normalized%\[bot\]}"
    local old_ifs="$IFS"
    IFS=','
    for b in $bots; do
      nb="$(printf '%s' "$b" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')"
      nb="${nb%\[bot\]}"
      if [ -n "$nb" ] && [ "$nb" = "$normalized" ]; then
        IFS="$old_ifs"
        return 0
      fi
    done
    IFS="$old_ifs"
    AUTHZ_REASON="bot actor '$actor' is not in allowed_bots ('$bots')"
    return 1
  fi
  permission="$(gh api "repos/$repo/collaborators/$actor/permission" --jq '.permission' 2>/dev/null || true)"
  case "$permission" in admin|write) return 0 ;; esac
  AUTHZ_REASON="actor '$actor' has insufficient permission: '${permission:-unknown}'"
  return 1
}
