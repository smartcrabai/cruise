#!/usr/bin/env bash
# Verify vendor/claude-agent-sdk against the published `seher-claude-agent-sdk`
# .crate, and prove that [patch.crates-io] is what local/dist builds resolve
# while `cargo publish` still resolves crates.io.
#
# PROHIBITED.md §5 requires the vendored tree to stay source-identical to the
# published crate; JCODE.md goal 7 requires `cargo install cruise` to keep
# working. Neither was machine-checked: the Cargo.lock `checksum` that pinned
# the registry tarball disappears from the root lockfile once the patch applies.
#
# Checks:
#   1. the [dependencies] pin, the [patch.crates-io] path and the vendored
#      package version agree, and cargo resolves the crate through the path
#      (a version bump without re-extracting vendor/ leaves the patch unused,
#      which cargo reports only as a warning)
#   2. the published .crate matches the sparse-index checksum
#   3. the vendored tree is byte-identical to that .crate, except for the files
#      added on purpose (LICENSE, README.md)
#   4. `cargo package` drops [patch], keeps the `=<version>` crates.io pin, and
#      ships no vendor/ sources; the packaged lockfile still pins the checksum
#      verified in step 2
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
# The vendored manifest and the packaged manifest/lockfile are cargo-normalized,
# so the one-key-per-line layout is stable.
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
  || die "$VENDOR_DIR is not byte-identical to the published .crate (the vendored source must stay unmodified)"

cruise_version="$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | select(.name == "cruise") | .version')"
cargo package --quiet --no-verify --allow-dirty -p cruise
packaged="target/package/cruise-$cruise_version.crate"

manifest="$(tar xzOf "$packaged" "cruise-$cruise_version/Cargo.toml")"
if grep -q '^\[patch' <<< "$manifest"; then
  die "the packaged manifest still carries a [patch] table"
fi
packaged_pin="$(toml_value "[dependencies.$CRATE]" version <<< "$manifest")"
[[ "$packaged_pin" == "=$version" ]] \
  || die "the packaged manifest requests $CRATE '${packaged_pin:-<missing>}', expected '=$version' from crates.io"

lock="$(tar xzOf "$packaged" "cruise-$cruise_version/Cargo.lock")"
packaged_cksum="$(awk -v name="name = \"$CRATE\"" '
  $0 == name { inside = 1; next }
  /^\[\[package\]\]/ { inside = 0 }
  inside && $1 == "checksum" { gsub(/"/, "", $3); print $3; exit }
' <<< "$lock")"
[[ "$packaged_cksum" == "$expected_cksum" ]] \
  || die "the packaged lockfile pins $CRATE checksum '${packaged_cksum:-<missing>}', expected $expected_cksum"

if tar tzf "$packaged" | grep -q '/vendor/'; then
  die "the packaged .crate ships vendor/ sources (Apache-2.0 code under the MIT package license)"
fi

echo "OK: $VENDOR_DIR matches $CRATE $version ($expected_cksum), patch applied, publish resolves crates.io"
