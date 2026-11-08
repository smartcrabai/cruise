#!/usr/bin/env bash
# Independent, receipt-backed locale specification and integration checks.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/ui-locale.XXXXXX")
status=0
run_check() {
  local name=$1 result
  shift
  echo "== $name"
  printf '%q ' "$@" >> "$RUN/commands.log"
  printf '\n' >> "$RUN/commands.log"
  "$@" > "$RUN/$name.log" 2>&1
  result=$?
  cat "$RUN/$name.log"
  printf '%s: %s\n' "$name" "$result" >> "$RUN/results.log"
  if [[ $result != 0 ]]; then status=1; fi
}
run_check setup bash scripts/setup_formal.sh
if [[ -f target/formal-tools/env.sh ]]; then
  # shellcheck disable=SC1091
  source target/formal-tools/env.sh
fi
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/ui_locale.qnt
run_check versions bash -c 'node --version && java -version && "$1" --version && rustc +1.99.1 --version' _ "$Q"
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261023 --out-itf "$RUN/test_{test}_{seq}.itf.json"
run_check quint-simulation "$Q" run "$MODEL" --invariant safety --backend typescript --seed 20261023 --max-samples 1000 --max-steps 5
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" ui_locale 5 "$RUN/counterexample.itf.json"
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/ui_locale.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/ui_locale.als "$RUN/alloy/receipt.json"
run_check locale-bdd cargo +1.99.1 test --locked --test ui_locale_bdd
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "UI locale reports: $RUN"
exit "$status"
