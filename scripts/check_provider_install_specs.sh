#!/usr/bin/env bash
# Standalone, test-only provider install contract and model checks.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/provider-install.XXXXXX")
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
}
run_check setup bash scripts/setup_formal.sh
if [[ $status != 0 || ! -f target/formal-tools/env.sh ]]; then
  echo "BLOCKED: pinned formal toolchain setup failed; reports: $RUN" >&2
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/provider_install.qnt
run_check versions bash -c 'node --version && java -version && "$1" --version' _ "$Q"
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --main provider_install --backend typescript --seed 20261023 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run "$MODEL" --main provider_install --init init --step step --invariant safety --backend typescript --seed 20261023 --max-samples 1000 --max-steps 14 --out-itf "$RUN/simulation_{seq}.itf.json"
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" provider_install 14 "$RUN/counterexample.itf.json"
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check alloy-solvers java -jar target/formal-tools/alloy-6.2.0.jar solvers
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/provider_install.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/provider_install.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check provider-install-unit cargo +1.99.1 test --locked --lib provider_install::
run_check provider-install-bdd cargo +1.99.1 test --locked --test provider_install_bdd
run_check provider-install-cli cargo +1.99.1 test --locked --test provider_install_cli --test provider_resolution_cli
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Provider install reports: $RUN"
exit "$status"
