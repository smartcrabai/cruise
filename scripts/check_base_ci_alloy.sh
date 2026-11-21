#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
exec bash "$ROOT/scripts/formal/run_pinned_alloy.sh" tests/formal/base_ci_gate.als base-ci-alloy
