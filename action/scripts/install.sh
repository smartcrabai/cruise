#!/usr/bin/env bash
# Installs the `cruise` CLI if it is not already on PATH. Uses the official
# cargo-dist installer script (it already resolves the correct target triple
# and download URL per-platform). cruise always runs with `sdk: pi` in this
# action (see setup-env.sh), so no separate `claude` CLI install is needed.
set -euo pipefail

CRUISE_VERSION="${CRUISE_VERSION:-latest}"
INSTALL_DIR="${RUNNER_TEMP:-/tmp}/cruise-bin"
mkdir -p "$INSTALL_DIR"

if command -v cruise >/dev/null 2>&1; then
  echo "cruise: already installed at $(command -v cruise)"
else
  if [ -z "$CRUISE_VERSION" ] || [ "$CRUISE_VERSION" = "latest" ]; then
    installer_url="https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh"
  else
    installer_url="https://github.com/smartcrabai/cruise/releases/download/${CRUISE_VERSION}/cruise-installer.sh"
  fi
  echo "cruise: installing ($CRUISE_VERSION) from $installer_url"
  curl -fsSL "$installer_url" | \
    CRUISE_UNMANAGED_INSTALL="$INSTALL_DIR" \
    CRUISE_NO_MODIFY_PATH=1 \
    CRUISE_DISABLE_UPDATE=1 \
    CRUISE_PRINT_QUIET=1 \
    sh
  echo "$INSTALL_DIR" >> "$GITHUB_PATH"
  export PATH="$INSTALL_DIR:$PATH"
fi

if ! command -v cruise >/dev/null 2>&1; then
  echo "::error::cruise installation failed (not found on PATH)" >&2
  exit 1
fi
cruise --version

if ! command -v gh >/dev/null 2>&1; then
  echo "::error::gh CLI not found on PATH (GitHub-hosted runners include it by default; self-hosted runners must install it)" >&2
  exit 1
fi
