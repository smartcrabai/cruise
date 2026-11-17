# Base CI gate Alloy model

Run from the repository root:

```sh
bash scripts/check_base_ci_alloy.sh
node --test scripts/formal/check_alloy_receipt.test.cjs
```

The first command verifies the released Alloy 6.2.0 distribution jar (SHA-256 `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`) and Temurin JDK 25.0.5+8 archive against the platform-specific SHA-256 values pinned in the script, including on cached downloads. Supported hosts are macOS arm64 and Linux x86_64. Bash, curl, tar, Node.js and sha256sum or shasum are required. The runner extracts a fresh verified JDK and checks its runtime version. It records `java -jar "$JAR" help exec` and `java -jar "$JAR" commands "$MODEL"`. The inspected 6.2.0 help describes wildcard command selection, output directories and receipt generation. The execution and receipt checks are:

```sh
"$JAVA" --enable-native-access=ALL-UNNAMED -jar "$JAR" exec --command '*' --solver sat4j --output "$RUN/output" --type json "$MODEL"
node scripts/formal/check_alloy_receipt.cjs "$MODEL" "$RUN/output/receipt.json"
```

Every check explicitly expects 0 and every witness explicitly expects 1. The receipt validator rejects missing, extra or malformed results, counterexamples, and witnesses without concrete instances. Logs, receipt and witness instances are saved in a fresh `target/formal-reports/base-ci-alloy.*` directory.

The model considers up to four independent invocations and covers CLI/UI, GitHub/GitLab/unsupported forge, opt-out and lifecycle bypasses, CLI dry run, configured/default branch selection, and abstract status/error outcomes. A non-rejected non-dry invocation is abstracted as creating a workspace. The policy is encoded as facts, so several assertions restate that policy and test its internal consistency rather than independently proving the implementation. It does not model CLI parsing, API status classification, warnings' visibility, forge discovery failures, defaults loading, scheduling, concrete workspace operations, or a refinement relation to Rust. It is bounded specification evidence, not a proof of the running program.

On 2026-11-12, `bash scripts/check_base_ci_alloy.sh` exited 0 with six UNSAT assertions and five SAT witnesses, validated by the receipt checker. `node --test scripts/formal/check_alloy_receipt.test.cjs` exited 0 with 13 tests passing. The local run's reports are in `target/formal-reports/base-ci-alloy.1tWexq/`.
