#!/bin/sh
# Stand-in reproduce agent: runs each behavioral item's repro command, records a verdict.
# exit 0 -> not-reproduced; 127 -> blocked (tool missing); any other -> reproduced.
A="$(dirname "$1")/artifacts"; out="$A/analysis/reproduction.md"; : > "$out"
awk '/^### W/{id=$2} /^- contributing findings:/{src=$0} /^- repro:/{print id"\t"src"\t"substr($0, index($0,":")+2)}' "$A/analysis/worklist.md" |
while IFS="$(printf '\t')" read -r id src cmd; do
  case "$src" in *REV-*|*PLAN-*|*WIRE-*) ;; *) continue ;; esac
  cmd=$(printf '%s' "$cmd" | tr -d '`')
  sh -c "$cmd" > /tmp/repro.$$ 2>&1; code=$?
  case $code in 0) v=not-reproduced ;; 127) v=blocked ;; *) v=reproduced ;; esac
  printf '### %s\n- command: `%s`\n- exit: %s\n- observed: %s\n- verdict: %s\n\n' "$id" "$cmd" "$code" "$(head -1 /tmp/repro.$$)" "$v" >> "$out"
done
rm -f /tmp/repro.$$
[ -f ../dirty ] && echo "probe wrote this" > reproduce-scratch.txt
cat "$out"
