#!/usr/bin/env bash
# Installs the `cruise` CLI if it is not already on PATH. Uses the official
# cargo-dist installer script (it already resolves the correct target triple
# and download URL per-platform), then rejects a binary older than
# MIN_CRUISE_VERSION.
#
# The floor is not cosmetic: this action generates configs with no `sdk:` line
# and provisions credentials into cruise's own jcode home (see
# provision-jcode.sh), both of which only mean anything to a cruise whose
# default backend is `sdk: jcode`. An older binary would reject the config
# outright or silently resolve a backend that no longer exists, so an old
# binary paired with this action is refused with a message naming both
# versions instead of failing deep inside the run.
#
# The `jcode` binary itself is installed by install-jcode.sh.
set -euo pipefail

MIN_CRUISE_VERSION="0.2.0"
CRUISE_VERSION="${CRUISE_VERSION:-latest}"
INSTALL_DIR="${RUNNER_TEMP:-/tmp}/cruise-bin"
mkdir -p "$INSTALL_DIR"

# True when semantic version $1 is >= $2. Compares the numeric major/minor/
# patch triple only: a pre-release/build suffix ("0.2.0-rc1", cargo-dist's
# own dev builds) is dropped rather than ordered, so a pre-release of the
# floor counts as meeting it. `sort -V` is deliberately avoided (BSD and GNU
# sort disagree on suffix ordering, and this runs on both).
version_at_least() { # $1=have $2=minimum
  local have="${1%%-*}" want="$2"
  local h_major h_minor h_patch w_major w_minor w_patch
  IFS=. read -r h_major h_minor h_patch <<EOF
$have
EOF
  IFS=. read -r w_major w_minor w_patch <<EOF
$want
EOF
  h_major="${h_major:-0}"; h_minor="${h_minor:-0}"; h_patch="${h_patch:-0}"
  case "$h_major$h_minor$h_patch" in
    *[!0-9]*|"") return 1 ;;
  esac
  if [ "$h_major" -ne "$w_major" ]; then [ "$h_major" -gt "$w_major" ]; return; fi
  if [ "$h_minor" -ne "$w_minor" ]; then [ "$h_minor" -gt "$w_minor" ]; return; fi
  [ "$h_patch" -ge "$w_patch" ]
}

if command -v cruise >/dev/null 2>&1; then
  echo "cruise: already installed at $(command -v cruise)"
else
  if [ -z "$CRUISE_VERSION" ] || [ "$CRUISE_VERSION" = "latest" ]; then
    installer_url="https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh"
  else
    installer_url="https://github.com/smartcrabai/cruise/releases/download/${CRUISE_VERSION}/cruise-installer.sh"
  fi
  echo "cruise: installing ($CRUISE_VERSION) from $installer_url"
  if ! curl -fsSL "$installer_url" | \
    CRUISE_UNMANAGED_INSTALL="$INSTALL_DIR" \
    CRUISE_NO_MODIFY_PATH=1 \
    CRUISE_DISABLE_UPDATE=1 \
    CRUISE_PRINT_QUIET=1 \
    sh
  then
    echo "::error::cruise installer failed for version '$CRUISE_VERSION' from $installer_url" >&2
    exit 1
  fi
  echo "$INSTALL_DIR" >> "$GITHUB_PATH"
  export PATH="$INSTALL_DIR:$PATH"
fi

if ! command -v cruise >/dev/null 2>&1; then
  echo "::error::cruise installation failed (not found on PATH)" >&2
  exit 1
fi
version_output="$(cruise --version 2>&1)"
echo "$version_output"
# cargo-dist/clap print "cruise <semver>". Take the first semver-shaped
# token of the "cruise ..." line rather than the first line's last word, so
# an extra stderr line (e.g. an update notice) or a trailing build hash
# ("cruise 0.2.0 (abc1234)") cannot be misread as the version. Shell
# builtins only: this runs on a PATH that need not carry anything but the
# shell itself.
installed_version=""
while IFS= read -r line; do
  case "$line" in
    "cruise "[0-9]*|"cruise v"[0-9]*)
      for word in $line; do
        case "$word" in
          v[0-9]*.[0-9]*.[0-9]*|[0-9]*.[0-9]*.[0-9]*)
            installed_version="${word#v}"
            break
            ;;
        esac
      done
      [ -n "$installed_version" ] && break
      ;;
  esac
done <<< "$version_output"
if [ -z "$installed_version" ]; then
  echo "::error::could not determine the cruise version from \`cruise --version\` output: $version_output" >&2
  exit 1
fi
if ! version_at_least "$installed_version" "$MIN_CRUISE_VERSION"; then
  echo "::error::cruise $installed_version is too old for this version of the action (requires cruise v$MIN_CRUISE_VERSION or later, the first release whose default backend is \`sdk: jcode\`) -- pin a newer 'cruise_version', or pin the action to a ref that matches your binary" >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "::error::gh CLI not found on PATH (GitHub-hosted runners include it by default; self-hosted runners must install it)" >&2
  exit 1
fi
