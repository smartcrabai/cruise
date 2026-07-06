# Contributing

Thank you for your interest in contributing.

## How to Contribute

1. Open an issue or discussion for non-trivial changes before starting work.
2. Fork the repository and create a topic branch.
3. Keep changes focused and include tests or documentation updates when appropriate.
4. Run the repository's formatter, linter, and test commands before opening a pull request.
5. Open a pull request with a clear summary and any relevant context.

## Development

Before committing and opening a pull request, run the following checks locally:

```sh
# Build the workspace
cargo build

# Run the test suite
cargo test

# Run the linter
cargo clippy

# Apply formatting across the entire workspace before committing
cargo fmt --all
```

## Pull Request Guidelines

- Keep pull requests small and reviewable.
- Explain why the change is needed, not only what changed.
- Link related issues when applicable.
- Be respectful and constructive in reviews and discussions.
- This repository uses **squash merge** for pull requests. Keep your branch history tidy, and make sure the final PR title and description describe the change clearly, because they will become the squashed commit message.

## Security Issues

Please do not disclose security vulnerabilities in public issues. See `SECURITY.md` for reporting instructions.
