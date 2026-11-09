# WebUI daemon executable specifications

Run `bash scripts/check_webui_daemon_specs.sh` from the repository root. The
secret-free `.github/workflows/webui-daemon-specs.yml` runs the same check on
pull requests and pushes to main, and uploads `target/formal-reports/` even
when a check fails. Results and exact commands are recorded per run. CI has
not been executed by a local run.

The shared `scripts/setup_formal.sh` installs Node-locked Quint 0.33.0 via
`npm ci`, checksum-verifies Alloy 6.2.0 (SHA-256
`6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`)
and Temurin 25.0.5+8, and selects Apalache 0.62.1. Node 26.13.0 and Rust
1.99.1 are required. Cucumber 0.23.0 is pinned in Cargo.lock. Alloy 6.2.0's
`help exec` specifies `--command`; expectations are Alloy model `expect 0`
and `expect 1` clauses, not an unsupported CLI `--expect` flag.

The runner executes these commands, where `Q` is
`tests/formal/node_modules/.bin/quint`, `R` is a fresh report directory,
and `PORT` is a fresh loopback port selected by the verification helper:

```sh
bash scripts/setup_formal.sh
source target/formal-tools/env.sh
$Q typecheck tests/formal/webui_daemon.qnt
$Q test tests/formal/webui_daemon.qnt --backend typescript --seed 20261025 --out-itf "$R/test_{test}_{seq}.itf.json"
$Q run --seed 20261025 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety --out-itf "$R/simulation_{seq}.itf.json" tests/formal/webui_daemon.qnt
$Q verify --invariant safety --max-steps 8 --main webui_daemon --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/webui_daemon.qnt
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/webui_daemon.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/webui_daemon.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test webui_daemon_bdd
cargo +1.99.1 test --locked --all-features --test webui_daemon --test webui_daemon_control
```

The actual verification command is invoked by `scripts/formal/verify_quint.cjs`,
which records its complete argv and allocates the port. Quint test discovery is
explicitly checked for all seven named tests because Quint otherwise succeeds
with zero tests. Cucumber drives the real CLI and HTTP server, and fails on
skipped/undefined steps and failed assertions.

The Quint model uses two PIDs and two process identities, a single registry
slot and serialized starts. It checks ownership, identity-checked signalling,
status and unchanged sessions. Simulation is not proof. Apalache checks the
conjunction of safety invariants to eight transitions, not unbounded behavior.
Alloy checks four assertions and two witnesses in exactly six ordered states
(five transitions), with SAT4J. These are abstractions, not a refinement proof
of Rust or OS behavior. They do not model HTTP token comparison, real locks,
timeouts, filesystem durability, Windows APIs, or liveness. HTTP authorization
and actual lifecycle behavior are exercised separately by production tests.

Local run on 2026-11-07: see `target/formal-reports/` for the full logs and
receipt. Four Alloy assertions should be UNSAT and both witnesses SAT. A
successful receipt validation is required, rather than relying on Alloy's exit
code. The runner reports setup failure as a blocker, not a successful check.
