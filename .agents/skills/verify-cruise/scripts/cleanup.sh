#!/usr/bin/env bash
# Stop the daemon this run started and delete the run root. Evidence lives
# outside ROOT (target/verify-cruise/) and is never removed. Usage: cleanup.sh ROOT
set -uo pipefail
root=${1:?usage: cleanup.sh ROOT}
scripts=$(cd "$(dirname "$0")" && pwd)
case "$root" in
  "${TMPDIR:-/tmp}"*/verify-cruise.*|/tmp/verify-cruise.*|/private/var/folders/*/verify-cruise.*) ;;
  *) echo "cleanup.sh: refusing to remove $root (not a verify-cruise run root)" >&2; exit 2 ;;
esac
[[ -f "$root/env" ]] || { echo "cleanup.sh: $root is not a verify-cruise run" >&2; exit 2; }
"$scripts/in.sh" "$root" cruise daemon stop --force >/dev/null 2>&1
# Kill only the PID launch.sh recorded, and only while it is still that executable.
pid=$(jq -r .pid "$root/daemon-status.json" 2>/dev/null)
exe=$(jq -r .executable "$root/daemon-status.json" 2>/dev/null)
if [[ "$pid" =~ ^[0-9]+$ ]] && ps -p "$pid" -o command= 2>/dev/null | grep -qF "$exe"; then
  kill "$pid" 2>/dev/null
fi
rm -rf "$root"
echo "removed $root"
