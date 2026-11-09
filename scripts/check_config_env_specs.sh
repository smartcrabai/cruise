#!/usr/bin/env bash
# Test-only bounded workflow environment resolution model.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Linux-x86_64) ;;
  *) echo 'BLOCKED: config env specifications require Darwin arm64 or Linux x64' >&2; exit 1 ;;
esac
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/config-env.XXXXXX")
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
MODEL=tests/formal/config_env_override.qnt
run_check versions bash -c '[[ $(node --version) == v26.13.0 ]] && node --version && java -version'
run_check quint-version "$Q" --version
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --main config_env_override --backend typescript --seed 20261107 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run "$MODEL" --main config_env_override --init init --step step --invariant safety --backend typescript --seed 20261107 --max-samples 1000 --max-steps 8 --out-itf "$RUN/simulation_{seq}.itf.json"
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" config_env_override 8 "$RUN/counterexample.itf.json"
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/config_env_override.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/config_env_override.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check gherkin cargo +1.99.1 test --locked --all-features --test config_env_bdd
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Config env model reports: $RUN"
exit "$status"
