#!/usr/bin/env bash
# Build once, create an isolated Cruise home and fixture repository, start a
# daemon owned by this run, and print the run root (ROOT) on stdout.
# Usage: .agents/skills/verify-cruise/scripts/launch.sh [--no-build]
set -euo pipefail
repo_root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$repo_root"
if [[ "${1:-}" != --no-build ]]; then
  cargo +1.99.1 build --locked --all-features --bin cruise --bin cruise-runtime --example verify_pty >&2
fi
root=$(mktemp -d "${TMPDIR:-/tmp}/verify-cruise.XXXXXX")
root=$(cd "$root" && pwd -P)
mkdir -p "$root/home/config/cruise" "$root/home/data" "$root/home/state" "$root/repo"
# Only these variables reach Cruise; in.sh starts from an empty environment.
cat >"$root/env" <<EOF
PATH=$repo_root/target/debug:$repo_root/target/debug/examples:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin
HOME=$root/home
XDG_CONFIG_HOME=$root/home/config
XDG_DATA_HOME=$root/home/data
XDG_STATE_HOME=$root/home/state
JCODE_HOME=$root/home/jcode
LANG=en_US.UTF-8
TERM=xterm-256color
CRUISE_UI_LANGUAGE=en
CRUISE_DISABLE_NOTIFICATIONS=1
CRUISE_DISABLE_HERDR=1
GIT_CONFIG_NOSYSTEM=1
GIT_CONFIG_GLOBAL=/dev/null
GIT_TERMINAL_PROMPT=0
GIT_AUTHOR_NAME=verify-cruise
GIT_AUTHOR_EMAIL=verify-cruise@example.invalid
GIT_COMMITTER_NAME=verify-cruise
GIT_COMMITTER_EMAIL=verify-cruise@example.invalid
EOF
printf '%s\n' "$repo_root" >"$root/source"
git -C "$repo_root" rev-parse HEAD >"$root/source-head"
# Base CI checks would call the forge for the fixture remote.
printf 'check_base_ci: false\n' >"$root/home/config/cruise/default.yaml"
run_in() { "$repo_root/.agents/skills/verify-cruise/scripts/in.sh" "$root" "$@"; }
run_in git init -q -b main
run_in git commit -q --allow-empty -m base
cp "$repo_root/.agents/skills/verify-cruise/fixtures/"*.yaml "$root/"
run_in cruise daemon start --port 0 >"$root/daemon-start.log" 2>&1
run_in cruise daemon status --json >"$root/daemon-status.json"
printf '%s\n' "$root"
