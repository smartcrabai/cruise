#!/usr/bin/env bash
# Check the bounded exec recovery protocol and retain each tool's raw report.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports || exit 1
RUN=$(mktemp -d "$ROOT/target/formal-reports/exec-recovery-quint.XXXXXX") || exit 1
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
finish() {
  printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
  echo "Reports: $RUN"
  exit "$status"
}
run_check setup bash scripts/setup_formal.sh
if [[ $status != 0 || ! -f target/formal-tools/env.sh ]]; then
  echo 'Pinned setup failed or did not produce env.sh; refusing to use stale tools.' | tee -a "$RUN/results.log" >&2
  status=1
  finish
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh || { status=1; finish; }
QUINT="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL="$ROOT/tests/formal/exec_recovery.qnt"
run_check typecheck "$QUINT" typecheck "$MODEL"
run_check test "$QUINT" test "$MODEL" --main exec_recovery --backend typescript --seed 20261121 --out-itf "$RUN/test_{test}_{seq}.itf.json"
for test in replayTest failedAgainTest hardPendingTest livePauseTest interruptedCompletionTest handledTest invalidTest unsavedTest; do
  if ! grep -Fq "ok $test passed" "$RUN/test.log"; then
    echo "Missing Quint test result: $test" | tee -a "$RUN/results.log"
    status=1
  fi
done
run_check simulation "$QUINT" run "$MODEL" --main exec_recovery --init init --step step --invariant safety --backend typescript --seed 20261121 --max-samples 1000 --max-steps 30 --out-itf "$RUN/simulation_{seq}.itf.json"
run_check verify node scripts/formal/verify_quint.cjs "$QUINT" "$MODEL" exec_recovery 15 "$RUN/counterexample.itf.json"
finish
