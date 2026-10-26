#!/usr/bin/env bash
# Tool restrictions contract, shared locked toolchain, no SDK credentials needed.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) OPENCODE_PACKAGE=cli-darwin-arm64 ;;
  Linux-x86_64) OPENCODE_PACKAGE=cli-linux-x64-baseline ;;
  *) echo 'BLOCKED: tool restriction specs require Darwin arm64 or Linux x64' >&2; exit 1 ;;
esac
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/tool-restrictions.XXXXXX")
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
  echo 'BLOCKED: pinned toolchain setup failed' | tee -a "$RUN/results.log"
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
# Reuse the lockfile-pinned CLI packages and skip their postinstall downloader.
run_check opencode-cli-setup npm ci --prefix tests/formal/opencode-cli --ignore-scripts
OPENCODE="$ROOT/tests/formal/opencode-cli/node_modules/@opencode/$OPENCODE_PACKAGE/bin/opencode"
# shellcheck disable=SC2016
run_check opencode-cli-version bash -c 'version=$("$1" --version) && printf "%s\n" "$version" && [[ "$version" == "opencode v2.1.4" ]]' _ "$OPENCODE"
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/tool_restrictions.qnt
# shellcheck disable=SC2016
run_check versions bash -c 'node --version && java -version && rustc +1.99.1 --version'
run_check quint-version "$Q" --version
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
for command in typecheck test run verify; do
  run_check "quint-$command-help" "$Q" "$command" --help
done
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261025 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run --seed 20261025 --max-samples 1000 --max-steps 4 --invariant safety --backend typescript --out-itf "$RUN/simulation_{seq}.itf.json" "$MODEL"
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$ROOT/$MODEL" tool_restrictions 4 "$RUN/counterexample.itf.json"
# Alloy 6.2.0 help exec has no --expect flag. Its supported syntax is expect
# 0/1 on each model command, checked independently against the CLI receipt.
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/tool_restrictions.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/tool_restrictions.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check production-bdd cargo +1.99.1 test --locked --all-features --test tool_restrictions_bdd
run_check bdd-negative-controls node scripts/formal/check_tool_restrictions_bdd_failures.cjs "$RUN"
run_check production-cli-config cargo +1.99.1 test --locked --all-features --test tool_restrictions_cli --test tool_restrictions_config
# Includes effective policy, matcher, all three SDK mappings, MCP engine
# filtering, command admission, workflow defaults, calls and after-PR wiring.
run_check production-unit-contracts cargo +1.99.1 test --locked --all-features --lib
run_check opencode-entrypoint-contracts cargo +1.99.1 test --locked --all-features --test opencode_sdk_cli --test opencode_runtime_contract --test opencode_state_contract
run_check sdk-model-bdd cargo +1.99.1 test --locked --all-features --test sdk_model_bdd
# These tests use an isolated local model server, not an account or paid provider.
run_check opencode-real-cli env OPENCODE_REAL_CLI="$OPENCODE" cargo +1.99.1 test --locked --all-features --test opencode_real_cli -- --ignored --nocapture
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Tool restriction reports: $RUN"
exit "$status"
