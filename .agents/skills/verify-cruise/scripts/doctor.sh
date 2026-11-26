#!/usr/bin/env bash
# Read-only health check for a verify-cruise run. Prints one line per check and
# exits 1 if any check fails. Usage: doctor.sh ROOT
set -uo pipefail
root=${1:?usage: doctor.sh ROOT}
scripts=$(cd "$(dirname "$0")" && pwd)
failed=0
ok() { printf 'ok    %s\n' "$1"; }
bad() { printf 'FAIL  %s\n' "$1"; failed=1; }

[[ -f "$root/env" && -f "$root/source" ]] || { bad "$root is not a verify-cruise run"; exit 1; }
source_repo=$(cat "$root/source")
run_home=$(sed -n 's/^HOME=//p' "$root/env")
if [[ "$run_home" == "$root/home" && "$run_home" != "$HOME" ]]; then ok "isolated HOME $run_home"; else bad "HOME is not isolated: $run_home"; fi

runtime="$source_repo/target/debug/cruise-runtime"
status=$("$scripts/in.sh" "$root" cruise daemon status --json 2>/dev/null) || status=
launched_pid=$(jq -r .pid "$root/daemon-status.json" 2>/dev/null)
if [[ -n "$status" && $(jq -r .running <<<"$status") == true ]]; then ok "daemon running at $(jq -r .origin <<<"$status")"; else bad "daemon not running"; fi
if [[ -n "$status" && $(jq -r .pid <<<"$status") == "$launched_pid" ]]; then ok "daemon PID $launched_pid was started by this run"; else bad "daemon PID differs from the one launch.sh started ($launched_pid)"; fi
if [[ -n "$status" && $(jq -r .executable <<<"$status") == "$runtime" ]]; then ok "daemon executable is this checkout's build"; else bad "daemon executable is not $runtime"; fi
if [[ -n "$status" && $(jq -r .outdated <<<"$status") == false ]]; then ok "client and daemon versions match"; else bad "daemon reports outdated"; fi

head=$(git -C "$source_repo" rev-parse HEAD)
if [[ "$head" == "$(cat "$root/source-head")" ]]; then ok "source HEAD $head unchanged since launch"; else bad "source HEAD moved since launch; relaunch"; fi
# launch.sh writes source-head right after building.
newer=$(cd "$source_repo" && find src webui build.rs Cargo.toml Cargo.lock -newer "$root/source-head" -type f -print -quit 2>/dev/null)
if [[ -z "$newer" ]]; then ok "no source change since the launch build"; else bad "stale build: $newer changed after launch; relaunch"; fi

if "$scripts/in.sh" "$root" git rev-parse --abbrev-ref HEAD >/dev/null 2>&1; then ok "fixture repository on branch $("$scripts/in.sh" "$root" git rev-parse --abbrev-ref HEAD)"; else bad "fixture repository missing"; fi
exit "$failed"
