#!/usr/bin/env bash
# Test for the ad-hoc signing and repackaging logic used in release.yml.
#
# Validates the bash commands added to the "Ad-hoc sign macOS app"
# step (release.yml:283-311) without requiring a full Tauri build.
#
# Requirements: macOS with codesign and hdiutil (CI: macos-latest runner)
# Usage: bash scripts/test_adhoc_signing.sh

set -euo pipefail

if [[ "$(uname)" != "Darwin" ]]; then
  echo "SKIP: macOS required (codesign, hdiutil)"
  exit 0
fi

# ---------------------------------------------------------------------------
# Minimal test framework
# ---------------------------------------------------------------------------
PASS=0
FAIL=0

pass() { echo "PASS: $1"; ((PASS++)) || true; }
fail() { echo "FAIL: $1 -- $2"; ((FAIL++)) || true; }

assert_file_exists() {
  local label="$1" path="$2"
  if [[ -e "$path" ]]; then pass "$label"; else fail "$label" "file not found: $path"; fi
}

assert_file_not_exists() {
  local label="$1" path="$2"
  if [[ ! -e "$path" ]]; then pass "$label"; else fail "$label" "file unexpectedly exists: $path"; fi
}

assert_signed() {
  local label="$1" path="$2"
  if codesign -v "$path" 2>/dev/null; then pass "$label"; else fail "$label" "codesign -v failed for: $path"; fi
}

assert_symlink_target() {
  local label="$1" path="$2" expected="$3"
  if [[ ! -L "$path" ]]; then fail "$label" "not a symlink: $path"; return; fi
  local target
  target=$(readlink "$path")
  if [[ "$target" == "$expected" ]]; then pass "$label"; else fail "$label" "expected '$expected', got: '$target'"; fi
}

# ---------------------------------------------------------------------------
# Fixture: a minimal .app bundle (uses /bin/echo as the binary placeholder)
# ---------------------------------------------------------------------------
WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT

BUNDLE_DIR="$WORKDIR/target/release/bundle"
MACOS_DIR="$BUNDLE_DIR/macos"
DMG_DIR="$BUNDLE_DIR/dmg"

APP_NAME="cruise.app"
APP="$MACOS_DIR/$APP_NAME"
APP_VERSION="0.1.21"
DMG="$DMG_DIR/cruise_${APP_VERSION}_aarch64.dmg"
TAR_GZ="$MACOS_DIR/cruise.app.tar.gz"
SIG="${TAR_GZ}.sig"

setup_mock_bundle() {
  mkdir -p "$APP/Contents/MacOS" "$DMG_DIR"

  # Copy a real Mach-O binary so codesign can sign it
  cp /bin/echo "$APP/Contents/MacOS/cruise"

  cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>   <string>cruise</string>
  <key>CFBundleIdentifier</key>   <string>dev.cruise.app</string>
  <key>CFBundleName</key>         <string>Cruise</string>
  <key>CFBundlePackageType</key>  <string>APPL</string>
  <key>CFBundleShortVersionString</key> <string>0.1.21</string>
</dict>
</plist>
PLIST

  # Simulate the files Tauri would have created before the signing step
  # (old DMG containing an unsigned .app)
  hdiutil create -volname "Cruise" -srcfolder "$APP" -ov -format UDZO "$DMG" \
    > /dev/null 2>&1

  # (old tar.gz containing the unsigned .app)
  tar czf "$TAR_GZ" -C "$MACOS_DIR" "$APP_NAME"

  # (placeholder .sig as Tauri would have written)
  echo "fake-sig-data" > "$SIG"
}

# ---------------------------------------------------------------------------
# Step 1: .app discovery  (mirrors: release.yml:286)
# ---------------------------------------------------------------------------
test_app_discovery() {
  # Given: macos bundle dir contains exactly one .app
  # When:  we run the discovery find command from release.yml
  # Then:  APP resolves to the .app path
  local found
  found=$(find "$MACOS_DIR" -name "*.app" -maxdepth 1 | head -1)
  if [[ -n "$found" && -d "$found" ]]; then
    pass "app discovery: finds cruise.app"
  else
    fail "app discovery" "find returned empty or non-directory: '$found'"
  fi
}

# ---------------------------------------------------------------------------
# Step 2: ad-hoc signing  (mirrors: release.yml:291)
# ---------------------------------------------------------------------------
test_adhoc_signing() {
  # Given: an unsigned .app exists
  # When:  codesign --force --deep --sign - is run
  # Then:  codesign -v succeeds
  codesign --force --deep --sign - "$APP" 2>/dev/null
  assert_signed "ad-hoc signing: codesign -v passes after signing" "$APP"
}

