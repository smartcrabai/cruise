# Executable `cruise list` action contract

The two models specify the UI/delegation boundary: entry mode, top-level and nested navigation, stable row identity, creation mode and persistence, default-No consent, Run All availability/start/attach, and re-fetch after returning from a submenu or action. Neither model reimplements daemon policy or proves Rust conformance. The finite models abstract terminal I/O, daemon policy decisions and failure timing. In particular, a Run All availability result is an observation, not a reservation. Confirmation preserves the prompt's intent, but dispatch uses the live batch state: attach-only confirmation returns `noActiveBatch` without starting a batch if the observed batch ended, even with eligible work. Start confirmation attaches instead if a batch became active. Both outcomes return to the picker. Real terminal and persisted-state behavior is checked with `tests/features/list_actions.feature` and the Rust regressions below.

## Reproduce

With Node 26.13.1, Bash, curl and Rust 1.99.1 (including rustfmt and Clippy), run from the repository root:

```sh
bash scripts/check_list_actions_specs.sh
cargo +1.99.1 test --locked --all-features --test list_actions_bdd
cargo +1.99.1 test --locked --all-features --bin cruise-runtime list_cmd::tests
cargo +1.99.1 test --locked --all-features \
  --test list_actions_pty --test list_generate_plan --test planning_spinner_pty \
  --test run_lifecycle_cli --test daemon_list_api --test skip_planning_state \
  --test ui_language_catalog --test ui_language_cli --test ssh_entrypoint
cargo +1.99.1 fmt -- --check
cargo +1.99.1 clippy --all-targets --all-features -- -D warnings -A clippy::large_futures
```

The model script uses the existing `setup_formal.sh`, npm lockfile, and checksum-verified release artifacts: Quint 0.33.0, Apalache 0.62.1, Alloy 6.2.0 and Temurin 25.0.5+8. The supported combinations are Linux x86-64 (CI) and macOS arm64. The production Gherkin runner uses locked Cucumber 0.23.0 and needs macOS or Linux with a working PTY, Git, and permission to run a local daemon and bind a loopback port. No external provider tokens are needed.

The script runs these Quint commands (the verifier helper selects a fresh loopback endpoint and records the full invocation in `quint-verify.log`):

```sh
Q=tests/formal/node_modules/.bin/quint
M=tests/formal/list_actions.qnt
"$Q" typecheck "$M"
"$Q" test "$M" --backend typescript --seed 20261120
"$Q" run --seed 20261120 --max-samples 2000 --max-steps 16 "$M" --backend typescript --invariant safety --verbosity 1
# Effective verify invocation, omitting endpoint and output-report flags:
"$Q" verify --invariant safety --max-steps 12 --main list_actions --init init --step step --backend apalache --apalache-version 0.62.1 "$M"
```

Simulation is sampling, not proof. Apalache checks the invariant up to 12 transitions in this abstraction. Alloy checks bounded traces and executable witnesses. The runner records `java -jar ... help exec`, enumerates `commands`, then executes `java --enable-native-access=ALL-UNNAMED -jar ... exec --command '*' --solver sat4j --output ... --type json tests/formal/list_actions.als`. Alloy 6.2.0 has no `exec --expect` switch. The model declares `expect 0` on all checks and `expect 1` on all witnesses, and the receipt checker rejects missing commands, unexpected counterexamples or absent witnesses. Reports are under `target/formal-reports/list-actions.*/` (including `alloy/output/receipt.json`). `.github/workflows/list-actions-specs.yml` runs all checks on pull requests and main, and uploads the model reports.

## Local verification (November 20, 2026, macOS arm64)

- Quint: typecheck and 14 tests passed. Simulation found no violation for seed `20261120`, capped at 2,000 samples and 16 steps. Apalache returned `NoError` through 12 transitions for `safety`.
- Alloy: all seven assertions returned UNSAT; all 18 intended witnesses returned SAT, including same-label row identity, background creation, Clean refresh, and the Run All attach race. The receipt validation and its 13 unit tests passed. The local bundle is `target/formal-reports/list-actions.acPRMR/`.
- Production: Cucumber passed 11 scenarios and 86 steps; four selector unit tests and 43 related integration tests passed on Rust 1.99.1. An initial Gherkin run caught a mismatch in the populated-inventory step wording and exited nonzero. The matching step was corrected and the scenarios rerun.
- Formatting, strict Clippy, `actionlint` and `shellcheck` passed. Strict Clippy initially flagged an unfulfilled `dead_code` expectation on macOS because the executor unit tests read the sandbox grant fields. The expectation now applies only to builds that do not read those fields.
