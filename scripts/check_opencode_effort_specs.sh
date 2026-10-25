#!/usr/bin/env bash
# Common OpenCode effort + no-route contract. Local fixtures, no secrets/providers.
# All dependencies come from setup_formal.sh, npm/Cargo locks and explicit pins.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) OPENCODE_PACKAGE=cli-darwin-arm64 ;;
  Linux-x86_64) OPENCODE_PACKAGE=cli-linux-x64-baseline ;;
  *) echo 'BLOCKED: effort specs require Darwin arm64 or Linux x64' >&2; exit 1 ;;
esac
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/opencode-effort.XXXXXX")
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
MODEL=tests/formal/opencode_effort.qnt
# Install only lockfile-integrity-checked packages. Do NOT execute the CLI's
# postinstall, which can fetch unlocked fallback packages. Select the supported
# native package directly (baseline glibc on Linux, no AVX2 requirement).
run_check opencode-cli-setup npm ci --prefix tests/formal/opencode-cli --ignore-scripts
OPENCODE="$ROOT/tests/formal/opencode-cli/node_modules/@opencode/$OPENCODE_PACKAGE/bin/opencode"
# shellcheck disable=SC2016
run_check opencode-cli-version bash -c 'version=$("$1" --version) && printf "%s\n" "$version" && [[ "$version" == "opencode v2.1.4" ]]' _ "$OPENCODE"
# shellcheck disable=SC2016
run_check versions bash -c '[[ $(node --version) == v22.22.0 ]] && node --version && java -version && java -XshowSettings:properties -version 2>&1 | grep -E "java.runtime.version = 25\\.0\\.5\\+8-LTS" && [[ $("$1" --version) == 0.33.0 ]] && "$1" --version && rustc +1.99.1 --version' _ "$Q"
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
for command in typecheck test run verify; do
  run_check "quint-$command-help" "$Q" "$command" --help
done
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check alloy-solvers java -jar target/formal-tools/alloy-6.2.0.jar solvers
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261024 --out-itf "$RUN/test_{test}_{seq}.itf.json"
# Simulation is supplementary sampling, NOT proof.
run_check quint-simulation "$Q" run --seed 20261024 --max-samples 1000 --max-steps 12 --invariant safety --backend typescript --out-itf "$RUN/simulation_{seq}.itf.json" "$MODEL"
# Wrapper logs actual verify argv and selects isolated Apalache 0.62.1.
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" opencode_effort 12 "$RUN/counterexample.itf.json"
# Alloy 6.2.0 help exec has NO --expect flag. Every selected model command
# declares expect 0 (assertion) / expect 1 (witness), checked against its receipt.
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/opencode_effort.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/opencode_effort.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check opencode-effort-bdd cargo +1.99.1 test --locked --all-features --test opencode_bdd
run_check bdd-negative-controls node scripts/formal/check_bdd_failures.cjs "$RUN"
run_check opencode-unit-contracts cargo +1.99.1 test --locked --all-features --lib backend::opencode::tests::
run_check retry-unit-contracts cargo +1.99.1 test --locked --all-features --lib retry::
run_check opencode-entrypoint-contracts cargo +1.99.1 test --locked --all-features --test opencode_sdk_cli --test opencode_runtime_contract --test opencode_state_contract --test opencode_config_contract
run_check opencode-real-cli env OPENCODE_REAL_CLI="$OPENCODE" cargo +1.99.1 test --locked --all-features --test opencode_real_cli -- --ignored --nocapture
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "OpenCode effort reports: $RUN"
exit "$status"
