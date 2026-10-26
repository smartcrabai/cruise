#!/usr/bin/env bash
# Workflow-start auth contract. Shared pins and receipts, no credentials required.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Linux-x86_64) ;;
  *) echo 'BLOCKED: auth specs require the supported formal toolchain and POSIX fixtures' >&2; exit 1 ;;
esac
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/agent-auth.XXXXXX")
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
  echo 'BLOCKED: pinned toolchain setup failed, not a passing cached check' | tee -a "$RUN/results.log"
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/agent_auth.qnt
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
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261024 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run --seed 20261024 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$RUN/simulation_{seq}.itf.json" "$MODEL"
# Run from the report directory so Apalache's own server logs are retained
# under target rather than leaving _apalache-out in the repository root.
# shellcheck disable=SC2016
run_check quint-verify bash -c 'cd "$1" && exec node "$2" "$3" "$4" agent_auth 8 "$5"' _ "$RUN" "$ROOT/scripts/formal/verify_quint.cjs" "$Q" "$ROOT/$MODEL" "$RUN/counterexample.itf.json"
# The pinned CLI help has no --expect option. Its supported equivalent is
# expect 0/1 on model commands, checked against the receipt for EVERY command.
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/agent_auth.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/agent_auth.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check agent-auth-bdd cargo +1.99.1 test --locked --all-features --test agent_auth_bdd
run_check bdd-negative-controls node scripts/formal/check_agent_auth_bdd_failures.cjs "$RUN"
run_check agent-auth-cli cargo +1.99.1 test --locked --all-features --test agent_auth_cli
# Covers injected-time boundaries, credential parsing, candidate resolution,
# shared planning entrypoint, CLI setup and the TUI/WebUI setup_run_blocking path.
run_check production-unit-contracts cargo +1.99.1 test --locked --all-features --lib
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Agent authentication reports: $RUN"
exit "$status"
