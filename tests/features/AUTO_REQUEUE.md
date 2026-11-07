# Auto-requeue CLI Gherkin contracts

Run from the repository root with the pinned Rust toolchain and Cargo lockfile:

```sh
cargo test --locked --test auto_requeue_bdd
```

The Cucumber runner invokes the built `cruise` binary with isolated home,
configuration, and session directories. It checks configuration persistence,
validation, the single-run flag boundary, batch startup exclusion for previously
failed sessions, and retry budget enforcement on a real failing workflow. The
startup fixture first creates a planned session through `cruise plan`, then edits
its persisted state to represent a prior failure before invoking `cruise run --all`.
The retry scenario creates a separate temporary Git repository and executes a
workflow whose no-file-changes condition fails, with limits of zero and two.

The retry scenario checks the final failure, persisted retry count, and retry
notifications. It does not independently establish the exact resumed step.
