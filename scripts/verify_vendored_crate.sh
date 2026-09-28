#!/usr/bin/env bash
# Verify vendor/claude-agent-sdk against the upstream `seher-claude-agent-sdk`
# .crate archive and prove that [patch.crates-io] makes local builds resolve
# the checked-in source copy.
#
# The vendored tree must stay source-identical to the upstream archive. The
# Cargo.lock registry checksum disappears from the root lockfile once the
# patch applies, so this script checks the archive checksum separately.
#
# Checks:
#   1. the [dependencies] pin, the [patch.crates-io] path and vendored package
#      version agree, and cargo resolves the crate through the path
#   2. the upstream .crate matches the sparse-index checksum
#   3. the vendored tree is byte-identical to that archive, except for the
#      files added on purpose (LICENSE, README.md)
#
# Requirements: cargo, curl, jq, tar, diff, sha256sum or shasum
# Usage: bash scripts/verify_vendored_crate.sh

set -euo pipefail

CRATE="seher-claude-agent-sdk"
VENDOR_DIR="vendor/claude-agent-sdk"
# Files intentionally added to the vendored tree; absent from the .crate.
ADDED_FILES=(LICENSE README.md)

cd "$(dirname "${BASH_SOURCE[0]}")/.."

die() {
  echo "::error::$*" >&2
  exit 1
}

sha256() {
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

# Value of `<key> = "<value>"` inside a TOML table, without pulling in a parser.
# The vendored manifest is cargo-normalized, so the one-key-per-line layout
# is stable.
toml_value() {
  local table="$1" key="$2"
  awk -v table="$table" -v key="$key" '
    $0 == table { inside = 1; next }
    /^\[/ { inside = 0 }
    inside && $1 == key { gsub(/"/, "", $3); print $3; exit }
  '
}

version="$(toml_value '[package]' version < "$VENDOR_DIR/Cargo.toml")"
[[ -n "$version" ]] || die "cannot read the package version from $VENDOR_DIR/Cargo.toml"

grep -qxF "$CRATE = \"=$version\"" Cargo.toml \
  || die "Cargo.toml [dependencies] must pin $CRATE = \"=$version\" (the vendored version)"
grep -qxF "$CRATE = { path = \"$VENDOR_DIR\" }" Cargo.toml \
  || die "Cargo.toml [patch.crates-io] must point $CRATE at $VENDOR_DIR"

# `cargo tree -i` prints a manifest directory only for path packages, so this
# fails both when the patch went unused and when it was dropped outright.
cargo tree -i "$CRATE" | grep -qE "^$CRATE v$version \(.*/$VENDOR_DIR\)\$" \
  || die "$CRATE $version does not resolve to $VENDOR_DIR -- [patch.crates-io] is not in effect"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

index_url="https://index.crates.io/${CRATE:0:2}/${CRATE:2:2}/$CRATE"
entry="$(curl -sSfL "$index_url" | jq -c --arg version "$version" 'select(.vers == $version)')"
[[ -n "$entry" ]] || die "$CRATE $version is not in the sparse index ($index_url)"
expected_cksum="$(jq -r '.cksum' <<< "$entry")"
if [[ "$(jq -r '.yanked' <<< "$entry")" == "true" ]]; then
  echo "note: $CRATE $version is yanked upstream; the vendored copy is what keeps builds working"
fi

crate_file="$tmp/$CRATE-$version.crate"
curl -sSfL -o "$crate_file" "https://static.crates.io/crates/$CRATE/$CRATE-$version.crate"
actual_cksum="$(sha256 "$crate_file")"
[[ "$actual_cksum" == "$expected_cksum" ]] \
  || die "downloaded .crate sha256 $actual_cksum does not match the sparse-index cksum $expected_cksum"

tar xzf "$crate_file" -C "$tmp"
cp -R "$VENDOR_DIR" "$tmp/vendored"
for file in "${ADDED_FILES[@]}"; do
  rm "$tmp/vendored/$file" \
    || die "$VENDOR_DIR/$file is missing (Apache-2.0 license text and provenance note are required)"
done
diff -r "$tmp/$CRATE-$version" "$tmp/vendored" \
  || die "$VENDOR_DIR is not byte-identical to the upstream .crate (the vendored source must stay unmodified)"


echo "OK: $VENDOR_DIR matches upstream $CRATE $version ($expected_cksum), and [patch.crates-io] resolves to the checked-in copy"
