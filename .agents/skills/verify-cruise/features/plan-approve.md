# Approve a plan in the terminal

`cruise plan --skip-planning` on a terminal shows the plan in a panel, asks for an action, then asks which steps to skip; approving creates a session in `Planned` and prints how to run it.

Covers: `src/plan_cmd.rs`, `src/planning.rs`, `src/planning/`, `src/option_handler.rs`, `src/session.rs`, `src/flags.rs`, `src/cli.rs`

## Sub-features

- `plan-menu` shows the plan panel and the `Approve`, `Fix`, `Ask`, `Verify`, `Execute now`, and `Publish as Issue` actions.
- `plan-skip` asks which steps to skip after `Approve`.
- `plan-created` creates a `Planned` session and prints `cruise run <id>`.
- `plan-cancel` discards the session on Escape.

## How to get to it (user POV)

- Run `cruise plan --skip-planning --config ../choose.yaml "<task>"` in a terminal.

## Driving it with verify_pty / webui-session

Preconditions:

- Baseline launch passed Doctor.

- **Approve.** Choose `Approve`, then confirm no skips. Run `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" verify_pty --out "$PWD/$RUN/plan-approve" --wait 'Publish as Issue' --snap menu --send '\r' --wait 'Steps to skip' --snap skip --send '\r' -- cruise plan --skip-planning --config ../choose.yaml "pick a path"`. `01-menu.txt` lists the six actions under `? Action:`; `02-skip.txt` lists `choose`, `fast`, `alternate`, `done`; the final screen shows `Session <id> created.` and `Run with: cruise run <id>`; `exit_code` is `0`.
- **State.** Read the session back. Run `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" cruise list --json | jq '.[] | select(.title == "pick a path") | .phase'`. It prints `"Planned"`.
- **Cancel.** Escape the action menu. Run `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" verify_pty --out "$PWD/$RUN/plan-cancel" --wait 'Publish as Issue' --send '\e' -- cruise plan --skip-planning --config ../choose.yaml "discard me"`. The final screen shows `? Action: <canceled>` and `Session <id> discarded.`; `cruise list --json` has no `discard me` session.

## Gotchas

- `Approve` alone does not finish: the skip selector waits for `Enter`. A drive that stops after the first `\r` leaves the session `AwaitingApproval`.
- Without a TTY, `--skip-planning` approves automatically; this recipe proves the interactive path only through `verify_pty`.
