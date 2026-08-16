#!/usr/bin/env bash
# Exercises action/scripts/resolve-config.sh and action/scripts/install.sh
# directly, using the shared harness's faked runner contract.
. "$(dirname "$0")/lib/action_test_harness.sh"

# ===========================================================================
# resolve-config.sh
# ===========================================================================

WS="$TMP/ws"

reset_ws() {
  rm -rf "$WS"
  mkdir -p "$WS"
}

# Detects whether PyYAML is importable so the YAML-validity checks below can
# do a real parse when possible and fall back to structural grep otherwise.
PYYAML_OK=0
if python3 -c 'import yaml' >/dev/null 2>&1; then
  PYYAML_OK=1
else
  echo "WARN: PyYAML unavailable -- generated config was not parsed, only structurally checked"
fi

# --- config input: relative path resolves to an absolute CRUISE_CONFIG ----
new_case
reset_ws
mkdir -p "$WS/subdir"
printf 'sdk: pi\n' > "$WS/subdir/my-config.yaml"
CONFIG_INPUT="subdir/my-config.yaml" GITHUB_WORKSPACE="$WS" bash action/scripts/resolve-config.sh >/dev/null
assert_eq "resolve-config: a relative config input resolves to an absolute CRUISE_CONFIG path" \
  "$WS/subdir/my-config.yaml" "$(genv CRUISE_CONFIG)"
if [ -f "$(out exec_config_path)" ]; then
  pass "resolve-config: the exec config is generated even when config input is explicitly set"
else
  fail "resolve-config: the exec config is generated even when config input is explicitly set" "exec_config_path=$(out exec_config_path)"
fi

# --- config input: absolute path is used as-is -----------------------------
new_case
CONFIG_INPUT="$WS/subdir/my-config.yaml" GITHUB_WORKSPACE="$WS" bash action/scripts/resolve-config.sh >/dev/null
assert_eq "resolve-config: an absolute config input is used as-is" \
  "$WS/subdir/my-config.yaml" "$(genv CRUISE_CONFIG)"

# --- config input: nonexistent file --------------------------------------
new_case
reset_ws
stderr_out="$(CONFIG_INPUT="nope.yaml" GITHUB_WORKSPACE="$WS" bash action/scripts/resolve-config.sh 2>&1 >/dev/null)"
status=$?
assert_nonzero_status "resolve-config: a nonexistent config input exits non-zero" "$status" "status=$status"
assert_contains "resolve-config: a nonexistent config input reports the resolved absolute path and the raw input" \
  "$stderr_out" "cruise config not found at '$WS/nope.yaml' (input 'config' = 'nope.yaml')"
if grep -q '^CRUISE_CONFIG=' "$GITHUB_ENV"; then
  fail "resolve-config: a nonexistent config input leaves CRUISE_CONFIG unset" "$(cat "$GITHUB_ENV")"
else
  pass "resolve-config: a nonexistent config input leaves CRUISE_CONFIG unset"
fi
if [ -s "$GITHUB_OUTPUT" ]; then
  fail "resolve-config: a nonexistent config input aborts before generating any output" "$(cat "$GITHUB_OUTPUT")"
else
  pass "resolve-config: a nonexistent config input aborts before generating any output"
fi

# --- config input empty + repo already has its own config -----------------
# Asserts ABSENCE of the CRUISE_CONFIG line entirely (not merely an empty
# value) per the script's own comment: an empty `CRUISE_CONFIG=` would be a
# distinct bug (cruise's resolver treats that as "use this nonexistent
# path" and hard-errors), so grep for the line existing at all.
assert_own_config_detected() { # $1 = path (may include a subdir) relative to $WS
  local relpath="$1"
  new_case
  reset_ws
  mkdir -p "$WS/$(dirname "$relpath")"
  printf 'sdk: pi\n' > "$WS/$relpath"
  local stdout_out
  stdout_out="$(GITHUB_WORKSPACE="$WS" bash action/scripts/resolve-config.sh)"
  if grep -q '^CRUISE_CONFIG=' "$GITHUB_ENV"; then
    fail "resolve-config: an existing repo config at '$relpath' leaves CRUISE_CONFIG entirely unset" "$(cat "$GITHUB_ENV")"
  else
    pass "resolve-config: an existing repo config at '$relpath' leaves CRUISE_CONFIG entirely unset"
  fi
  assert_contains "resolve-config: an existing repo config at '$relpath' logs that cruise's own resolver will pick it up" \
    "$stdout_out" "repository already has its own config"
  if [ -f "$(out exec_config_path)" ]; then
    pass "resolve-config: an existing repo config at '$relpath' still generates the exec config file"
  else
    fail "resolve-config: an existing repo config at '$relpath' still generates the exec config file" "exec_config_path=$(out exec_config_path)"
  fi
}
assert_own_config_detected "cruise.yaml"
assert_own_config_detected "cruise.yml"
assert_own_config_detected ".cruise.yaml"
assert_own_config_detected ".cruise.yml"
assert_own_config_detected ".cruise/foo.yaml"
assert_own_config_detected ".cruise/foo.yml"

