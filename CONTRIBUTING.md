# Contributing

Thank you for your interest in contributing.

## How to Contribute

1. Open an issue or discussion for non-trivial changes before starting work.
2. Fork the repository and create a topic branch.
3. Keep changes focused and include tests or documentation updates when appropriate.
4. Run the repository's formatter, linter, and test commands before opening a pull request.
5. Open a pull request with a clear summary and any relevant context.

## Test Synchronization

- Hook tests: ignore only `ErrorKind::BrokenPipe`; assert exit code and stderr with oversized input
- TUI creation: wait for the selected `Planned` session, then verify persisted state
- Cancellation: wait for command-start markers and release completion explicitly
- Unix session homes: retain a duplicate lock descriptor across teardown; require immediate resume and live-owner exclusion

## Pull Request Guidelines

- Keep pull requests small and reviewable.
- Explain why the change is needed, not only what changed.
- Link related issues when applicable.
- Be respectful and constructive in reviews and discussions.

## Security Issues

Please do not disclose security vulnerabilities in public issues. See `SECURITY.md` for reporting instructions.
