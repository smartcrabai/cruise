---
name: verify-cruise
description: Drive this checkout's Cruise CLI/TUI in a real PTY and its WebUI in Chromium from an isolated HOME, and capture terminal screens and screenshots as implementation evidence. Use before opening a Cruise PR, when a change touches a mapped feature in features/, or when asked to prove CLI, TUI, or WebUI behavior.
---

# Verify Cruise

Prove a change the way a user meets it: build this checkout, start a daemon that belongs to this run, drive the real `cruise` binary in a pseudo-terminal or the WebUI in a browser, and keep the screens. Never drive the user's own daemon, `~/.local/share/cruise`, `~/apps/cruise`, or existing sessions and Goals. Every command below runs from the repository root.

## Launch

```sh
ROOT=$(.agents/skills/verify-cruise/scripts/launch.sh)            # builds, then prints the run root
RUN=target/verify-cruise/$(date +%Y%m%d-%H%M%S); mkdir -p "$RUN"   # evidence for this run
```

`launch.sh` runs `cargo +1.99.1 build --locked --all-features --bin cruise --bin cruise-runtime --example verify_pty` (pass `--no-build` to reuse a build of the current tree), creates `ROOT` under `$TMPDIR` with its own `HOME`/`XDG_*`, a fixture Git repository at `ROOT/repo` on `main`, the workflow fixtures `ROOT/run-session.yaml` and `ROOT/choose.yaml` (command steps only; no SDK, model, or network), writes `check_base_ci: false`, and starts `cruise daemon start --port 0`. Ready means `launch.sh` printed `ROOT` and Doctor passes.

Run any command inside the run with `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" COMMAND...`: an empty environment plus `ROOT/env` (this checkout's `target/debug` first on `PATH`), working directory `ROOT/repo`. Host prerequisites: Rust 1.99.1, Git, `jq`, and Node 26.13.1. Browser scripts need `npm ci --prefix tests/webui --ignore-scripts` and `tests/webui/node_modules/.bin/playwright install chromium` once.

## Doctor

```sh
.agents/skills/verify-cruise/scripts/doctor.sh "$ROOT"
```

Read-only. Fails unless `HOME` is the run's own, the daemon is running with the PID `launch.sh` started and this checkout's `target/debug/cruise-runtime`, client and daemon versions match, source `HEAD` and `src/`, `webui/`, `build.rs`, and the manifests are unchanged since the launch build, and the fixture repository exists. Run it before the first drive, after any failed drive, and after editing code (a stale build fails it: clean up and launch again).

## Drive

Terminal: `verify_pty` (`scripts/verify_pty.rs`, built as a Cargo example) runs a command in a 120x40 PTY with `TERM=xterm-256color`, answers the terminal queries Cruise's prompts send, and executes operations in order: `--wait TEXT` (screen or transcript contains TEXT), `--send KEYS` (`\r`, `\e` for Escape, `\e[B` for Down), `--sleep MS`, `--snap NAME`. After the operations the program must exit within `--timeout` seconds (default 60).

```sh
.agents/skills/verify-cruise/scripts/in.sh "$ROOT" verify_pty --out "$PWD/$RUN/run-session" \
  -- cruise exec --config ../run-session.yaml "hello from verify-cruise"
```

Browser: `node .agents/skills/verify-cruise/scripts/webui-session.mjs "$ROOT" SESSION_ID "Fast path" "$RUN/webui-option"` logs in with a fresh `cruise webui` URL, drives a Planned `choose.yaml` session, answers its Option dialog by visible label, and waits for `Completed`. Use role and visible-name selectors; the scripts already do.

The feature map in [features/README.md](features/README.md) lists the recipes. Drive every feature whose `Covers:` paths the change touches; one is enough for a doc-only change to this skill.

## Evidence

- `verify_pty` writes `transcript.raw`, `NN-NAME.txt` (plain screen), `NN-NAME.html` (colored screen with the command line as its header), and `result.json` (`argv`, `exit_code`, `completed`, `snapshots`). Render the terminal screens: `node .agents/skills/verify-cruise/scripts/screen-png.mjs "$RUN"/*/*.html`.
- `webui-session.mjs` writes `NN-NAME.png`, `NN-NAME.aria.txt`, and `result.json` with the session phase at each step and the value the workflow wrote.
- A proof shows the action and the resulting state, plus a side effect read back separately: `in.sh "$ROOT" cruise list --json` for the phase and the file the workflow wrote in `ROOT/repo`. Save them in `$RUN` (for example `in.sh "$ROOT" cruise list --json | jq '[.[]|{title,phase}]' > "$RUN/sessions.json"`).
- A regression check proves nothing until it has been seen failing: show it failing once on the broken build or input.
- For a PR, publish the evidence directory to the orphan branch `cruise-evidence` and paste the printed links: `.agents/skills/verify-cruise/scripts/publish.sh "$RUN" "pr/<branch>/$(basename "$RUN")"`. It refuses text artifacts that look like credentials (login nonces, tokens) and never touches the checkout or index. Screenshots cannot be scanned; capture only fixture data. The WebUI login URL is single-use and is never written to evidence.

## Cleanup

```sh
.agents/skills/verify-cruise/scripts/cleanup.sh "$ROOT"
```

Stops the daemon through `cruise daemon stop --force`, then kills only the PID `launch.sh` recorded if it is still that executable, and deletes `ROOT`. It never kills by process name and never touches `target/verify-cruise/`. Run it after every failed attempt too, then confirm the evidence directory still exists.

## Helpers

All in `scripts/`, executable, shown above: `launch.sh`, `in.sh`, `doctor.sh`, `verify_pty.rs` (`[[example]] verify_pty` in `Cargo.toml`), `screen-png.mjs`, `webui-session.mjs`, `publish.sh`, `cleanup.sh`.

## Gotchas

- `cruise plan` on a TTY asks for an action and then for steps to skip; `Enter` on the second prompt approves with no skips. `cruise run` on a TTY first asks where to execute; choose `Use current branch` (`\e[B` then `\r`) to keep work in `ROOT/repo`.
- Each drive is its own process; the daemon is shared within a run. Sessions accumulate, so select by ID or phase, not by position.
- On some macOS hosts the bundled Chromium headless shell crashes (`Page crashed`, `BUS_ADRALN`) at widths of 768px and more, including the repository's own `tests/webui` desktop contracts. Set `VERIFY_CRUISE_BROWSER_CHANNEL=chrome` to use the installed Google Chrome with Playwright's isolated profile.
- `launch.sh --no-build` reuses binaries as they are; Doctor catches source edits after launch, not before.
- The CLI exits 0 when a workflow finishes; read `exit_code` in `result.json` and the session phase, not only the screen.
