#!/usr/bin/env bash
# Run production CLI/daemon recovery scenarios using the locked Cucumber runner.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
case $(uname -s) in
  Darwin|Linux) ;;
  *) echo 'BLOCKED: exec recovery requires a POSIX PTY' >&2; exit 1 ;;
esac
for tool in bats jq curl expect git cargo; do
  command -v "$tool" >/dev/null || { echo "BLOCKED: $tool is required" >&2; exit 1; }
done
bats_help=$(bats --help)
[[ $bats_help == *--filter-tags* ]] || { echo 'BLOCKED: Bats must support --filter-tags' >&2; exit 1; }
# CI may provide its freshly built production CLI. Otherwise use Cargo's binary.
if [[ -n ${CRUISE_BIN:-} ]]; then
  [[ -x $CRUISE_BIN && -f $CRUISE_BIN ]] || { echo "BLOCKED: not an executable CLI: $CRUISE_BIN" >&2; exit 1; }
fi
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/exec-recovery-bdd.XXXXXX")
echo "Exec recovery report: $RUN/cucumber.log"
{
  bats --version
  cargo +1.99.1 --version
  cargo +1.99.1 test --locked --all-features --test exec_recovery_bdd
} 2>&1 | tee "$RUN/cucumber.log"
