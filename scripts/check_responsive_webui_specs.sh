#!/usr/bin/env bash
# Responsive shell contract. Every failure remains a failure, including setup.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/responsive-webui.XXXXXX")
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
MODEL=tests/formal/responsive_webui.qnt
run_check node-version node --version
run_check java-version java -version
run_check quint-version "$Q" --version
run_check rust-version rustc +1.99.1 --version
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
for command in typecheck test run verify; do
  run_check "quint-$command-help" "$Q" "$command" --help
done
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check alloy-solvers java -jar target/formal-tools/alloy-6.2.0.jar solvers
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261025 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run --seed 20261025 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety --out-itf "$RUN/simulation_{seq}.itf.json" "$MODEL"
# This helper prints and executes actual quint verify argv on an isolated port.
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" responsive_webui 8 "$RUN/counterexample.itf.json"
# Alloy 6.2.0 help has no --expect option. Use supported command-level expect
# 0 for assertions and expect 1 for witnesses, then validate every receipt.
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/responsive_webui.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/responsive_webui.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check responsive-bdd bash scripts/check_responsive_webui_bdd.sh
run_check webui-regressions cargo +1.99.1 test --locked --all-features --bin cruise-runtime webui::
run_check clippy cargo +1.99.1 clippy --locked --all-targets --all-features
run_check formatting cargo +1.99.1 fmt --check
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Responsive WebUI reports: $RUN"
exit "$status"
