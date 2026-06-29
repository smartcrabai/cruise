#!/usr/bin/env bash
# Validate structure and required content of Docker-related config files:
#   Dockerfile, .dockerignore, .github/workflows/docker-publish.yml
#
# Runs from the repository root.
# Usage: bash scripts/test_docker_config.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# Minimal test framework (same style as scripts/test_adhoc_signing.sh)
# ---------------------------------------------------------------------------
PASS=0
FAIL=0

pass() { echo "PASS: $1"; ((PASS++)) || true; }
fail() { echo "FAIL: $1 -- $2"; ((FAIL++)) || true; }

assert_file_exists() {
  local label="$1" path="$2"
  if [[ -e "$path" ]]; then pass "$label"; else fail "$label" "file not found: $path"; fi
}

assert_contains() {
  local label="$1" path="$2" pattern="$3"
  if [[ ! -e "$path" ]]; then
    fail "$label" "file not found: $path"
    return
  fi
  if grep -qF -- "$pattern" "$path"; then pass "$label"; else fail "$label" "pattern not found in $path: $pattern"; fi
}

assert_not_contains() {
  local label="$1" path="$2" pattern="$3"
  if [[ ! -e "$path" ]]; then
    fail "$label" "file not found: $path"
    return
  fi
  if ! grep -qF -- "$pattern" "$path"; then pass "$label"; else fail "$label" "forbidden pattern found in $path: $pattern"; fi
}

assert_line_count() {
  local label="$1" path="$2" pattern="$3" expected="$4"
  if [[ ! -e "$path" ]]; then
    fail "$label" "file not found: $path"
    return
  fi
  local actual
  actual=$(grep -cF "$pattern" "$path" || true)
  if [[ "$actual" -eq "$expected" ]]; then
    pass "$label"
  else
    fail "$label" "expected $expected occurrences of '$pattern' in $path, got $actual"
  fi
}

# ---------------------------------------------------------------------------
# Dockerfile
# ---------------------------------------------------------------------------
echo ""
echo "=== Dockerfile ==="

assert_file_exists "Dockerfile exists" "Dockerfile"

# Single-stage: exactly one FROM line (no cargo-chef builder stage)
assert_line_count "Dockerfile has exactly one FROM line (single-stage)" "Dockerfile" "FROM " 1

# Base image
assert_contains "Dockerfile uses oven/bun:1-slim as base" "Dockerfile" "FROM oven/bun:1-slim"

# Multi-arch support
assert_contains "Dockerfile declares ARG TARGETARCH" "Dockerfile" "ARG TARGETARCH"

# gh CLI version pin
assert_contains "Dockerfile declares ARG GH_VERSION" "Dockerfile" "ARG GH_VERSION"

# Binary is staged per-arch via ARG TARGETARCH, not hardcoded target/ path
assert_contains "Dockerfile copies binary via bin/\${TARGETARCH}/cruise" "Dockerfile" 'bin/${TARGETARCH}/cruise'
assert_not_contains "Dockerfile does not copy from target/ directly" "Dockerfile" "COPY target/"

# Non-root runtime user
assert_contains "Dockerfile sets USER bun" "Dockerfile" "USER bun"

# Entrypoint
assert_contains "Dockerfile sets ENTRYPOINT [\"cruise\"]" "Dockerfile" 'ENTRYPOINT ["cruise"]'

# Workdir
assert_contains "Dockerfile sets WORKDIR /work" "Dockerfile" "WORKDIR /work"

# claude installation via bun
assert_contains "Dockerfile installs claude via BUN_INSTALL=/usr/local bun install -g" \
  "Dockerfile" "BUN_INSTALL=/usr/local bun install -g @anthropic-ai/claude-code"

# gh installation via dpkg --print-architecture (arch-agnostic)
assert_contains "Dockerfile installs gh via dpkg --print-architecture" \
  "Dockerfile" "dpkg --print-architecture"

# Antipatterns absent
assert_not_contains "Dockerfile has no cargo-chef reference" "Dockerfile" "cargo-chef"
assert_not_contains "Dockerfile has no cargo build in RUN layer" "Dockerfile" "cargo build"

# ---------------------------------------------------------------------------
# .dockerignore
# ---------------------------------------------------------------------------
echo ""
echo "=== .dockerignore ==="

assert_file_exists ".dockerignore exists" ".dockerignore"

assert_contains ".dockerignore excludes .git" ".dockerignore" ".git"
assert_contains ".dockerignore excludes target" ".dockerignore" "target"
assert_contains ".dockerignore excludes node_modules" ".dockerignore" "node_modules"
assert_contains ".dockerignore excludes src-tauri/target" ".dockerignore" "src-tauri/target"

# ---------------------------------------------------------------------------
# .github/workflows/docker-publish.yml
# ---------------------------------------------------------------------------
echo ""
echo "=== .github/workflows/docker-publish.yml ==="

WORKFLOW=".github/workflows/docker-publish.yml"

assert_file_exists "docker-publish.yml exists" "$WORKFLOW"

# Registry and image
assert_contains "workflow targets ghcr.io" "$WORKFLOW" "ghcr.io"
assert_contains "workflow image name is smartcrabai/cruise" "$WORKFLOW" "smartcrabai/cruise"

# Multi-arch platforms
assert_contains "workflow builds linux/amd64,linux/arm64" "$WORKFLOW" "linux/amd64,linux/arm64"

# Permissions
assert_contains "workflow grants packages: write" "$WORKFLOW" "packages: write"
assert_contains "workflow grants contents: read" "$WORKFLOW" "contents: read"

# Job structure: build-binary must exist and docker must depend on it
assert_contains "workflow has build-binary job" "$WORKFLOW" "build-binary:"
assert_contains "workflow docker job depends on build-binary" "$WORKFLOW" "needs: build-binary"

# Matrix: both architectures present
assert_contains "build-binary matrix includes amd64" "$WORKFLOW" "amd64"
assert_contains "build-binary matrix includes arm64" "$WORKFLOW" "arm64"

# Native ARM runner (no cross-compilation)
assert_contains "build-binary uses ubuntu-24.04-arm runner for ARM" "$WORKFLOW" "ubuntu-24.04-arm"

# Cargo build: bin cruise only, not --workspace (would pull in GUI crates)
assert_contains "cargo build uses --bin cruise flag" "$WORKFLOW" "--bin cruise"
assert_not_contains "cargo build does not use --workspace" "$WORKFLOW" "--workspace"

# Artifact handoff between jobs
assert_contains "workflow uploads binaries as artifacts" "$WORKFLOW" "upload-artifact"
assert_contains "workflow downloads binaries as artifacts" "$WORKFLOW" "download-artifact"

# Tag trigger pattern matches release.yml convention
assert_contains "workflow triggers on semver tags" "$WORKFLOW" "[0-9]+.[0-9]+.[0-9]+"

# Authenticate with GITHUB_TOKEN (no PAT required)
assert_contains "workflow uses GITHUB_TOKEN for ghcr.io login" "$WORKFLOW" "secrets.GITHUB_TOKEN"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "Results: $PASS passed, $FAIL failed"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
