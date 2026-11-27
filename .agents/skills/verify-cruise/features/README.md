# Cruise verification map

The maintained recipes for proving Cruise behavior from the user's side. Read this index, then use the feature file whose `Covers:` paths match the change.

## Baseline preconditions

- `ROOT=$(.agents/skills/verify-cruise/scripts/launch.sh)` printed a run root and `.agents/skills/verify-cruise/scripts/doctor.sh "$ROOT"` passed.
- `ROOT/repo` is a Git repository on `main` with one empty commit; `ROOT/run-session.yaml` and `ROOT/choose.yaml` are the workflow fixtures.
- `RUN=target/verify-cruise/<timestamp>` exists for evidence.
- Never drive a daemon or data directory that `launch.sh` did not create.

## Driving conventions

- Terminal actions: `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" verify_pty --out "$PWD/$RUN/<feature>" [ops] -- cruise ...`.
- Browser actions: `node .agents/skills/verify-cruise/scripts/webui-session.mjs "$ROOT" <session-id> "<choice>" "$RUN/<feature>"`.
- Read state back with `.agents/skills/verify-cruise/scripts/in.sh "$ROOT" cruise list --json` and files under `ROOT/repo`.
- Select sessions by ID or phase; earlier recipes leave sessions behind.
- Pass fixture paths relative to `ROOT/repo` (`../choose.yaml`) so screens show short commands.

## Proof and skip reporting

- Terminal proof: the snapshot screens (`NN-*.txt`, rendered `NN-*.png`), `result.json` with `exit_code`, and the read-back phase or file.
- Browser proof: a screenshot and ARIA snapshot for each state, plus `result.json`.
- Name the feature and entry point with every artifact; report an unreachable path with its command and the unmet precondition. Do not report a skipped entry point as verified through another path.

## Feature entry contract

Each feature file starts with an H1 title, one paragraph of user-visible behavior, and a `Covers:` line of repository paths whose change requires driving it. Then exactly four H2 sections in this order: `Sub-features`, `How to get to it (user POV)`, `Driving it with verify_pty / webui-session`, `Gotchas`.

## Features

- [Run a workflow in the current directory](./run-session.md): `cruise exec` with a command workflow, streamed step output, and the written file.
- [Approve a plan in the terminal](./plan-approve.md): the plan action menu and skip selector, ending in a Planned session.
- [Answer an Option prompt in the terminal](./terminal-option.md): the workspace choice and an Option menu during `cruise run`.
- [Answer an Option prompt in the WebUI](./webui-option.md): run from the session page, answer in the dialog, and see the completed log.

Not mapped yet: Manager goals (needs a provider fixture to reach approval), `cruise list` actions, and the Quint model review added by #917.
