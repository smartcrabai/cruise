# Run a workflow in the current directory

`cruise exec --config <workflow>` runs the workflow's steps in the current directory without a plan, worktree, or PR, streams each step's output to the terminal, and ends with a summary line.

Covers: `src/exec_cmd.rs`, `src/engine.rs`, `src/engine/`, `src/step/`, `src/graph.rs`, `src/graph/`, `src/display.rs`, `src/workflow.rs`, `src/launcher.rs`, `src/main.rs`, `src/daemon/`

## Sub-features

- `exec-run` runs every step of a command workflow in order.
- `exec-output` streams step output and the `workflow complete` summary.
- `exec-effect` leaves the step's file change in the working tree.

## How to get to it (user POV)

- Run `cruise exec --config ../run-session.yaml "<text>"` in a Git repository on a branch.

## Driving it with verify_pty / webui-session

Preconditions:

- Baseline launch passed Doctor.
- `ROOT/repo/verify-output.txt` does not exist.

- **Run.** Execute the fixture. Run `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" verify_pty --out "$PWD/$RUN/run-session" -- cruise exec --config ../run-session.yaml "hello from verify-cruise"`. `result.json` has `"exit_code": 0`; `01-final.txt` shows `> write`, `> show`, the line `hello from verify-cruise`, and `workflow complete (2 run, 0 skipped, 0 failed)`.
- **Effect.** Read the file the workflow wrote. Run `cat "$ROOT/repo/verify-output.txt"`. It prints `hello from verify-cruise`.
- **Proof.** Render the screen. Run `node .agents/skills/verify-cruise/scripts/screen-png.mjs "$RUN/run-session/01-final.html"`. The PNG shows the command line and the summary.

## Gotchas

- An exec session is removed from `cruise list` after it completes; prove the result from the screen, `exit_code`, and the file.
- `exec` needs an attached branch; a detached `HEAD` in `ROOT/repo` fails before any step runs.