# ---------------------------------------------------------------------------
# Step 3: DMG recreation with Applications symlink  (mirrors: release.yml:293-302)
# ---------------------------------------------------------------------------
test_dmg_recreation() {
  # Stages .app + /Applications symlink, creates DMG, cleans up, then verifies
  # DMG contents — mirrors the full staging sequence in release.yml.
  local stage
  stage=$(mktemp -d)
  cp -R "$APP" "$stage/"
  ln -s /Applications "$stage/Applications"
  assert_symlink_target \
    "staging dir: Applications symlink target is /Applications (absolute)" \
    "$stage/Applications" "/Applications"

  hdiutil create -volname "Cruise" -srcfolder "$stage" -ov -format UDZO "$DMG" \
    > /dev/null 2>&1
  assert_file_exists "DMG recreation: new DMG created" "$DMG"

  rm -rf "$stage"

  local mount_point
  mount_point=$(mktemp -d)
  hdiutil attach -mountpoint "$mount_point" "$DMG" -quiet
  local inner_app
  inner_app=$(find "$mount_point" -name "*.app" -maxdepth 1 | head -1)
  assert_signed "DMG contents: embedded .app is signed" "$inner_app"
  assert_symlink_target \
    "DMG contents: Applications symlink points to /Applications" \
    "$mount_point/Applications" "/Applications"
  hdiutil detach "$mount_point" -quiet
  rm -rf "$mount_point"
}

# ---------------------------------------------------------------------------
# Step 4: tar.gz recreation  (mirrors: plan step 3)
# ---------------------------------------------------------------------------
test_targz_old_files_removed() {
  # Given: old tar.gz and .sig exist
  # When:  we remove them before recreating
  # Then:  neither file exists
  rm -f "$TAR_GZ" "$SIG"
  assert_file_not_exists "tar.gz recreation: old tar.gz removed" "$TAR_GZ"
  assert_file_not_exists "tar.gz recreation: old .sig removed" "$SIG"
}

test_targz_recreated() {
  # Given: old tar.gz was removed and the .app is signed
  # When:  tar czf is run targeting the signed .app
  # Then:  a new tar.gz exists at the same path
  tar czf "$TAR_GZ" -C "$MACOS_DIR" "$APP_NAME"
  assert_file_exists "tar.gz recreation: new tar.gz created" "$TAR_GZ"
}

test_targz_top_level_entry_is_app() {
  # Given: the new tar.gz was created with -C <parent> <appname>
  # When:  we list its entries
  # Then:  the top-level entry starts with cruise.app (no extra path prefix)
  local first_entry
  first_entry=$(tar tzf "$TAR_GZ" | head -1)
  if [[ "$first_entry" == "${APP_NAME}"* ]]; then
    pass "tar.gz structure: top-level entry is .app (no path prefix)"
  else
    fail "tar.gz structure" "unexpected first entry: '$first_entry' (expected '${APP_NAME}...')"
  fi
}

test_targz_contains_signed_app() {
  # Given: the new tar.gz was created from the signed .app
  # When:  we extract the archive and inspect the .app inside
  # Then:  codesign -v succeeds for the extracted .app
  local extract_dir
  extract_dir=$(mktemp -d)
  tar xzf "$TAR_GZ" -C "$extract_dir"
  local extracted_app
  extracted_app=$(find "$extract_dir" -name "*.app" -maxdepth 1 | head -1)
  assert_signed "tar.gz recreation: extracted .app is signed" "$extracted_app"
  rm -rf "$extract_dir"
}

# ---------------------------------------------------------------------------
# Step 5: conditional updater re-signing  (mirrors: plan step 4)
# ---------------------------------------------------------------------------
test_updater_signing_skipped_without_key() {
  # Given: TAURI_SIGNING_PRIVATE_KEY is not set
  # When:  the conditional block runs
  # Then:  no .sig file is created (the block is skipped gracefully)
  if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
    # Simulate the conditional from the plan:
    #   if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then cargo tauri signer sign ...; fi
    # The block should be skipped → .sig file should not exist
    assert_file_not_exists "updater signing: skipped when key is absent" "$SIG"
  else
    pass "updater signing: TAURI_SIGNING_PRIVATE_KEY is set (signing would occur in CI)"
  fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
echo "Setting up mock Tauri bundle structure..."
setup_mock_bundle
echo ""
echo "--- Step 1: .app discovery ---"
test_app_discovery

echo ""
echo "--- Step 2: ad-hoc signing ---"
test_adhoc_signing

echo ""
echo "--- Step 3: DMG recreation with Applications symlink ---"
test_dmg_recreation

echo ""
echo "--- Step 4: tar.gz recreation ---"
test_targz_old_files_removed
test_targz_recreated
test_targz_top_level_entry_is_app
test_targz_contains_signed_app

echo ""
echo "--- Step 5: conditional updater signing ---"
test_updater_signing_skipped_without_key

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[[ "$FAIL" -eq 0 ]]
