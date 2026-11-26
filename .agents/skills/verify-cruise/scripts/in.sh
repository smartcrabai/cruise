#!/usr/bin/env bash
# Run one command inside a verify-cruise run: empty environment plus ROOT/env,
# working directory ROOT/repo. Usage: in.sh ROOT COMMAND [ARGS]...
set -euo pipefail
root=${1:?usage: in.sh ROOT COMMAND [ARGS]...}
shift
[[ -f "$root/env" && -d "$root/repo" ]] || { echo "in.sh: $root is not a verify-cruise run" >&2; exit 2; }
vars=()
while IFS= read -r line; do vars+=("$line"); done <"$root/env"
cd "$root/repo"
exec env -i "${vars[@]}" "$@"