# --- config input takes priority over an existing repo config -------------
new_case
reset_ws
printf 'sdk: pi\n' > "$WS/cruise.yaml"
mkdir -p "$WS/custom"
printf 'sdk: pi\n' > "$WS/custom/explicit.yaml"
CONFIG_INPUT="custom/explicit.yaml" GITHUB_WORKSPACE="$WS" bash action/scripts/resolve-config.sh >/dev/null
assert_eq "resolve-config: an explicit config input takes priority over an existing repo-owned config" \
  "$WS/custom/explicit.yaml" "$(genv CRUISE_CONFIG)"

# --- config input empty + no repo config: generates a default -------------
new_case
reset_ws
GITHUB_WORKSPACE="$WS" bash action/scripts/resolve-config.sh >/dev/null
default_cfg="$(genv CRUISE_CONFIG)"
assert_eq "resolve-config: the generated default config path is under \$RUNNER_TEMP/cruise" \
  "$RUNNER_TEMP/cruise/default-config.yaml" "$default_cfg"
if [ -f "$default_cfg" ]; then
  pass "resolve-config: the generated default config file actually exists"
else
  fail "resolve-config: the generated default config file actually exists" "default_cfg=$default_cfg"
fi

if [ "$PYYAML_OK" -eq 1 ]; then
  if python3 -c "import yaml; yaml.safe_load(open('$default_cfg'))" >/dev/null 2>&1; then
    pass "resolve-config: the generated default config is valid YAML (PyYAML parse)"
  else
    fail "resolve-config: the generated default config is valid YAML (PyYAML parse)" "$(cat "$default_cfg")"
  fi
else
  if grep -qx 'sdk: pi' "$default_cfg" && grep -qx 'steps:' "$default_cfg" \
     && grep -qx '  write-tests:' "$default_cfg" && grep -qx '  implement:' "$default_cfg"; then
    pass "resolve-config: the generated default config has the expected top-level structure (structural only)"
  else
    fail "resolve-config: the generated default config has the expected top-level structure (structural only)" "$(cat "$default_cfg")"
  fi
fi

first_wt_line="$(sed -n '1p' prompts/write-test-first.md)"
first_impl_line="$(sed -n '1p' prompts/implement-after-tests.md)"
if [ -n "$first_wt_line" ]; then
  pass "resolve-config: prompts/write-test-first.md has a non-empty first line to use as a needle"
else
  fail "resolve-config: prompts/write-test-first.md has a non-empty first line to use as a needle" "prompts/write-test-first.md's first line is empty"
fi
if [ -n "$first_impl_line" ]; then
  pass "resolve-config: prompts/implement-after-tests.md has a non-empty first line to use as a needle"
else
  fail "resolve-config: prompts/implement-after-tests.md has a non-empty first line to use as a needle" "prompts/implement-after-tests.md's first line is empty"
fi
assert_contains "resolve-config: the generated default config embeds the write-test-first.md prompt" \
  "$(cat "$default_cfg")" "$first_wt_line"
assert_contains "resolve-config: the generated default config embeds the implement-after-tests.md prompt" \
  "$(cat "$default_cfg")" "$first_impl_line"

if grep -Eq '^[[:space:]]*(model|plan_model):' "$default_cfg"; then
  fail "resolve-config: the generated default config sets neither model: nor plan_model:" "$(grep -En '^[[:space:]]*(model|plan_model):' "$default_cfg")"
else
  pass "resolve-config: the generated default config sets neither model: nor plan_model:"
fi

# --- exec config: always generated, references {input} not {plan} ---------
exec_cfg="$(out exec_config_path)"
if [ -n "$exec_cfg" ]; then
  pass "resolve-config: exec_config_path is emitted with no config input and no repo config"
