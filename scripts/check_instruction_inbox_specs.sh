#!/usr/bin/env bash
# Check the bounded inbox protocol independently and retain each report.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/instruction-inbox.XXXXXX")
status=0
run_check() {
  local name=$1 result
  shift
  printf '%q ' "$@" >> "$RUN/commands.log"
  printf '\n' >> "$RUN/commands.log"
  echo "== $name"
  "$@" > "$RUN/$name.log" 2>&1
  result=$?
  cat "$RUN/$name.log"
  printf '%s: %s\n' "$name" "$result" >> "$RUN/results.log"
  if [[ $result != 0 ]]; then status=1; fi
}
run_check setup bash scripts/setup_formal.sh
if [[ $status != 0 || ! -f target/formal-tools/env.sh ]]; then
  echo 'Pinned setup failed or did not produce env.sh; refusing to run checks with stale tools.' | tee -a "$RUN/results.log" >&2
  echo "Reports: $RUN"
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
QUINT="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL="$ROOT/tests/formal/instruction_inbox.qnt"
run_check typecheck "$QUINT" typecheck "$MODEL"
run_check test "$QUINT" test "$MODEL" --main instruction_inbox --backend typescript --seed 20261023 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check simulation "$QUINT" run "$MODEL" --main instruction_inbox --init init --step step --invariant safety --backend typescript --seed 20261023 --max-samples 100 --max-steps 12 --out-itf "$RUN/simulation_{seq}.itf.json"
run_check verify node scripts/formal/verify_quint.cjs "$QUINT" "$MODEL" instruction_inbox 4 "$RUN/counterexample.itf.json"
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Reports: $RUN"
exit "$status"
