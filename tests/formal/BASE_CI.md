# Base branch CI gate: Quint model

`base_ci.qnt` models the plan's workflow-start admission decision. It covers
fresh start exclusions (exec, resume, after-PR, dry-run, opt-out, unsupported
forge), configured/default branch choice, fail-closed errors and failing CI,
warning-only pending/no-checks, and preventing workspace effects before
admission. `safety` combines these properties. The 12 executable scenarios
exercise each outcome and bypass.

This is an abstract decision model, not a proof of the Rust implementation.
It assumes a single stable classified observation: API parsing, status mapping,
branch resolution failures, CLI versus UI failure persistence, remote races,
and GitHub/GitLab protocol details are not modeled. Dry-run is treated as a
no-workspace terminal outcome for this gate, not a model of all dry-run effects.
It checks safety rather than liveness and allows stuttering. Verification is
bounded, not an unbounded proof.

## CI and pinned toolchain

`bash scripts/check_base_ci_specs.sh` runs all three specifications, logging the exact commands and results to a fresh `target/formal-reports/base-ci.*` directory. It uses `scripts/setup_formal.sh` and the existing npm lockfile: Node 26.13.0, Quint 0.33.0, Apalache 0.62.1 and Temurin JDK 25.0.5+8. The CI simulation uses seed 20261112, 1,000 samples and eight steps. The Apalache bounded verification checks `safety` for eight steps. Neither bounded check proves the implementation correct.

## Reproduction and observed result

On 2026-11-12, macOS arm64, with the existing locked
`tests/formal/package-lock.json` (`@informalsystems/quint` 0.33.0),
Apalache 0.62.1, and Java 27.0.1, the following commands ran from the
repository root. If dependencies are absent, install with
`npm ci --ignore-scripts --prefix tests/formal` first.

```sh
tests/formal/node_modules/.bin/quint typecheck tests/formal/base_ci.qnt
tests/formal/node_modules/.bin/quint test tests/formal/base_ci.qnt --main base_ci --backend typescript --seed 20261112
tests/formal/node_modules/.bin/quint run tests/formal/base_ci.qnt --main base_ci --init init --step step --invariant safety --backend typescript --seed 20261112 --max-samples 1000 --max-steps 4
mkdir -p target/formal-reports/base-ci
node scripts/formal/verify_quint.cjs tests/formal/node_modules/.bin/quint tests/formal/base_ci.qnt base_ci 4 "$PWD/target/formal-reports/base-ci/counterexample.itf.json"
```

All exited 0: typecheck passed, 12 tests passed, seeded simulation found no
violation in 1,000 samples, and Apalache returned `NoError` through four
transitions. The wrapper selected isolated endpoint `127.0.0.1:62148` and
invoked `quint verify --invariant safety --max-steps 4 --main base_ci --init
init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint
127.0.0.1:62148 --out-itf <absolute report path> <absolute model path>`.
The selected port varies by run. Backend version is explicitly pinned by the
existing wrapper, and the fresh endpoint avoids reuse of another server.