else
  fail "resolve-config: exec_config_path is emitted with no config input and no repo config" "exec_cfg is empty"
fi
if [ -f "$exec_cfg" ]; then
  pass "resolve-config: the exec config file exists on disk"
else
  fail "resolve-config: the exec config file exists on disk" "exec_cfg=$exec_cfg"
fi
# The file's leading comment legitimately mentions "{input}" in prose too, so
# a bare substring match on the whole file would still pass even if the
# `prompt:` field itself lost the placeholder; check the field's actual value.
if grep -Eq '^[[:space:]]*prompt:[[:space:]]*"\{input\}"' "$exec_cfg"; then
  pass "resolve-config: the exec config's prompt field references {input}"
else
  fail "resolve-config: the exec config's prompt field references {input}" "$(cat "$exec_cfg")"
fi
# The file's leading comment legitimately mentions "{plan}" in prose (to
# explain why {input} was chosen instead), so check the actual `prompt:`
# field's value rather than the whole file for a bare substring.
if grep -Eq '^\s*prompt:\s*"\{plan\}"' "$exec_cfg"; then
  fail "resolve-config: the exec config's prompt field does not reference {plan}" "$(cat "$exec_cfg")"
else
  pass "resolve-config: the exec config's prompt field does not reference {plan}"
fi
if grep -Eq '^[[:space:]]*(model|plan_model):' "$exec_cfg"; then
  fail "resolve-config: the exec config sets neither model: nor plan_model:" "$(cat "$exec_cfg")"
else
  pass "resolve-config: the exec config sets neither model: nor plan_model:"
fi

# --- prompt embedding round-trips verbatim (blank lines, YAML specials) ---
# embed_prompt_file emits exactly one output line per input line (blank
# lines pass through with no indent), so the Nth line of the source prompt
# file maps 1:1 to the Nth line of its embedded block; slicing that many
# lines starting right after the block's "prompt: |" marker and stripping
# the fixed 6-space indent must reproduce the source file exactly.
prompt_marker_lines="$(grep -n '^    prompt: |$' "$default_cfg" | cut -d: -f1)"
wt_marker="$(printf '%s\n' "$prompt_marker_lines" | sed -n '1p')"
impl_marker="$(printf '%s\n' "$prompt_marker_lines" | sed -n '2p')"
wt_len="$(wc -l < prompts/write-test-first.md | tr -d ' ')"
impl_len="$(wc -l < prompts/implement-after-tests.md | tr -d ' ')"
wt_start=$((wt_marker + 1))
wt_end=$((wt_start + wt_len - 1))
impl_start=$((impl_marker + 1))
impl_end=$((impl_start + impl_len - 1))
sed -n "${wt_start},${wt_end}p" "$default_cfg" | sed 's/^      //' > "$TMP/extracted-write-test-first.md"
sed -n "${impl_start},${impl_end}p" "$default_cfg" | sed 's/^      //' > "$TMP/extracted-implement-after-tests.md"
if diff -q "$TMP/extracted-write-test-first.md" prompts/write-test-first.md >/dev/null; then
  pass "resolve-config: write-test-first.md's prompt round-trips verbatim into the generated config"
else
  fail "resolve-config: write-test-first.md's prompt round-trips verbatim into the generated config" \
    "$(diff "$TMP/extracted-write-test-first.md" prompts/write-test-first.md)"
fi
if diff -q "$TMP/extracted-implement-after-tests.md" prompts/implement-after-tests.md >/dev/null; then
  pass "resolve-config: implement-after-tests.md's prompt round-trips verbatim into the generated config"
else
  fail "resolve-config: implement-after-tests.md's prompt round-trips verbatim into the generated config" \
    "$(diff "$TMP/extracted-implement-after-tests.md" prompts/implement-after-tests.md)"
fi

# --- GITHUB_ACTION_PATH wins over the script-relative fallback ------------
# On a real runner GITHUB_ACTION_PATH points at the action's own checkout
# (which is a *different* tree from GITHUB_WORKSPACE whenever the action is
# used from another repository), so the generated config must embed the
# prompts from there, not from the caller's checkout.
new_case
reset_ws
ACTION_TREE="$TMP/action-tree"
rm -rf "$ACTION_TREE"
mkdir -p "$ACTION_TREE/prompts"
printf 'FAKE ACTION-TREE TEST PROMPT\n' > "$ACTION_TREE/prompts/write-test-first.md"
printf 'FAKE ACTION-TREE IMPL PROMPT\n' > "$ACTION_TREE/prompts/implement-after-tests.md"
CONFIG_INPUT="" GITHUB_WORKSPACE="$WS" GITHUB_ACTION_PATH="$ACTION_TREE" \
  bash action/scripts/resolve-config.sh >/dev/null
