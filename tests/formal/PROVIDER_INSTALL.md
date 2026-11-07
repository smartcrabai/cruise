# Provider install lifecycle model

The independent `provider_install.als` model checks resolution, publication,
lock and failure invariants over ten ordered states with SAT4J, and requires
satisfiable stale-reinstall and failed-replacement witnesses. The runner invokes
Alloy 6.2.0 with explicit `expect 0/1` commands and validates every entry in its
fresh JSON receipt because the Alloy CLI exit code alone does not enforce
expectations. The Alloy model has the same abstraction limits as the Quint model.

`provider_install.qnt` describes the planned provider-install contract, not the
current implementation or a refinement proof. Run `bash scripts/check_provider_install_specs.sh`.
The runner uses the shared pinned tool setup, saves logs under a fresh
`target/formal-reports/provider-install.*` directory, tests scenarios, simulates
1,000 traces of at most 14 steps, and requests bounded Apalache verification at
14 steps. Simulation is not proof and bounded verification cannot establish
unbounded correctness or liveness.

The tool setup pins Node 26.12.1, Quint 0.33.0, Apalache 0.62.1,
Temurin JDK 25.0.5+8, Alloy 6.2.0 and Rust 1.99.1. The JDK and Alloy
release artifacts are checked against the SHA-256 values in
`scripts/setup_formal.sh`. The runner records exact argv and all results in
`commands.log` and `results.log`, including the actual Apalache command in
`quint-verify.log`. Alloy's 6.2.0 CLI uses model-level `expect 0` and
`expect 1` rather than an `--expect` flag.

The executable Gherkin suite runs the production `cruise` executable with
isolated local fixtures. Its strict runner fails on skipped or undefined steps
and failed assertions. The same CI job also runs the existing provider CLI
integration tests. These tests do not download upstream provider binaries.

The model covers override (including invalid override) over managed over PATH,
missing and damaged/stale records, locked staging, digest and version probe,
publication, failure preservation, and reinstall. `safety` combines the six
planned invariants and transition checks for lock-gated publication and failed
install preservation. The model has one provider and one installation actor;
publication abstracts both filesystem rename and atomic record switch. It does
not establish filesystem atomicity, crash recovery, concurrent process
behavior, archive safety, digest correctness, or conformance of Rust code. A
published binary's successful checks are distinct from the current staging
candidate's checks. Damage describes the record's usability rather than
retroactively changing the historical publication checks.
