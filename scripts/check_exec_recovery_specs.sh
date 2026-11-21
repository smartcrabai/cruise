#!/usr/bin/env bash
# Run the independent checks even if a different check is blocked.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports || exit 1
REPORT=$(mktemp -d "$ROOT/target/formal-reports/exec-recovery.XXXXXX") || exit 1
status=0
run_check() {
  local name=$1 result
  shift
  { printf '%q ' "$@"; printf '\n'; } >> "$REPORT/commands.log"
  echo "== $name"
  "$@" 2>&1 | tee "$REPORT/$name.log"
  result=$?
  printf '%s: %s\n' "$name" "$result" >> "$REPORT/results.log"
  if [[ $result != 0 ]]; then status=1; fi
}
run_check quint bash scripts/check_exec_recovery_quint.sh
run_check alloy bash scripts/check_exec_recovery_alloy.sh
run_check gherkin bash scripts/check_exec_recovery_bdd.sh
printf 'Aggregate exit status: %s\nReports: %s\n' "$status" "$REPORT"
exit "$status"
