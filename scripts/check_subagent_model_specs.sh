#!/usr/bin/env bash
# Reproducible subagent-model contracts. Never require agent credentials or APIs.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Linux-x86_64) ;;
  *) echo 'BLOCKED: subagent specifications require Darwin arm64 or Linux x64' >&2; exit 1 ;;
esac
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/subagent-model.XXXXXX")
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
  echo 'BLOCKED: pinned toolchain installation failed' | tee -a "$RUN/results.log"
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/subagent_model.qnt
# shellcheck disable=SC2016
run_check versions bash -c '[[ $(node --version) == v22.23.3 ]] && node --version && java -version && java -XshowSettings:properties -version 2>&1 | grep -E "java.runtime.version = 25\\.0\\.5\\+8-LTS"'
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
run_check quint-simulation "$Q" run --seed 20261025 --max-samples 1000 --max-steps 12 --invariant safety --backend typescript --out-itf "$RUN/simulation_{seq}.itf.json" "$MODEL"
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" subagent_model 12 "$RUN/counterexample.itf.json"
# Alloy 6.2.0 help exec has no --expect flag. Each named model command declares
# expect 0 (check) or expect 1 (witness). Validate the actual receipt, not exit 0.
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/subagent_model.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/subagent_model.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check subagent-bdd cargo +1.99.1 test --locked --all-features --test subagent_model_bdd
run_check bdd-negative-controls node scripts/formal/check_subagent_bdd_failures.cjs "$RUN"
# Includes Claude env at the actual subprocess boundary and jcode API frames,
# plus defaults, groups, parallel children, workflow-call and review wiring.
run_check subagent-cli cargo +1.99.1 test --locked --all-features --test sdk_model_execution_cli --test subagent_model_jcode_cli
run_check subagent-unit-contracts cargo +1.99.1 test --locked --all-features --lib
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Subagent model reports: $RUN"
exit "$status"
