#!/usr/bin/env bash
# Real production HTTP + browser scenarios, using the repository's Rust BDD stack.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
[[ $(node --version) == v22.23.3 ]] || { echo 'BLOCKED: Node 22.23.3 required' >&2; exit 1; }
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/responsive-browser.XXXXXX")
# Do not inherit a local debug override into the positive CI acceptance run.
unset WEBUI_BDD_FEATURE WEBUI_BDD_ASSETS WEBUI_BDD_BIN
# Installing this suite must not garbage-collect another project's browser cache.
export PLAYWRIGHT_SKIP_BROWSER_GC=1
npm ci --prefix tests/webui --ignore-scripts
case $(uname -s) in
  Linux) tests/webui/node_modules/.bin/playwright install --with-deps chromium ;;
  Darwin) tests/webui/node_modules/.bin/playwright install chromium ;;
  *) echo 'BLOCKED: browser suite requires supported POSIX production fixtures' >&2; exit 1 ;;
esac
node --version | tee "$RUN/node-version.log"
tests/webui/node_modules/.bin/playwright --version | tee "$RUN/playwright-version.log"
node -e 'const { chromium } = require("./tests/webui/node_modules/playwright"); (async () => { const browser = await chromium.launch(); console.log("Chromium " + browser.version()); await browser.close(); })().catch(error => { console.error(error); process.exitCode = 1; });' | tee "$RUN/chromium-version.log"
cargo +1.99.1 test --locked --all-features --test webui_responsive_bdd 2>&1 | tee "$RUN/cucumber.log"
node tests/webui/negative-controls.mjs 2>&1 | tee "$RUN/negative-controls.log"
echo "Responsive browser reports: $RUN"
