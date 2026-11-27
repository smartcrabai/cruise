# Answer an Option prompt in the terminal

`cruise run <id>` on a terminal asks where to execute, runs the workflow, shows an Option step as a menu, and follows the chosen branch to completion.

Covers: `src/run_cmd.rs`, `src/option_handler.rs`, `src/ask_handler.rs`, `src/console_mode.rs`, `src/multiline_input.rs`, `src/worktree.rs`, `src/daemon/client.rs`

## Sub-features

- `run-workspace` offers `Create worktree` and `Use current branch` before running.
- `run-option` shows the Option labels from the workflow and moves with the arrow keys.
- `run-branch` runs only the chosen branch's steps and finishes `Completed`.

## How to get to it (user POV)

- Run `cruise run <session-id>` for a Planned session whose workflow has an `option` step.

## Driving it with verify_pty / webui-session

Preconditions:

- A `Planned` session from [Approve a plan in the terminal](./plan-approve.md); `ID` holds its ID.
- `ROOT/repo/chosen.txt` does not exist.

- **Run.** Choose the current branch, then `Alternate path`. Run `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" verify_pty --out "$PWD/$RUN/terminal-option" --wait 'Use current branch' --snap workspace --send '\e[B' --sleep 150 --send '\r' --wait 'Alternate path' --snap option --send '\e[B' --sleep 150 --send '\r' -- cruise run "$ID"`. `02-option.txt` shows `? Select an option` with `Fast path` and `Alternate path`; the final screen shows `choose -> alternate [option]`, `> done`, `alternate`, and `workflow complete (3 run, 0 skipped, 0 failed)`; `exit_code` is `0`.
- **Effect.** Run `cat "$ROOT/repo/chosen.txt"`. It prints `alternate`.
- **State.** Run `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" cruise list --json | jq -r --arg id "$ID" '.[] | select(.id == $id) | .phase'`. It prints `Completed`.

## Gotchas

- Send the arrow key and `Enter` as separate operations with a short `--sleep`; sent together, the menu can read `Enter` before it moves.
- `Create worktree` is the default; pressing `Enter` immediately runs in a new worktree outside `ROOT/repo`, so the effect file is not where this recipe reads it.