action_path_cfg="$(genv CRUISE_CONFIG)"
assert_contains "resolve-config: GITHUB_ACTION_PATH supplies the embedded prompts" \
  "$(cat "$action_path_cfg")" 'FAKE ACTION-TREE TEST PROMPT'
assert_contains "resolve-config: GITHUB_ACTION_PATH supplies the implement prompt too" \
  "$(cat "$action_path_cfg")" 'FAKE ACTION-TREE IMPL PROMPT'
if grep -Fq "$first_wt_line" "$action_path_cfg"; then
  fail "resolve-config: GITHUB_ACTION_PATH is preferred over the script-relative fallback" \
    "the repo's own prompts leaked in: $(cat "$action_path_cfg")"
else
  pass "resolve-config: GITHUB_ACTION_PATH is preferred over the script-relative fallback"
fi

# --- a prompt file missing from the action tree fails loudly ---------------
new_case
reset_ws
rm -f "$ACTION_TREE/prompts/write-test-first.md"
missing_prompt_out="$(CONFIG_INPUT="" GITHUB_WORKSPACE="$WS" GITHUB_ACTION_PATH="$ACTION_TREE" \
  bash action/scripts/resolve-config.sh 2>&1 >/dev/null)"
status=$?

assert_nonzero_status "resolve-config: a missing prompt file exits non-zero" "$status" "status=$status"
assert_contains "resolve-config: a missing prompt file reports the path plus GITHUB_ACTION_PATH/ACTION_ROOT" \
  "$missing_prompt_out" "::error::cruise: prompt file not found at '$ACTION_TREE/prompts/write-test-first.md' (GITHUB_ACTION_PATH=$ACTION_TREE, ACTION_ROOT=$ACTION_TREE)"

# ===========================================================================
# install.sh
# ===========================================================================
# install.sh pipes `curl -fsSL "$url" | ENV... sh`. A stub `curl` prints a
# locally-authored fake installer script to stdout (no network involved);
# the real `/bin/sh` on PATH then executes that script, which records the
# env vars it received and drops a fake `cruise` binary into
# $CRUISE_UNMANAGED_INSTALL so the rest of install.sh's own PATH checks
# behave exactly as they would against a real install.

export GITHUB_PATH="$TMP/github_path"
FAKE_INSTALLER="$TMP/fake-installer.sh"
export FAKE_INSTALLER

stub curl <<'SH'
#!/usr/bin/env bash
printf 'curl %s\n' "$*" >> "$STUB_LOG"
cat "$FAKE_INSTALLER"
SH

# A stub `gh` so the trailing gh-presence check succeeds in the "happy path"
# cases without depending on whatever is actually installed on this host.
log_stub gh

write_ok_installer() {
  cat > "$FAKE_INSTALLER" <<'EOF'
#!/bin/sh
printf 'installer CRUISE_UNMANAGED_INSTALL=%s CRUISE_NO_MODIFY_PATH=%s CRUISE_DISABLE_UPDATE=%s CRUISE_PRINT_QUIET=%s\n' \
  "$CRUISE_UNMANAGED_INSTALL" "$CRUISE_NO_MODIFY_PATH" "$CRUISE_DISABLE_UPDATE" "$CRUISE_PRINT_QUIET" >> "$STUB_LOG"
mkdir -p "$CRUISE_UNMANAGED_INSTALL"
cat > "$CRUISE_UNMANAGED_INSTALL/cruise" <<'BIN'
#!/bin/sh
echo "cruise 0.0.0-fake"
BIN
chmod +x "$CRUISE_UNMANAGED_INSTALL/cruise"
EOF
}

write_failing_installer() {
  cat > "$FAKE_INSTALLER" <<'EOF'
#!/bin/sh
mkdir -p "$CRUISE_UNMANAGED_INSTALL"
cat > "$CRUISE_UNMANAGED_INSTALL/cruise" <<'BIN'
#!/bin/sh
echo "cruise 0.0.0-fake"
BIN
chmod +x "$CRUISE_UNMANAGED_INSTALL/cruise"
printf 'installer-invoked-then-failed\n' >> "$STUB_LOG"
exit 1
EOF
}

