#!/usr/bin/env bash
# Workflow-flag specifications. All tools share the repository's exact pins.
# No credentials or external model services. Any missing prerequisite fails.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Linux-x86_64) ;;
  *) echo 'BLOCKED: workflow-flag checks require supported formal tools and a POSIX PTY' >&2; exit 1 ;;
esac
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/workflow-flags.XXXXXX")
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
MODEL=tests/formal/workflow_flags.qnt
# shellcheck disable=SC2016
run_check versions bash -c '[[ $(node --version) == v22.23.3 ]] && node --version && [[ $("$1" --version) == 0.33.0 ]] && "$1" --version && java -version && java -XshowSettings:properties -version 2>&1 | grep -E "java.runtime.version = 25\\.0\\.5\\+8-LTS" && rustc +1.99.1 --version' _ "$Q"
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
for command in typecheck test run verify; do
  run_check "quint-$command-help" "$Q" "$command" --help
done
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check alloy-solvers java -jar target/formal-tools/alloy-6.2.0.jar solvers
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261024 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run --seed 20261024 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$RUN/simulation_{seq}.itf.json" "$MODEL"
# Keep Apalache's own auxiliary files inside this fresh evidence directory.
verify_flags() {
  (cd "$RUN" && node "$ROOT/scripts/formal/verify_quint.cjs" "$Q" "$ROOT/$MODEL" workflow_flags 8 "$RUN/counterexample.itf.json")
}
run_check quint-verify verify_flags
# Separate model for this task. Keep the older run-boundary model intact.
SKIP_MODEL=tests/formal/flag_skip_selection.qnt
run_check skip-quint-typecheck "$Q" typecheck "$SKIP_MODEL"
run_check skip-quint-test "$Q" test "$SKIP_MODEL" --backend typescript --seed 20261026 --out-itf "$RUN/skip_test_{test}_{seq}.itf.json"
run_check skip-quint-simulation "$Q" run --seed 20261026 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$RUN/skip_simulation_{seq}.itf.json" "$SKIP_MODEL"
verify_skip_selection() {
  (cd "$RUN" && node "$ROOT/scripts/formal/verify_quint.cjs" "$Q" "$ROOT/$SKIP_MODEL" flag_skip_selection 8 "$RUN/skip-counterexample.itf.json")
}
run_check skip-quint-verify verify_skip_selection
# The installed Alloy 6.2.0 help has no --expect CLI option. Use its documented
# per-command expect 0/1 syntax and validate every result from the CLI receipt.
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/workflow_flags.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/workflow_flags.als "$RUN/alloy/receipt.json"
run_check skip-alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/skip-alloy" tests/formal/flag_skip_selection.als
run_check skip-alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/flag_skip_selection.als "$RUN/skip-alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
# Do not inherit the developer's cruise provider/model defaults or saved history.
# Leave HOME, credentials and commit-guard variables untouched. Test fixtures
# provide their own backends and use narrower XDG overrides where needed.
export XDG_CONFIG_HOME="$RUN/xdg-config"
export XDG_DATA_HOME="$RUN/xdg-data"
export XDG_STATE_HOME="$RUN/xdg-state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME"
printf 'XDG_CONFIG_HOME=%q\nXDG_DATA_HOME=%q\nXDG_STATE_HOME=%q\n' \
  "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" > "$RUN/test-environment.log"
run_check workflow-flags-bdd cargo +1.99.1 test --locked --all-features --test workflow_flags_bdd
run_check bdd-negative-controls node scripts/formal/check_workflow_flags_bdd_failures.cjs "$RUN"
run_check flag-contracts cargo +1.99.1 test --locked --all-features --test flags_contract --test flags_cli --test flag_skip_list_cli
# Application creation/history, shared pruning/group metadata, TUI navigation and
# WebUI real routes plus approval helper tests live in these production targets.
run_check production-units cargo +1.99.1 test --locked --all-features --lib --bin cruise-runtime
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Workflow flag reports: $RUN"
exit "$status"
