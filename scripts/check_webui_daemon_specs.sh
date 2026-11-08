#!/usr/bin/env bash
# Fail on every failed check while preserving separate evidence logs.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/webui-daemon.XXXXXX")
status=0
run_check() {
  local name=$1
  shift
  echo "== $name"
  printf '%q ' "$@" >> "$RUN/commands.log"
  printf '\n' >> "$RUN/commands.log"
  "$@" > "$RUN/$name.log" 2>&1
  local result=$?
  cat "$RUN/$name.log"
  printf '%s: %s\n' "$name" "$result" >> "$RUN/results.log"
  if [[ $result != 0 ]]; then status=1; fi
  return 0
}
run_check setup bash scripts/setup_formal.sh
if [[ $status != 0 || ! -f target/formal-tools/env.sh ]]; then
  echo 'BLOCKED: pinned formal toolchain setup failed' | tee -a "$RUN/results.log"
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/webui_daemon.qnt
run_check node-version node --version
run_check java-version java -version
run_check quint-version "$Q" --version
run_check rust-version rustc +1.99.1 --version
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261025 --out-itf "$RUN/test_{test}_{seq}.itf.json"
# Quint exits zero when no tests are discovered. Require each contract witness.
for test in daemonRoundTripTest bindFailureTest stalePidReuseTest staleReplacementTest signalMatchingIdentityTest restartRoundTripTest parentHoldsLockTest; do
  if ! grep -Fq "ok $test passed" "$RUN/quint-test.log"; then
    echo "Missing Quint test result: $test" | tee -a "$RUN/results.log"
    status=1
  fi
done
run_check quint-simulation "$Q" run --seed 20261025 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety --out-itf "$RUN/simulation_{seq}.itf.json" "$MODEL"
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" webui_daemon 8 "$RUN/counterexample.itf.json"
# Alloy 6.2.0 expresses expectations in the model, not via an exec flag.
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/webui_daemon.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/webui_daemon.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check daemon-bdd cargo +1.99.1 test --locked --all-features --test webui_daemon_bdd
run_check daemon-integration cargo +1.99.1 test --locked --all-features --test webui_daemon --test webui_daemon_control
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "WebUI daemon reports: $RUN"
exit "$status"
