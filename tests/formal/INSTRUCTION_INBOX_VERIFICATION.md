# Instruction inbox verification evidence

Executed on 2026-11-07 from the repository root on macOS arm64:

- `bash scripts/check_instruction_inbox_specs.sh`: exit 0, report `target/formal-reports/instruction-inbox.ieXoNL/`. Setup, Quint typecheck, five executable tests, seeded simulation and bounded verification all passed. Exact expanded commands and individual exit codes are in `commands.log` and `results.log`.
- The runner uses Quint 0.33.0, Node 26.13.0 and Temurin 25.0.5+8. The simulation uses the TypeScript backend, seed `20261023`, at most 100 samples and 12 steps. Simulation is not proof.
- The runner invokes `quint verify --invariant safety --max-steps 4` using Apalache 0.62.1 through `scripts/formal/verify_quint.cjs`. It returned `NoError`, with no violation at the four-step bound. The wrapper logs its complete actual command, including the explicitly pinned backend and fresh server endpoint. Bounded verification is not an unbounded proof or evidence of implementation refinement.
- `bash scripts/check_instruction_inbox_alloy.sh`: exit 0, report `target/formal-reports/instruction-inbox-alloy.JaWxKD/`. Alloy 6.2.0 with SAT4J returned six UNSAT assertions and one SAT `CarryWitness`. The receipt validator checked every expected result and the witness instance. Alloy scopes are seven states, three IDs and two keys. See `INSTRUCTION_INBOX_ALLOY.md` for the pinned archive checksum and limitations.

- `cargo +1.99.1 test --locked --all-features --test instruction_inbox_bdd`: exit 0, three scenarios and 15 steps passed using Rust 1.99.1 and Cucumber 0.23.0. It invokes the real CLI and engine with a deterministic local agent executable.

Both runners are exercised by the `instruction-inbox-specs` job in `.github/workflows/ci.yml`. They require the pinned tools and supported host architecture rather than accepting missing tools as a pass. The CI job itself and its Linux host were not executed locally.

## Tool invocations

The runners execute the following commands, with `Q` set to the pinned `tests/formal/node_modules/.bin/quint`, `MODEL` to the absolute model path, and `RUN` to the generated report directory:

```sh
"$Q" typecheck "$MODEL"
"$Q" test "$MODEL" --main instruction_inbox --backend typescript --seed 20261023 --out-itf "$RUN/test_{test}_{seq}.itf.json"
"$Q" run "$MODEL" --main instruction_inbox --init init --step step --invariant safety --backend typescript --seed 20261023 --max-samples 100 --max-steps 12 --out-itf "$RUN/simulation_{seq}.itf.json"
node scripts/formal/verify_quint.cjs "$Q" "$MODEL" instruction_inbox 4 "$RUN/counterexample.itf.json"
```

The wrapper invokes Quint's `verify --invariant safety --max-steps 4 --main instruction_inbox --init init --step step --backend apalache --apalache-version 0.62.1` with a fresh server endpoint and the model path. The Alloy runner executes the pinned Java and jar with `help exec`, then `exec --command '*' --solver sat4j --output "$RUN/output" --type json` and the Alloy model path, followed by `node scripts/formal/check_alloy_receipt.cjs` on that model and receipt. The receipt states: Partition, OneLease, ClosedAcceptance, Terminal, Dedupe and CloseRecovery are UNSAT as expected, and CarryWitness is SAT as expected.
