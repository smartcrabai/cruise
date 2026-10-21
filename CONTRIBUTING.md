# Contributing

Thank you for your interest in contributing.

## How to Contribute

1. Open an issue or discussion for non-trivial changes before starting work.
2. Fork the repository and create a topic branch.
3. Keep changes focused and include tests or documentation updates when appropriate.
4. Run the repository's formatter, linter, and test commands before opening a pull request.
5. Open a pull request with a clear summary and any relevant context.

## Native Update Smoke Test

Run `python3 scripts/test_update_smoke.py --build` (`python` on Windows) to build
real A/B binaries and exercise native updates, recovery, and launcher fallback.
The fixed-pair reinstall scenario changes the runtime's mtime without changing
its bytes: overlay validity depends on the fixed pair's size and mtime, not file
identity. This avoids replacing an executable in place on Windows while still
verifying fallback and retention of the previous overlay and active pointer.

## Pull Request Guidelines

- Keep pull requests small and reviewable.
- Explain why the change is needed, not only what changed.
- Link related issues when applicable.
- Be respectful and constructive in reviews and discussions.

## Security Issues

Please do not disclose security vulnerabilities in public issues. See `SECURITY.md` for reporting instructions.
