#!/bin/bash
set -euo pipefail

# Run only in remote environment
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

# Install tools
cargo install taplo-cli cargo-sort

# Build the project
cargo build
