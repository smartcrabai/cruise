#!/usr/bin/env bash
# Fresh evidence only. setup_formal.sh checks the repository's pinned tools.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/plan-verify.XXXXXX")
status=0
run_check() {
  local name=$1 result
  shift
  echo "== $name"
  printf '%q ' "$@" >> "$RUN/commands.log"
  printf '\n' >> "$RUN/commands.log"
  "$@" > "$RUN/$name.log" 2>&1
  result=$?
  cat "$RUN/$name.log"
  printf '%s: %s\n' "$name" "$result" >> "$RUN/results.log"
  if [[ $result != 0 ]]; then status=1; fi
}
run_check setup bash scripts/setup_formal.sh
if [[ $status != 0 || ! -f target/formal-tools/env.sh ]]; then
  echo 'BLOCKED: pinned formal setup failed' | tee -a "$RUN/results.log"
  exit 1
fi
source target/formal-tools/env.sh
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL="$ROOT/tests/formal/plan_verify.qnt"
run_check versions bash -c 'node --version; "$1" --version; java -version; rustc +1.99.1 --version' _ "$Q"
run_check alloy-version java -jar "$ROOT/target/formal-tools/alloy-6.2.0.jar" version
run_check alloy-exec-help java -jar "$ROOT/target/formal-tools/alloy-6.2.0.jar" help exec
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261107 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run --seed 20261107 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript "$MODEL"
verify_model() { (cd "$RUN" && node "$ROOT/scripts/formal/verify_quint.cjs" "$Q" "$MODEL" plan_verify 8 "$RUN/counterexample.itf.json"); }
run_check quint-verify verify_model
run_check alloy java -jar "$ROOT/target/formal-tools/alloy-6.2.0.jar" exec --command '*' --solver sat4j --type json --output "$RUN/alloy" "$ROOT/tests/formal/plan_verify.als"
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/plan_verify.als "$RUN/alloy/receipt.json"
export XDG_CONFIG_HOME="$RUN/xdg-config" XDG_DATA_HOME="$RUN/xdg-data" XDG_STATE_HOME="$RUN/xdg-state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME"
run_check plan-verify-bdd cargo +1.99.1 test --locked --all-features --test plan_verify_bdd
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Plan verification reports: $RUN"
exit "$status"
