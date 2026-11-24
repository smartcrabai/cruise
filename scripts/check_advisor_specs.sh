#!/usr/bin/env bash
# Local R14 contract runner. No provider credentials or external agent required.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/advisor.XXXXXX")
echo "Advisor contract reports: $RUN"
status=0
run_check() {
  local name=$1 result=0
  shift
  printf '%q ' "$@" >> "$RUN/commands.log"
  printf '\n' >> "$RUN/commands.log"
  "$@" > "$RUN/$name.log" 2>&1 || result=$?
  cat "$RUN/$name.log"
  printf '%s: %s\n' "$name" "$result" >> "$RUN/results.log"
  if [[ $result != 0 ]]; then status=1; fi
}
run_check setup bash scripts/setup_formal.sh
if [[ $status != 0 || ! -f target/formal-tools/env.sh ]]; then
  echo 'BLOCKED: pinned formal toolchain setup failed' | tee -a "$RUN/results.log"
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/advisor.qnt
ALLOY=(java -jar target/formal-tools/alloy-6.2.0.jar)
run_check node-version node --version
run_check java-version java -version
run_check quint-version "$Q" --version
run_check rust-version rustc +1.99.1 --version
run_check alloy-version "${ALLOY[@]}" version
run_check alloy-exec-help "${ALLOY[@]}" help exec
run_check alloy-solvers "${ALLOY[@]}" solvers
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261124
run_check quint-simulation "$Q" run "$MODEL" --backend typescript --seed 20261124 \
  --max-samples 1000 --max-steps 8 --invariant safety
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" advisor 8 "$RUN/counterexample.itf.json"
# Alloy 6.2.0 has no exec --expect option. The named commands declare
# expect 0/1 in the model, and the receipt check enforces the actual results.
run_check alloy "${ALLOY[@]}" exec --command '*' --solver sat4j --repeat 1 \
  --type json --output "$RUN/alloy" tests/formal/advisor.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/advisor.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
BDD=(cargo +1.99.1 test --locked --all-features --test advisor_bdd -- --color never)
run_check bdd "${BDD[@]}"

# Deliberately wrong expectation, undefined step, and an empty selection. Each
# control must reach Cucumber and fail for the stated reason, not merely fail
# to compile, select the fixture, or launch the binary.
cat > "$RUN/wrong-expectation.feature" <<'GHERKIN'
Feature: Advisor assertion negative control
  Scenario: Ordinary jcode preflight must not be reported as rejected
    Given a jcode workflow without an advisor
    When production preflight runs with advisor "flag-off"
    Then preflight reports "rejected"
GHERKIN
cat > "$RUN/undefined-step.feature" <<'GHERKIN'
Feature: Advisor undefined step negative control
  Scenario: An unbound contract cannot pass
    Then deliberately undefined advisor contract
GHERKIN
negative() {
  local name=$1 marker=$2 context=$3 result=0
  shift 3
  printf '%q ' "${BDD[@]}" "$@" >> "$RUN/commands.log"
  printf '\n' >> "$RUN/commands.log"
  "${BDD[@]}" "$@" > "$RUN/bdd-negative-$name.log" 2>&1 || result=$?
  if [[ $result == 101 ]] && grep -Fq "$marker" "$RUN/bdd-negative-$name.log" \
    && grep -Fq "$context" "$RUN/bdd-negative-$name.log"; then
    printf 'bdd-negative-%s: expected failure confirmed\n' "$name" | tee -a "$RUN/results.log"
  else
    cat "$RUN/bdd-negative-$name.log"
    printf 'bdd-negative-%s: expected contract failure but got exit %s\n' "$name" "$result" | tee -a "$RUN/results.log"
    status=1
  fi
}
negative assertion '1 step failed' 'advisor preflight rejection mismatch' --input "$RUN/wrong-expectation*.feature"
negative undefined '1 step failed' 'deliberately undefined advisor contract' --input "$RUN/undefined-step*.feature"
negative empty 'no advisor scenarios executed' '0 steps' --name '^NO_SUCH_ADVISOR_SCENARIO$'
run_check advisor-unit cargo +1.99.1 test --locked --all-features --lib advisor
run_check advisor-builtin cargo +1.99.1 test --locked --all-features \
  --test builtin_materialization_contract tdd_materializes_one_opt_in_advisor_pass -- --exact
run_check advisor-schema cargo +1.99.1 test --locked --all-features --test model_schedule_schema
run_check cli-build cargo +1.99.1 build --locked --all-features --bin cruise --bin cruise-runtime
if [[ ! -f "$RUN/cli-build.log" ]] || ! grep -qx 'cli-build: 0' "$RUN/results.log"; then
  echo 'BLOCKED: CLI regression binaries failed to build' | tee -a "$RUN/results.log"
else
  run_check cli-regression bash scripts/formal/check_advisor_cli.sh "$RUN"
fi
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
exit "$status"
