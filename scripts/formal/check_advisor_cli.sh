#!/usr/bin/env bash
# Called from the repository root after building both production binaries.
set -euo pipefail
report=${1:?missing report directory}
scratch=$(mktemp -d "$PWD/target/formal-tools/advisor-cli.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
version=1.14.0
archive="$scratch/bats.tar.gz"
curl --fail --location --silent --show-error --retry 3 \
  "https://codeload.github.com/bats-core/bats-core/tar.gz/refs/tags/v$version" -o "$archive"
node -e '
  const { createHash } = require("node:crypto");
  const { readFileSync } = require("node:fs");
  const digest = createHash("sha256").update(readFileSync(process.argv[1])).digest("hex");
  if (digest !== "bb537b70b15b732f6d8827dd6578e3d8ce166636ce1f18ea9a074184fcce9177") {
    throw Error(`Bats release checksum mismatch: ${digest}`);
  }
' "$archive"
tar -xzf "$archive" -C "$scratch"
bats="$scratch/bats-core-$version/bin/bats"
"$bats" --version
target=$(cargo +1.99.1 metadata --locked --no-deps --format-version 1 |
  node -p 'JSON.parse(require("node:fs").readFileSync(0)).target_directory')
export CRUISE_BIN="$target/debug/cruise"
printf 'CRUISE_BIN=%q %q %q\n' "$CRUISE_BIN" "$bats" tests/advisor_cli.bats >> "$report/commands.log"
"$bats" tests/advisor_cli.bats
