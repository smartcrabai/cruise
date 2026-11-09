# Batch auto-requeue model

`auto_requeue.qnt` models one session in a batch with a fixed retry limit.
Run `bash scripts/check_auto_requeue_specs.sh` from the repository root. The
runner uses the shared pinned toolchain, runs Quint typechecking, executable
tests, seeded simulation and an eight-transition Apalache bounded verification
of `safety`. It records commands, per-check output and exit statuses under a
fresh `target/formal-reports/auto-requeue.*` directory. The dedicated
`auto-requeue-specs` CI job runs the same checks without secrets and uploads
the evidence. The shared setup pins Node 26.13.0, Quint 0.33.0, Apalache
0.62.1, Temurin JDK 25.0.5+8, and Alloy 6.2.0 (SHA-256
`6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`).

`auto_requeue.als` independently models nine states and eight transitions.
Six assertions expect no counterexample and eight witnesses must be
satisfiable, including actual retry to completion and lowered-budget cases.
The Alloy 6.2.0 CLI uses `expect 0` and `expect 1` in the model rather than a
CLI `--expect` flag. The runner selects every command explicitly and validates
every expectation against the generated JSON receipt. Seeded simulation uses
seed 20261106, 1000 samples and at most eight steps. Simulation is not proof,
and Apalache verification is bounded to eight steps.

The runner records exact commands and generated paths in `commands.log`. Its
core commands (with `$Q` set to `tests/formal/node_modules/.bin/quint`, `$RUN`
to its fresh report directory, and the setup-generated Java environment) are:

```sh
"$Q" typecheck tests/formal/auto_requeue.qnt
"$Q" test tests/formal/auto_requeue.qnt --backend typescript --seed 20261106 --out-itf "$RUN/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261106 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$RUN/simulation_{seq}.itf.json" tests/formal/auto_requeue.qnt
node scripts/formal/verify_quint.cjs "$Q" "$PWD/tests/formal/auto_requeue.qnt" auto_requeue 8 "$RUN/counterexample.itf.json"
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/auto_requeue.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/auto_requeue.als "$RUN/alloy/receipt.json"
cargo +1.99.1 test --locked --test auto_requeue_bdd
cargo +1.99.1 test --locked --test auto_requeue_state_contract
```

The verification wrapper invokes `quint verify --invariant safety --max-steps
8 --main auto_requeue --init init --step step --backend apalache
--apalache-version 0.62.1` with an isolated server endpoint and the model.
The report logs its full actual argument vector.
The Gherkin scenarios use the built Cruise executable and isolated on-disk
configuration and sessions. Cucumber's `fail_on_skipped` and `run_and_exit`
make undefined, pending, skipped and failing steps fail the check.

Local execution on 2026-11-06 passed Quint typechecking, nine executable tests,
simulation and bounded verification, and the Alloy receipt validated six
UNSAT assertions and eight SAT witnesses. The full runner did not complete:
the native Cargo invocation waited on an unrelated shared build-directory lock,
and an independent clean build timed out while compiling dependencies. A
separate cached-dependency Cucumber execution passed six scenarios, but used a
prebuilt binary whose exact source provenance was not established. This is not
a substitute for a successful clean Cargo/CI run.

The model checks the budget, eligibility and increment of automatic retries,
that other actions cannot increase the counter, reset clears it, batch starts
only accept Planned or Suspended, and manual retry preserves it. Tests cover
success, exhaustion, disabled retry, ineligible and cancelled errors, manual
retry and reset, and a lowered limit. The latter separately checks that a
persisted count above a newly lowered limit prevents another retry, including
in the main bounded state space. The invariant only requires the count to be
within the limit immediately after an automatic retry.

The initial batch limit and count range from 0 to 2, with eight transitions in
bounded verification. Error classes, persistence and cancellation are abstract
state, not actual Rust errors or filesystem operations. Manual retry clears the
batch flag, preventing automatic retry on a later manual failure. There is no
proof of implementation refinement, persistence durability, concurrency,
liveness, or unbounded behavior. Production regression tests are needed for
those integration boundaries.
