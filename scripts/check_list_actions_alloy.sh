#!/usr/bin/env bash
# Invoked by check_list_actions_specs.sh after checksum-pinned setup_formal.sh.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
RUN="${1:?Provide an absolute report directory}/alloy"
mkdir "$RUN"
JAR="$ROOT/target/formal-tools/alloy-6.2.0.jar"
MODEL="$ROOT/tests/formal/list_actions.als"
java -jar "$JAR" version | tee "$RUN/version.log"
java -jar "$JAR" help exec > "$RUN/help-exec.log" 2>&1
java -jar "$JAR" commands "$MODEL" > "$RUN/commands.log" 2>&1
# The released CLI has no exec --expect flag. The explicit expect clauses in
# the model and this receipt check enforce UNSAT for checks and SAT for runs.
java --enable-native-access=ALL-UNNAMED -jar "$JAR" exec --command '*' --solver sat4j --output "$RUN/output" --type json "$MODEL" 2>&1 | tee "$RUN/exec.log"
node "$ROOT/scripts/formal/check_alloy_receipt.cjs" "$MODEL" "$RUN/output/receipt.json" | tee "$RUN/receipt-check.log"
