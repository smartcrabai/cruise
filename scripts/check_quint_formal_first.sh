#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p target/formal-reports
report=$(mktemp -d target/formal-reports/quint-formal-first.XXXXXX)
quint=tests/formal/node_modules/.bin/quint
model=tests/formal/quint_formal_first.qnt
export NO_COLOR=1
{
  "$quint" --version
  "$quint" typecheck "$model"
  "$quint" test "$model" --backend typescript --seed 57 --out-itf "$report/test_{test}_{seq}.itf.json"
  "$quint" run "$model" --backend typescript --seed 57 --max-samples 1000 --max-steps 16 --invariant invSafety --verbosity 1
  node scripts/formal/render_quint.cjs "$report" tests/formal
} 2>&1 | tee "$report/quint-check.log"
