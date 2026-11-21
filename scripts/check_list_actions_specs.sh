#!/usr/bin/env bash
# Run the pinned model checks and retain command transcripts and solver evidence.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports || exit 1
RUN=$(mktemp -d "$ROOT/target/formal-reports/list-actions.XXXXXX") || exit 1
status=0
run_check() {
  local name=$1 result
  shift
  printf '%q ' "$@" >> "$RUN/commands.log"
  printf '\n' >> "$RUN/commands.log"
  echo "== $name"
  "$@" 2>&1 | tee "$RUN/$name.log"
  result=$?
  printf '%s: %s\n' "$name" "$result" >> "$RUN/results.log"
  if [[ $result != 0 ]]; then status=1; fi
}
run_check setup bash scripts/setup_formal.sh
if [[ $status != 0 ]]; then
  echo "BLOCKED: pinned formal toolchain installation failed. Reports: $RUN" >&2
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh || exit 1
Q=tests/formal/node_modules/.bin/quint
MODEL=tests/formal/list_actions.qnt
run_check quint-version "$Q" --version
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261120
run_check quint-simulation "$Q" run --seed 20261120 --max-samples 2000 --max-steps 16 "$MODEL" --backend typescript --invariant safety --verbosity 1
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" list_actions 12 "$RUN/counterexample.itf.json"
run_check alloy bash scripts/check_list_actions_alloy.sh "$RUN"
run_check receipt-validator node --test scripts/formal/check_alloy_receipt.test.cjs
echo "List action specification reports: $RUN"
exit "$status"
