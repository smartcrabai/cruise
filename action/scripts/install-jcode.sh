#!/usr/bin/env bash
# Installs the `jcode` CLI, the binary cruise drives for its default
# `sdk: jcode` backend. Uses jcode's own installer (it resolves the target
# triple, picks the matching release asset and verifies its SHA-256 against
# the published SHA256SUMS), the same shape as install.sh's cargo-dist
# installer for cruise.
#
# The upstream installer is a Bash script and writes its launcher state below
# `$HOME` (and shell startup files) even when `JCODE_INSTALL_DIR` is set. Keep
# that state in the runner temp area: the action must not touch the runner
# user's jcode installation or shell configuration.
#
# JCODE_NO_TELEMETRY=1 is a requirement, not a preference: an embedded jcode
# must not report anything, for the install or for the runs cruise makes
# afterwards (cruise sets it again for every jcode invocation of its own).
# JCODE_SKIP_SERVER_RELOAD=1 keeps the installer from signalling a jcode
# daemon: there is none on a runner, and reloading a self-hosted runner's own
# daemon is not this action's business.
#
# jcode's minimum version is enforced by cruise itself at run time (it
# verifies the `jcode run --ndjson` event shape it was built against), so
# this step only pins and installs.
set -euo pipefail

JCODE_VERSION_INPUT="${JCODE_VERSION:-latest}"
INSTALL_ROOT="${RUNNER_TEMP:-/tmp}"
INSTALL_DIR="$INSTALL_ROOT/jcode-bin"
INSTALL_HOME="$INSTALL_ROOT/jcode-install-home"
mkdir -p "$INSTALL_DIR" "$INSTALL_HOME"

if command -v jcode >/dev/null 2>&1; then
  echo "jcode: already installed at $(command -v jcode)"
else
  installer_url="https://jcode.sh/install"
  # jcode publishes one installer for every release and takes the pin through
  # JCODE_VERSION (empty = resolve the newest release itself), so unlike
  # cruise's per-release installer there is no version to put in the URL.
  if [ -z "$JCODE_VERSION_INPUT" ] || [ "$JCODE_VERSION_INPUT" = "latest" ]; then
    pinned_version=""
  else
    # jcode's installer validates the pin against its release-tag form
    # ("v0.81.1"); a bare semver would die on its tag check with an opaque
    # "Failed to determine latest version", so normalize it here.
    case "$JCODE_VERSION_INPUT" in
      v*) pinned_version="$JCODE_VERSION_INPUT" ;;
      *) pinned_version="v$JCODE_VERSION_INPUT" ;;
    esac
  fi
  echo "jcode: installing ($JCODE_VERSION_INPUT) from $installer_url"
  if ! curl -fsSL "$installer_url" | \
    HOME="$INSTALL_HOME" \
    LOCALAPPDATA="$INSTALL_HOME/LocalAppData" \
    XDG_CONFIG_HOME="$INSTALL_HOME/xdg-config" \
    JCODE_HOME="$INSTALL_HOME/jcode-home" \
    JCODE_VERSION="$pinned_version" \
    JCODE_INSTALL_DIR="$INSTALL_DIR" \
    JCODE_NO_TELEMETRY=1 \
    JCODE_SKIP_SERVER_RELOAD=1 \
    bash
  then
    echo "::error::jcode installer failed for version '$JCODE_VERSION_INPUT' from $installer_url" >&2
    exit 1
  fi
  echo "$INSTALL_DIR" >> "$GITHUB_PATH"
  export PATH="$INSTALL_DIR:$PATH"
fi

if ! command -v jcode >/dev/null 2>&1; then
  echo "::error::jcode installation failed (not found on PATH)" >&2
  exit 1
fi
