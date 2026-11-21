#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
exec bash "$ROOT/scripts/formal/run_pinned_alloy.sh" tests/formal/exec_recovery.als exec-recovery-alloy
