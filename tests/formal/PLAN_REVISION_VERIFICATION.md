# Plan revision verification, 2026-11-12

The Quint and Alloy checks below were run locally after the offer-model changes. Reports and traces are under `target/formal-reports/plan-revision-reviewfix-final2.2rq9iD/`.

Commands run:

```sh
bash scripts/setup_formal.sh
source target/formal-tools/env.sh
RUN=$(mktemp -d target/formal-reports/plan-revision-reviewfix-final2.XXXXXX)
Q="$PWD/tests/formal/node_modules/.bin/quint"
"$Q" typecheck tests/formal/plan_revision.qnt
"$Q" test tests/formal/plan_revision.qnt --backend typescript --seed 20261112 --out-itf "$RUN/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261112 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$RUN/simulation_{seq}.itf.json" tests/formal/plan_revision.qnt
node scripts/formal/verify_quint.cjs "$Q" "$PWD/tests/formal/plan_revision.qnt" plan_revision 8 "$RUN/counterexample.itf.json"
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/plan_revision.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/plan_revision.als "$RUN/alloy/receipt.json"
```

Setup completed. Quint typecheck and `quint test` exited successfully. Seeded simulation found no violation in 1,000 samples of up to eight transitions. Apalache bounded verification reported `NoError` and no violation through eight transitions. Alloy receipt validation passed the `PlanningOffersMatchContract` assertion (UNSAT as expected) and all five SAT witnesses.

The production Gherkin target, its negative controls, and the complete `check_plan_revision_specs.sh` runner were not run. No BDD or aggregate-suite result is claimed here.
