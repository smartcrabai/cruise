# Local merge specification execution, 2026-11-06

**Historical results:** The execution below predates the durable-intent changes to the models and does not verify the current models or implementation. The combined verification requested for the updated work remains outstanding.

Run from the repository root with Node 26.13.0 and Rust 1.99.1:

```sh
CARGO_BUILD_BUILD_DIR="$PWD/target" PATH="$(mise where node@26.13.0)/bin:$PATH" bash scripts/check_local_merge_specs.sh
```

The Cargo override was needed locally because an unrelated worktree held the shared Cargo build-directory lock. It is not required by CI. The suite installs the repository's locked Quint 0.33.0, verifies the checksum of Alloy 6.2.0 (SHA-256 `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`), and uses checksum-verified Temurin 25.0.5+8. Apalache 0.62.1 is selected explicitly. The Alloy CLI help was captured: this version uses model-level `expect 0` and `expect 1`, not a CLI `--expect` option.

The suite's exact per-check commands, output and statuses are captured in `target/formal-reports/local-merge.DkSTlR/`. With `Q=tests/formal/node_modules/.bin/quint` and `R=target/formal-reports/local-merge.DkSTlR`, its principal commands are:

```sh
"$Q" typecheck tests/formal/local_merge.qnt
"$Q" test tests/formal/local_merge.qnt --backend typescript --seed 20261106 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261106 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/local_merge.qnt
# verify_quint.cjs selects a fresh loopback port, then runs:
"$Q" verify --invariant safety --max-steps 8 --main local_merge --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint 127.0.0.1:61156 --out-itf "$R/counterexample.itf.json" tests/formal/local_merge.qnt
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/local_merge.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/local_merge.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test local_merge_bdd
```

The loopback port is allocated dynamically on each run. Quint typecheck and six executable tests passed. Seeded simulation found no violation in 1,000 samples with eight steps. Apalache found no violation within eight transitions. Simulation is not proof, and bounded verification is not an unbounded proof or a refinement proof of Rust. Alloy's receipt recorded eight UNSAT assertion checks and seven SAT witnesses, all matching their expectations. The receipt validator's 13 tests passed. Separately, `cargo test --test local_merge_bdd` passed seven scenarios and 34 steps. The DkSTlR all-features suite was interrupted after 600 seconds during native dependency compilation, so it is not a passing aggregate run. See its `results.log` and `local-merge-bdd.log` for the partial evidence.

A further run without the Cargo override (`PATH="$(mise where node@26.13.0)/bin:$PATH" bash scripts/check_local_merge_specs.sh`) passed the formal checks again but was stopped while waiting for an unrelated process to release the shared Cargo build lock. Its partial evidence is under `target/formal-reports/local-merge.CTTetd/`. The all-features CI command remains unverified locally, and no aggregate suite pass is claimed.

The revised models include intent persistence failure, interruption after commit, retry, and rejection of a mismatch. They abstract parent, tree and operation-token matching as a nondeterministic outcome, assume atomic durable persistence, model cleanup atomically, and do not model concurrent Git changes, hook behavior, or prove refinement to Rust. After interruption before commit, a retry can restore a verified squash checkout and clear the intent; mismatched checkout state is refused and requires manual recovery. The production tests exercise real Git in temporary repositories. CI integration is in `.github/workflows/ci.yml`, job `local-merge-specs`, with report artifacts uploaded on failure as well as success. No remote CI run is claimed.