# --- CRUISE_VERSION=latest: no version pin in the installer URL -----------
new_case
: > "$GITHUB_PATH"
write_ok_installer
IDIR="$TMP/install-latest"
mkdir -p "$IDIR"
status_out=$(PATH="$STUB_DIR:/usr/bin:/bin" RUNNER_TEMP="$IDIR" CRUISE_VERSION=latest bash action/scripts/install.sh 2>&1)
status=$?
assert_status "install: CRUISE_VERSION=latest succeeds" 0 "$status" "$status_out"
assert_contains "install: CRUISE_VERSION=latest calls curl against the unversioned 'latest' download URL" \
  "$(cat "$STUB_LOG")" \
  "curl -fsSL https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh"
assert_contains "install: CRUISE_VERSION=latest passes CRUISE_UNMANAGED_INSTALL/NO_MODIFY_PATH/DISABLE_UPDATE/PRINT_QUIET to the installer" \
  "$(cat "$STUB_LOG")" "installer CRUISE_UNMANAGED_INSTALL=$IDIR/cruise-bin CRUISE_NO_MODIFY_PATH=1 CRUISE_DISABLE_UPDATE=1 CRUISE_PRINT_QUIET=1"
assert_eq "install: CRUISE_VERSION=latest appends the install dir to GITHUB_PATH" \
  "$IDIR/cruise-bin" "$(cat "$GITHUB_PATH")"

# --- CRUISE_VERSION unset: defaults to the same 'latest' behaviour --------
new_case
: > "$GITHUB_PATH"
write_ok_installer
IDIR2="$TMP/install-default"
mkdir -p "$IDIR2"
unset CRUISE_VERSION
status_out=$(PATH="$STUB_DIR:/usr/bin:/bin" RUNNER_TEMP="$IDIR2" bash action/scripts/install.sh 2>&1)
status=$?
assert_status "install: an unset CRUISE_VERSION defaults to the unversioned 'latest' URL" \
  0 "$status" "$status_out"
assert_contains "install: an unset CRUISE_VERSION calls curl against the 'latest' download URL" \
  "$(cat "$STUB_LOG")" "curl -fsSL https://github.com/smartcrabai/cruise/releases/latest/download/cruise-installer.sh"

# --- a pinned tag is passed through to the installer URL -------------------
new_case
: > "$GITHUB_PATH"
write_ok_installer
IDIR3="$TMP/install-pinned"
mkdir -p "$IDIR3"
status_out=$(PATH="$STUB_DIR:/usr/bin:/bin" RUNNER_TEMP="$IDIR3" CRUISE_VERSION=v0.1.68 bash action/scripts/install.sh 2>&1)
status=$?
assert_status "install: a pinned CRUISE_VERSION succeeds" 0 "$status" "$status_out"
assert_contains "install: a pinned CRUISE_VERSION (v0.1.68) is embedded in the versioned download URL" \
  "$(cat "$STUB_LOG")" "curl -fsSL https://github.com/smartcrabai/cruise/releases/download/v0.1.68/cruise-installer.sh"
assert_eq "install: a pinned CRUISE_VERSION also appends the install dir to GITHUB_PATH" \
  "$IDIR3/cruise-bin" "$(cat "$GITHUB_PATH")"

# --- cruise already on PATH: installer is skipped entirely -----------------
new_case
: > "$GITHUB_PATH"
stub cruise <<'SH'
#!/usr/bin/env bash
if [ "$1" = "--version" ]; then
  echo "cruise 9.9.9-preexisting"
  exit 0
fi
printf 'cruise %s\n' "$*" >> "$STUB_LOG"
SH
IDIR4="$TMP/install-preexisting"
mkdir -p "$IDIR4"
status_out=$(PATH="$STUB_DIR:/usr/bin:/bin" RUNNER_TEMP="$IDIR4" CRUISE_VERSION=latest bash action/scripts/install.sh 2>&1)
status=$?
assert_status "install: an already-installed cruise on PATH succeeds without reinstalling" \
  0 "$status" "$status_out"
if grep -q '^curl ' "$STUB_LOG"; then
  fail "install: an already-installed cruise on PATH never invokes curl" "$(cat "$STUB_LOG")"
else
  pass "install: an already-installed cruise on PATH never invokes curl"
fi
if [ -s "$GITHUB_PATH" ]; then
  fail "install: an already-installed cruise on PATH does not append to GITHUB_PATH" "$(cat "$GITHUB_PATH")"
