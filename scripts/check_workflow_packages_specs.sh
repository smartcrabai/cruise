#!/usr/bin/env bash
# Workflow package specifications, using the repository's exact formal tool pins.
# Missing prerequisites fail rather than treating cached evidence as a pass.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Linux-x86_64) ;;
  *) echo 'BLOCKED: workflow package checks require supported formal tools' >&2; exit 1 ;;
esac
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/workflow-packages.XXXXXX")
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
  echo 'BLOCKED: pinned formal setup failed, cached results are not a pass' | tee -a "$RUN/results.log"
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/workflow_packages.qnt
run_check versions bash -c '[[ $(node --version) == v26.13.0 ]] && node --version && [[ $("$1" --version) == 0.33.0 ]] && "$1" --version && java -version && java -XshowSettings:properties -version 2>&1 | grep -E "java.runtime.version = 25\\.0\\.5\\+8-LTS" && rustc +1.99.1 --version' _ "$Q"
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
for command in typecheck test run verify; do
  run_check "quint-$command-help" "$Q" "$command" --help
done
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check alloy-solvers java -jar target/formal-tools/alloy-6.2.0.jar solvers
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261107 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run --seed 20261107 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$RUN/simulation_{seq}.itf.json" "$MODEL"
verify_packages() {
  (cd "$RUN" && node "$ROOT/scripts/formal/verify_quint.cjs" "$Q" "$ROOT/$MODEL" workflow_packages 8 "$RUN/counterexample.itf.json")
}
run_check quint-verify verify_packages
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/workflow_packages.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/workflow_packages.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
export XDG_CONFIG_HOME="$RUN/xdg-config"
export XDG_DATA_HOME="$RUN/xdg-data"
export XDG_STATE_HOME="$RUN/xdg-state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME"
printf 'XDG_CONFIG_HOME=%q\nXDG_DATA_HOME=%q\nXDG_STATE_HOME=%q\n' \
  "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" > "$RUN/test-environment.log"
run_check workflow-packages-bdd cargo +1.99.1 test --locked --all-features --test workflow_packages_bdd
run_check workflow-packages-cli cargo +1.99.1 test --locked --all-features --test workflow_packages_cli
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Workflow package reports: $RUN"
exit "$status"
