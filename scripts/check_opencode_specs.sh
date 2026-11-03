#!/usr/bin/env bash
# Reproducible OpenCode V2 specs. No model credentials or installed OpenCode needed.
# Installs and validates the shared pinned toolchain before running checks.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Linux-x86_64) ;;
  *) echo 'BLOCKED: formal toolchain and POSIX BDD require Darwin arm64 or Linux x64' >&2; exit 1 ;;
esac
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/opencode.XXXXXX")
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
if [[ $status != 0 ]]; then
  echo 'BLOCKED: pinned formal toolchain setup failed; cached tools are not a substitute' | tee -a "$RUN/results.log"
  exit 1
fi
if [[ ! -f target/formal-tools/env.sh ]]; then
  echo 'BLOCKED: pinned formal toolchain is not installed' | tee -a "$RUN/results.log"
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/opencode.qnt
# shellcheck disable=SC2016
run_check versions bash -c '[[ $(node --version) == v26.12.1 ]] && node --version && java -version && java -XshowSettings:properties -version 2>&1 | grep -E "java.runtime.version = 25\\.0\\.5\\+8-LTS" && [[ $("$1" --version) == 0.33.0 ]] && "$1" --version && rustc +1.99.1 --version' _ "$Q"
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
for command in typecheck test run verify; do
  run_check "quint-$command-help" "$Q" "$command" --help
done
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check alloy-solvers java -jar target/formal-tools/alloy-6.2.0.jar solvers
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261023 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run --seed 20261023 --max-samples 1000 --max-steps 14 --invariant safety --backend typescript --out-itf "$RUN/simulation_{seq}.itf.json" "$MODEL"
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" opencode 14 "$RUN/counterexample.itf.json"
# 6.2.0 has no --expect CLI option. Explicit expect 0/1 live on every model
# command and are independently compared with the complete receipt below.
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/opencode.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/opencode.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check opencode-bdd cargo +1.99.1 test --locked --all-features --test opencode_bdd
run_check bdd-negative-controls node scripts/formal/check_bdd_failures.cjs "$RUN"
run_check opencode-contracts cargo +1.99.1 test --locked --all-features --test opencode_sdk_cli --test opencode_runtime_contract --test opencode_state_contract --test opencode_config_contract
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "OpenCode reports: $RUN"
exit "$status"
