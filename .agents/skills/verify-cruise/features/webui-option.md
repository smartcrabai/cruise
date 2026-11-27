# Answer an Option prompt in the WebUI

From a Planned session's page, `Run on current branch` starts the workflow; its Option step opens a dialog with the workflow's labels, and choosing one continues the run until the session shows `Completed` with the chosen branch in its log.

Covers: `src/webui/`, `webui/`, `src/daemon/api/`, `src/option_handler.rs`

## Sub-features

- `webui-session-page` shows the session title, phase badge, and `Run on current branch`.
- `webui-option-dialog` shows `Select an option` with each selector as a button.
- `webui-option-answer` sends the choice and the run follows that branch.
- `webui-completed` shows `Completed` and the step log after reload.

## How to get to it (user POV)

- Run `cruise webui`, open the printed login URL, choose the session in the sidebar.

## Driving it with verify_pty / webui-session

Preconditions:

- A `Planned` session for `choose.yaml`, created with `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" cruise plan --skip-planning --config ../choose.yaml "pick in the browser" </dev/null`; `ID` holds its ID.
- `ROOT/repo/chosen.txt` does not exist.

- **Open, run, answer.** Run `node .agents/skills/verify-cruise/scripts/webui-session.mjs "$ROOT" "$ID" "Fast path" "$RUN/webui-option"`. It prints `"chosen":"fast"` and `"phase":"Completed"`. `01-planned.png` shows the session page with `Run on current branch`; `02-option.png` shows the dialog with `Fast path` and `Alternate path` and the session `Running`; `03-completed.png` shows `Completed` and the log lines `choose -> fast [option]` and `fast`.
- **Effect.** Run `cat "$ROOT/repo/chosen.txt"`. It prints `fast`.

## Gotchas

- Bundled Chromium can crash at desktop widths on some macOS hosts; export `VERIFY_CRUISE_BROWSER_CHANNEL=chrome` (see SKILL.md).
- The login URL printed by `cruise webui` is single-use and expires after five minutes; the script requests a fresh one per run and never saves it.
- The dialog is the existing Option dialog; the rest of the page stays visible behind it. Screenshots taken while it is open show the session as `Running`.