else
  pass "install: an already-installed cruise on PATH does not append to GITHUB_PATH"
fi
assert_contains "install: an already-installed cruise on PATH still reports its version" "$status_out" "cruise 9.9.9-preexisting"
rm -f "$STUB_DIR/cruise"

# --- installer failure: the step must not silently continue ---------------
new_case
: > "$GITHUB_PATH"
write_failing_installer
IDIR5="$TMP/install-failing"
mkdir -p "$IDIR5"
status_out=$(PATH="$STUB_DIR:/usr/bin:/bin" RUNNER_TEMP="$IDIR5" CRUISE_VERSION=latest bash action/scripts/install.sh 2>&1)
status=$?
assert_nonzero_status "install: an installer that exits non-zero makes the step fail (not silently continue)" \
  "$status" "status=$status output=$status_out"
if grep -q 'installer-invoked-then-failed' "$STUB_LOG"; then
  pass "install: the failing installer was actually invoked before the step aborted"
else
  fail "install: the failing installer was actually invoked before the step aborted" "$(cat "$STUB_LOG")"
fi
assert_contains "install: a failing installer reports a clear ::error:: annotation" \
  "$status_out" "::error::cruise installer failed for version 'latest'"

# --- curl failure: the download itself must produce the same clear error ---
new_case
: > "$GITHUB_PATH"
stub curl <<'SH'
#!/usr/bin/env bash
printf 'curl-invoked-then-failed\n' >> "$STUB_LOG"
exit 22
SH
IDIR7="$TMP/install-curl-failing"
mkdir -p "$IDIR7"
status_out=$(PATH="$STUB_DIR:/usr/bin:/bin" RUNNER_TEMP="$IDIR7" CRUISE_VERSION=latest bash action/scripts/install.sh 2>&1)
status=$?
assert_nonzero_status "install: a curl failure makes the step fail" \
  "$status" "status=$status output=$status_out"
assert_contains "install: a curl failure reports a clear ::error:: annotation" \
  "$status_out" "::error::cruise installer failed for version 'latest'"

# Restore the normal download stub for the independent missing-gh case.
stub curl <<'SH'
#!/usr/bin/env bash
printf 'curl %s\n' "$*" >> "$STUB_LOG"
cat "$FAKE_INSTALLER"
SH

# --- gh missing on PATH: exits non-zero with a clear error -----------------
# GitHub-hosted ubuntu-latest ships /usr/bin/gh (cli/cli's .deb installs to
# /usr/bin), so borrowing the system dirs here would make `command -v gh`
# succeed on a real runner and silently redden this case. Build a PATH from
# scratch out of symlinks to only the binaries install.sh's happy path
# genuinely needs -- sh/env/bash to run the piped installer script (its
# stub's "#!/usr/bin/env bash" shebang resolves bash via PATH), plus
# mkdir/chmod/cat for the installer body itself -- and never gh, regardless
# of what the host has installed.
new_case
: > "$GITHUB_PATH"
write_ok_installer
rm -f "$STUB_DIR/gh"
NOGH_BIN="$TMP/nogh-bin"
must mkdir -p "$NOGH_BIN"
for _bin in sh env bash mkdir chmod cat; do
  must ln -s "$(command -v "$_bin")" "$NOGH_BIN/$_bin"
done
IDIR6="$TMP/install-nogh"
mkdir -p "$IDIR6"
status_out=$(PATH="$STUB_DIR:$NOGH_BIN" RUNNER_TEMP="$IDIR6" CRUISE_VERSION=latest bash action/scripts/install.sh 2>&1)
status=$?
assert_nonzero_status "install: a missing gh CLI makes the step fail after a successful cruise install" \
  "$status" "status=$status output=$status_out"
assert_contains "install: a missing gh CLI reports a clear ::error:: naming the cause" \
  "$status_out" "::error::gh CLI not found on PATH"

# Lines/branches not independently exercised here (documented, not testable
# hermetically): install.sh's `cruise --version` output is only ever the
# fake binary's own canned string above, never a real cargo-dist build; and
# the case where BOTH the installer succeeds AND the freshly-installed
# fake `cruise` binary still fails `command -v cruise` afterward (line 35-38's
# "cruise installation failed" ::error::) is unreachable from a stub that
# always drops a working binary in place -- reaching it would require an
# installer that reports success but writes no executable, which isn't a
# realistic contract to fake without asserting invented behaviour.

finish
